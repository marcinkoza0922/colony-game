use ecs::Entity;
use serde::{Deserialize, Serialize};

use crate::defs::{BuildingId, ItemId, PlantId, QuirkId, RaceId, RecipeId, ThoughtId};
use crate::geom::P3;
use crate::jobs::Job;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position(pub P3);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pawn {
    pub name: String,
    pub race: RaceId,
    pub age: u32,
}

/// Need levels in 0..=1, indexed by `NeedId`. Needs the race lacks stay at 1.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Needs {
    pub values: Vec<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Memory {
    pub thought: ThoughtId,
    pub ticks_left: u32,
    /// Who the thought is about ("Insulted by {name}").
    pub about: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mood {
    /// Current mood, 0..=100 (eases toward `target`).
    pub value: f32,
    pub target: f32,
    pub memories: Vec<Memory>,
    /// Situational thoughts active at the last evaluation.
    pub situational: Vec<ThoughtId>,
}

impl Default for Mood {
    fn default() -> Self {
        Self { value: 50.0, target: 50.0, memories: Vec::new(), situational: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct SkillLevel {
    pub level: u8,
    pub xp: f32,
}

pub const MAX_SKILL: u8 = 20;

pub fn xp_to_next(level: u8) -> f32 {
    1000.0 + 500.0 * level as f32
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Skills {
    pub levels: Vec<SkillLevel>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Quirks(pub Vec<QuirkId>);

/// 0 = disabled, 1 = highest … 4 = lowest. Indexed by `WorkTypeId`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkPriorities(pub Vec<u8>);

/// Movement along a path. `from` is the cell we're leaving; `path[next]` is the
/// cell we're entering; `progress` is in cost units (10 per orthogonal step).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mover {
    pub path: Vec<P3>,
    pub next: usize,
    pub from: P3,
    pub progress: f32,
    pub step_cost: f32,
}

impl Mover {
    pub fn target(&self) -> Option<P3> {
        self.path.last().copied()
    }
    /// Fraction 0..1 along the current step (for rendering).
    pub fn frac(&self) -> f32 {
        if self.step_cost <= 0.0 { 0.0 } else { (self.progress / self.step_cost).clamp(0.0, 1.0) }
    }
    pub fn current_step(&self) -> Option<P3> {
        self.path.get(self.next).copied()
    }
}

/// Screen-independent facing, 0..8 in map space (0 = north, clockwise).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Facing(pub u8);

impl Facing {
    pub fn from_delta(dx: i32, dy: i32) -> Option<Self> {
        let f = match (dx.signum(), dy.signum()) {
            (0, -1) => 0,
            (1, -1) => 1,
            (1, 0) => 2,
            (1, 1) => 3,
            (0, 1) => 4,
            (-1, 1) => 5,
            (-1, 0) => 6,
            (-1, -1) => 7,
            _ => return None,
        };
        Some(Facing(f))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Inventory {
    pub carrying: Option<Entity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BreakKind {
    /// Wanders aimlessly, refuses work.
    Sulking,
    /// Hides in bed, refuses work.
    Hiding,
    /// Eats everything it can find.
    FoodBinge,
}

impl BreakKind {
    pub fn label(self) -> &'static str {
        match self {
            BreakKind::Sulking => "Sulking",
            BreakKind::Hiding => "Hiding",
            BreakKind::FoodBinge => "Food binge",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MentalState {
    pub kind: BreakKind,
    pub ticks_left: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CurrentJob(pub Job);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub def: ItemId,
    pub count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plant {
    pub def: PlantId,
    /// 0..=1
    pub growth: f32,
    /// 0..=1; the plant dies at 0. Harvest yield scales with it.
    pub health: f32,
}

impl Plant {
    pub fn new(def: PlantId, growth: f32) -> Self {
        Self { def, growth, health: 1.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Building {
    pub def: BuildingId,
    /// 0..4 map-space rotation.
    pub rot: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Blueprint {
    pub def: BuildingId,
    pub rot: u8,
    /// Delivered counts, parallel to the def's `cost` list.
    pub delivered: Vec<u32>,
    pub work_done: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BillMode {
    /// Make this many, then the bill completes.
    Count(u32),
    /// Keep making.
    Forever,
    /// Make until the colony has this many of the first output.
    UntilHave(u32),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bill {
    pub id: u32,
    pub recipe: RecipeId,
    pub mode: BillMode,
    pub suspended: bool,
    pub done: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Workbench {
    pub bills: Vec<Bill>,
    /// Ingredients hauled into the bench for the current craft.
    pub stored: Vec<(ItemId, u32)>,
    pub next_bill_id: u32,
}

/// Visual genes, rolled from the race's palettes. Purely cosmetic.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Appearance {
    pub skin: [u8; 3],
    pub hair: [u8; 3],
    pub top: [u8; 3],
    pub bottom: [u8; 3],
    /// Hair/head style index (sprite set `pawn_<race>_s<style>`).
    pub style: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Bed {
    pub owner: Option<Entity>,
    /// Second sleeper of a double bed (the owner's partner).
    pub second: Option<Entity>,
}

impl Bed {
    pub fn sleeps(&self, e: Entity) -> bool {
        self.owner == Some(e) || self.second == Some(e)
    }
}
