//! Тени из прошлого кадра, пока камера стоит (опыт Р3, «Р3» в docs/3d-beyond-triangles.md).
//!
//! Приём из Commander Keen: перерисовывать только то, что изменилось. Если камера, лампы,
//! настройки и всё неподвижное те же, что в прошлом кадре, точка «теней 3D» может
//! измениться только рядом с движущимися предметами: где видна сама коробка предмета (с
//! запасом на затенение в углах) и где лежит её тень от солнца и двух ламп с тенями
//! (`space::dirty_tiles`). Остальные квадраты картинки теней берутся из прошлого кадра:
//! проход начинается с копии прошлой картинки, шейдер пропускает неизменные квадраты.
//! Числа в них те же, что дал бы пересчёт: те же входы — тот же ответ.
//!
//! Прошлая картинка хранится вне графа (`history`): после «теней 3D» кадр копирует в неё
//! свою картинку теней.

use ash::vk;
use reyvec_sdf::SpaceFrame;
use reyvec_sdf::shape3d::Lamp3;

use crate::context::GpuContext;
use crate::resources::{GpuResources, TextureId};

/// Что должно совпасть с прошлым кадром, чтобы взять его тени.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Key {
    pub view_proj: glam::Mat4,
    /// Размер кадра лучей 3D.
    pub full: (u32, u32),
    /// Сколько ламп: сами лампы сверяет [`ShadowReuse::moved_lamps`].
    pub lamps: usize,
    /// Настройки, от которых зависят тени (флажки панели, кэш солнца готов, …).
    pub settings: u64,
}

/// Что проходам кадра нужно знать.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReuseFrame {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub extent: vk::Extent2D,
    /// Тени этого кадра начинаются с прошлой картинки (иначе считаются целиком).
    pub reuse: bool,
    /// Прошлая картинка уже была записана копированием (иначе её раскладка UNDEFINED).
    pub written: bool,
}

/// Тени из прошлого кадра.
pub(crate) struct ShadowReuse {
    /// Флажок на панели.
    pub enabled: bool,
    key: Option<Key>,
    still: Option<SpaceFrame>,
    /// Лампы с тенями прошлого кадра.
    lamps: Vec<Lamp3>,
    /// Изменившиеся квадраты прошлого кадра (их тоже пересчитать: предмет оттуда ушёл).
    last: Vec<u32>,
    history: Option<(TextureId, vk::Extent2D)>,
    written: bool,
    /// Квадратов пересчитано и всего в последнем кадре (для теста и журнала).
    pub last_counts: Option<(u32, u32)>,
}

impl ShadowReuse {
    pub fn new() -> Self {
        ShadowReuse {
            enabled: false,
            key: None,
            still: None,
            lamps: Vec::new(),
            last: Vec::new(),
            history: None,
            written: false,
            last_counts: None,
        }
    }

    /// Кадр без опыта (выключен, другой вид): следующий считается целиком.
    pub fn forget(&mut self, res: &mut GpuResources) {
        self.key = None;
        self.still = None;
        self.lamps.clear();
        self.last.clear();
        self.last_counts = None;
        if !self.enabled
            && let Some((id, _)) = self.history.take()
        {
            res.destroy_texture(id);
            self.written = false;
        }
    }

    /// Лампы с тенями (первые две), которые сдвинулись или поменялись с прошлого кадра:
    /// место и дальность до и после. Тени такой лампы меняются в шаре её дальности.
    /// Остальные лампы «теней 3D» не касаются.
    pub fn moved_lamps(&mut self, lamps: &[Lamp3]) -> Vec<(glam::Vec3, f32)> {
        let shadowed = reyvec_sdf::shape3d::SHADOW_LAMPS;
        let mut moved = Vec::new();
        for (k, lamp) in lamps.iter().take(shadowed).enumerate() {
            match self.lamps.get(k) {
                Some(old) if old == lamp => {}
                Some(old) => {
                    moved.push((glam::Vec3::from(old.at), old.range));
                    moved.push((glam::Vec3::from(lamp.at), lamp.range));
                }
                None => {}
            }
        }
        self.lamps = lamps.iter().take(shadowed).copied().collect();
        moved
    }

    /// Сверяет неподвижное кадра `frame` (до отсева по взгляду) с прошлым: `true` — то же.
    pub fn same_still(&mut self, frame: &SpaceFrame) -> bool {
        let same = self
            .still
            .as_ref()
            .is_some_and(|s| crate::space::same_still(frame, s));
        if !same {
            self.still = Some(crate::space::still_frame(frame));
        }
        same
    }

    /// Готовит кадр: `key` — этот кадр, `still_same` — неподвижное то же, `dirty` —
    /// изменившиеся квадраты от движущихся этого кадра (`None` — меняется всё). Пишет в
    /// `words` раздел квадратов, если тени берутся из прошлого кадра. `half` — размер
    /// картинки теней.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        ctx: &GpuContext,
        res: &mut GpuResources,
        key: Key,
        still_same: bool,
        dirty: Option<(u32, Vec<u32>)>,
        half: vk::Extent2D,
        format: vk::Format,
        words: &mut Vec<u32>,
    ) -> Option<ReuseFrame> {
        if self.history.is_some_and(|(_, extent)| extent != half)
            && let Some((id, _)) = self.history.take()
        {
            res.destroy_texture(id);
        }
        if self.history.is_none() {
            match res.create_texture_format(
                ctx,
                half.width,
                half.height,
                format,
                vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::TRANSFER_DST,
                "тени 3D: прошлый кадр",
            ) {
                Ok(id) => {
                    self.history = Some((id, half));
                    self.written = false;
                    self.key = None;
                }
                Err(e) => {
                    tracing::warn!("картинка прошлых теней: {e}");
                    self.enabled = false;
                    return None;
                }
            }
        }
        let (id, extent) = self.history?;
        let same = still_same && self.written && self.key.as_ref() == Some(&key);
        let mut reuse = false;
        self.last_counts = None;
        match dirty {
            Some((across, bits)) => {
                if same && bits.len() == self.last.len() {
                    let merged: Vec<u32> =
                        bits.iter().zip(&self.last).map(|(a, b)| a | b).collect();
                    reuse = crate::space::set_dirty_tiles(words, across, &merged);
                    if reuse {
                        let down = (half.height).div_ceil(crate::space::DIRTY_TILE);
                        let ones: u32 = merged.iter().map(|w| w.count_ones()).sum();
                        self.last_counts = Some((ones, across * down));
                    }
                }
                self.last = bits;
                self.key = Some(key);
            }
            None => {
                self.last.clear();
                self.key = None;
            }
        }
        let frame = ReuseFrame {
            image: res.texture_image(id)?,
            view: res.texture_view(id)?,
            extent,
            reuse,
            written: self.written,
        };
        // Этот кадр копирует в прошлую картинку свои тени.
        self.written = true;
        Some(frame)
    }
}

impl Default for ShadowReuse {
    fn default() -> Self {
        Self::new()
    }
}
