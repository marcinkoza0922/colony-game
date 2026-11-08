use std::collections::BTreeMap;

use ecs::Entity;
use serde::{Deserialize, Serialize};

use crate::defs::{Defs, ItemCategory, ItemId};
use crate::geom::P3;

pub const TICKS_PER_SECOND: u64 = 30;
pub const TICKS_PER_HOUR: u64 = 1250;
pub const TICKS_PER_DAY: u64 = TICKS_PER_HOUR * 24;
/// The game starts at 08:00 on day 1.
const START_OFFSET: u64 = TICKS_PER_HOUR * 8;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Clock {
    pub tick: u64,
}

impl Clock {
    fn t(&self) -> u64 {
        self.tick + START_OFFSET
    }
    /// Day number, starting at 1.
    pub fn day(&self) -> u64 {
        self.t() / TICKS_PER_DAY + 1
    }
    /// Hour of day as a float 0..24.
    pub fn hour_f(&self) -> f32 {
        (self.t() % TICKS_PER_DAY) as f32 / TICKS_PER_HOUR as f32
    }
    pub fn hour(&self) -> u32 {
        self.hour_f() as u32
    }
    pub fn minute(&self) -> u32 {
        ((self.hour_f().fract()) * 60.0) as u32
    }
    /// Night is 22:00–06:00, shifted by `shift` hours (e.g. Night Owls).
    pub fn is_night_shifted(&self, shift: i32) -> bool {
        let h = (self.hour() as i32 - shift).rem_euclid(24);
        !(6..22).contains(&h)
    }
    pub fn is_night(&self) -> bool {
        self.is_night_shifted(0)
    }
    /// Daylight factor 0..1 for rendering.
    pub fn daylight(&self) -> f32 {
        let h = self.hour_f();
        let sun = ((h - 6.0) / 12.0 * std::f32::consts::PI).sin();
        sun.clamp(0.0, 1.0).powf(0.5)
    }
    pub fn time_string(&self) -> String {
        format!("Day {} {:02}:{:02}", self.day(), self.hour(), self.minute())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogKind {
    Info,
    Good,
    Bad,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    pub tick: u64,
    pub kind: LogKind,
    pub text: String,
    /// Who/what the entry is about (used for floating text over colonists).
    pub subject: Option<ecs::Entity>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GameLog {
    pub entries: Vec<LogEntry>,
}

impl GameLog {
    pub fn push(&mut self, tick: u64, kind: LogKind, text: impl Into<String>) {
        self.push_about(tick, kind, None, text);
    }

    pub fn push_about(&mut self, tick: u64, kind: LogKind, subject: Option<ecs::Entity>, text: impl Into<String>) {
        self.entries.push(LogEntry { tick, kind, text: text.into(), subject });
        if self.entries.len() > 300 {
            self.entries.drain(0..100);
        }
    }
}

// ------------------------------------------------------------------- zones

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stockpile {
    pub id: u16,
    pub name: String,
    pub allow_resources: bool,
    pub allow_raw_food: bool,
    pub allow_meals: bool,
    pub allow_manufactured: bool,
    pub cells: Vec<P3>,
}

impl Stockpile {
    pub fn accepts(&self, defs: &Defs, item: ItemId) -> bool {
        match defs.items[item].category {
            ItemCategory::Resource => self.allow_resources,
            ItemCategory::RawFood => self.allow_raw_food,
            ItemCategory::Meal => self.allow_meals,
            ItemCategory::Manufactured => self.allow_manufactured,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Zones {
    pub stockpiles: BTreeMap<u16, Stockpile>,
    pub next_id: u16,
}

impl Zones {
    pub fn get(&self, id: u16) -> Option<&Stockpile> {
        self.stockpiles.get(&id)
    }
}

// ------------------------------------------------------------ reservations

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ResKey {
    Entity(Entity),
    Cell(P3),
}

/// Who has claimed what. Prevents two pawns working the same target.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Reservations {
    map: BTreeMap<ResKey, Entity>,
}

impl Reservations {
    pub fn holder(&self, k: ResKey) -> Option<Entity> {
        self.map.get(&k).copied()
    }
    /// True if reserved by someone other than `by`.
    pub fn blocked(&self, k: ResKey, by: Entity) -> bool {
        self.map.get(&k).is_some_and(|h| *h != by)
    }
    pub fn try_reserve(&mut self, k: ResKey, by: Entity) -> bool {
        if self.blocked(k, by) {
            return false;
        }
        self.map.insert(k, by);
        true
    }
    pub fn release(&mut self, k: ResKey, by: Entity) {
        if self.map.get(&k) == Some(&by) {
            self.map.remove(&k);
        }
    }
    pub fn release_all(&mut self, by: Entity) {
        self.map.retain(|_, h| *h != by);
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Snapshot of pawn positions, refreshed at the start of each tick.
#[derive(Clone, Debug, Default)]
pub struct PawnPositions(pub Vec<(Entity, P3)>);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimMeta {
    pub seed: u64,
    pub start: P3,
}
