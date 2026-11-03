//! Звук событий игры (6.7): какой звук, насколько громко и с какой стороны.

use reyvec_audio::Sound;
use reyvec_scene::GameEvent;

/// Звук события, громкость в децибелах и место в сцене (для положения слева направо).
pub(crate) fn sound_for(event: &GameEvent) -> (Sound, f32, [f32; 2]) {
    match *event {
        GameEvent::Run { at, .. } => (Sound::Step, -12.0, at),
        GameEvent::Jumped { at } => (Sound::Jump, -9.0, at),
        // Чем сильнее упал, тем громче (с 4 единиц в секунду уже в полную силу).
        GameEvent::Landed { at, speed } => (Sound::Land, -18.0 + 4.5 * speed.min(4.0), at),
        GameEvent::Dashed { at, .. } => (Sound::Dash, -8.0, at),
        GameEvent::Swing { at, .. } => (Sound::Swing, -9.0, at),
        GameEvent::Hit { at, .. } => (Sound::Hit, -2.0, at),
    }
}

/// Положение слева направо (-1…1) для точки сцены `x` при камере с серединой
/// `center`, приближением `zoom` и отношением сторон окна `aspect`. Край экрана — 0,8:
/// совсем в одно ухо звучит неестественно.
pub(crate) fn pan_for(x: f32, center: f64, zoom: f64, aspect: f64) -> f32 {
    let screen = (f64::from(x) - center) * zoom / aspect.max(1e-3);
    (screen.clamp(-1.0, 1.0) * 0.8) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harder_landing_is_louder_and_hit_is_loudest() {
        let soft = sound_for(&GameEvent::Landed {
            at: [0.0, 0.0],
            speed: 0.5,
        });
        let hard = sound_for(&GameEvent::Landed {
            at: [0.0, 0.0],
            speed: 6.0,
        });
        assert!(hard.1 > soft.1 && hard.1 <= 0.0);
        let hit = sound_for(&GameEvent::Hit {
            at: [0.0, 0.0],
            dir: 1.0,
        });
        assert_eq!(hit.0, Sound::Hit);
        let step = sound_for(&GameEvent::Run {
            at: [0.0, 0.0],
            dir: 1.0,
        });
        assert!(hit.1 > step.1, "удар громче шага");
    }

    #[test]
    fn pan_follows_the_screen_side() {
        assert_eq!(pan_for(0.0, 0.0, 1.0, 16.0 / 9.0), 0.0);
        assert!(pan_for(1.0, 0.0, 1.0, 16.0 / 9.0) > 0.4);
        assert!(pan_for(-5.0, 0.0, 1.0, 16.0 / 9.0) == -0.8);
    }
}
