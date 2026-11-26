//! Задания: состояние и переходы между стадиями.

use std::collections::BTreeMap;

use crate::{State, Story};

/// Где задание сейчас.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// Не взято.
    #[default]
    None,
    Active,
    Done,
    Failed,
}

impl Status {
    /// Имя для условий Luau (`quest.mech.state == "active"`).
    pub fn name(self) -> &'static str {
        match self {
            Status::None => "none",
            Status::Active => "active",
            Status::Done => "done",
            Status::Failed => "failed",
        }
    }
}

/// Состояние одного задания.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestState {
    pub status: Status,
    /// Текущая стадия (у выполненного — последняя).
    pub stage: String,
    /// Сколько сделано по целям текущей стадии.
    pub progress: BTreeMap<String, u32>,
}

impl Story {
    /// Состояние задания (не взятое — пустое).
    pub fn quest_state(&self, state: &State, id: &str) -> QuestState {
        state.quests.get(id).cloned().unwrap_or_default()
    }

    /// Начать задание с первой стадии. Уже начатое или неизвестное — ничего не делает.
    pub fn quest_start(&self, state: &mut State, id: &str) -> Result<(), String> {
        let q = self.quests.get(id).ok_or(format!("нет задания «{id}»"))?;
        let first = q
            .stages
            .first()
            .ok_or(format!("у задания «{id}» нет стадий"))?;
        let s = state.quests.entry(id.to_string()).or_default();
        if s.status == Status::None {
            *s = QuestState {
                status: Status::Active,
                stage: first.id.clone(),
                progress: BTreeMap::new(),
            };
        }
        Ok(())
    }

    /// Прибавить `n` к цели текущей стадии. Все цели выполнены — следующая стадия
    /// (или задание выполнено).
    pub fn quest_add(&self, state: &mut State, id: &str, goal: &str, n: u32) -> Result<(), String> {
        let q = self.quests.get(id).ok_or(format!("нет задания «{id}»"))?;
        let Some(s) = state
            .quests
            .get_mut(id)
            .filter(|s| s.status == Status::Active)
        else {
            return Ok(());
        };
        let stage = q
            .stage(&s.stage)
            .ok_or(format!("нет стадии «{}»", s.stage))?;
        let Some(g) = stage.goals.iter().find(|g| g.id == goal) else {
            // Цель другой стадии: например, руду принесли, когда стадия уже дальше.
            return Ok(());
        };
        let p = s.progress.entry(goal.to_string()).or_default();
        *p = (*p + n).min(g.count);
        let all = stage
            .goals
            .iter()
            .all(|g| s.progress.get(&g.id).copied().unwrap_or(0) >= g.count);
        if all {
            let next = stage.next.clone();
            self.quest_stage(state, id, &next)?;
        }
        Ok(())
    }

    /// Перейти на стадию. Пустое имя — задание выполнено.
    pub fn quest_stage(&self, state: &mut State, id: &str, stage: &str) -> Result<(), String> {
        let q = self.quests.get(id).ok_or(format!("нет задания «{id}»"))?;
        if !stage.is_empty() && q.stage(stage).is_none() {
            return Err(format!("у задания «{id}» нет стадии «{stage}»"));
        }
        let s = state.quests.entry(id.to_string()).or_default();
        if matches!(s.status, Status::Done | Status::Failed) {
            return Ok(());
        }
        if stage.is_empty() {
            s.status = Status::Done;
        } else {
            s.status = Status::Active;
            s.stage = stage.to_string();
            s.progress.clear();
        }
        Ok(())
    }

    /// Задание выполнено.
    pub fn quest_complete(&self, state: &mut State, id: &str) -> Result<(), String> {
        self.quest_stage(state, id, "")
    }

    /// Задание провалено.
    pub fn quest_fail(&self, state: &mut State, id: &str) -> Result<(), String> {
        if !self.quests.contains_key(id) {
            return Err(format!("нет задания «{id}»"));
        }
        let s = state.quests.entry(id.to_string()).or_default();
        if s.status != Status::Done {
            s.status = Status::Failed;
        }
        Ok(())
    }

    /// Записи журнала: (задание, запись текущей стадии, цели «текст, сделано, нужно»)
    /// для начатых заданий.
    pub fn journal(&self, state: &State) -> Vec<JournalEntry> {
        let mut out = Vec::new();
        for (id, s) in &state.quests {
            let Some(q) = self.quests.get(id) else {
                continue;
            };
            if s.status == Status::None {
                continue;
            }
            let stage = q.stage(&s.stage);
            out.push(JournalEntry {
                quest: id.clone(),
                title: q.title.clone(),
                status: s.status,
                text: stage.map(|st| st.journal.clone()).unwrap_or_default(),
                goals: stage
                    .map(|st| {
                        st.goals
                            .iter()
                            .map(|g| {
                                (
                                    g.text.clone(),
                                    s.progress.get(&g.id).copied().unwrap_or(0),
                                    g.count,
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
        out
    }
}

/// Запись журнала заданий.
#[derive(Clone, Debug, PartialEq)]
pub struct JournalEntry {
    pub quest: String,
    pub title: String,
    pub status: Status,
    pub text: String,
    /// Цели: текст, сделано, нужно.
    pub goals: Vec<(String, u32, u32)>,
}
