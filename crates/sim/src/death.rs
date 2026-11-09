//! Death, corpses, rot and burial.

use std::rc::Rc;

use ecs::{Entity, World};
use serde::{Deserialize, Serialize};

use crate::components::*;
use crate::defs::*;
use crate::geom::P3;
use crate::items;
use crate::jobs;
use crate::map::Map;
use crate::resources::*;

/// Attached to the corpse item a dead colonist leaves.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Corpse {
    pub name: String,
    pub race: RaceId,
    pub appearance: Appearance,
    pub cause: String,
    pub died_tick: u64,
    /// The colonist entity it used to be (for relationships).
    pub was: Entity,
}

impl Corpse {
    pub fn age_days(&self, now: u64) -> f32 {
        now.saturating_sub(self.died_tick) as f32 / TICKS_PER_DAY as f32
    }
}

/// Days after death when a corpse starts to rot.
pub const ROT_DAYS: f32 = 2.0;

/// A dug grave; `occupant` once someone is buried in it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Grave {
    pub occupant: Option<String>,
}

/// Positions of corpses lying on the map (refreshed each tick, not saved).
#[derive(Clone, Debug, Default)]
pub struct CorpsePositions(pub Vec<P3>);

fn defs(world: &World) -> Rc<Defs> {
    world.res::<Rc<Defs>>().clone()
}

/// Turns a pawn into a corpse and lets the colony grieve.
pub fn on_death(world: &mut World, pawn: Entity, cause: &str) {
    let defs = defs(world);
    let tick = world.res::<Clock>().tick;
    let pos = world.get::<Position>(pawn).map(|p| p.0);
    let name = jobs::pawn_name(world, pawn);
    let race = world.get::<Pawn>(pawn).map(|p| p.race);
    let appearance = world.get_cloned::<Appearance>(pawn).unwrap_or_default();
    // Free its bed.
    let beds: Vec<Entity> = world.read::<Bed>().iter().filter(|(_, b)| b.owner == Some(pawn)).map(|(e, _)| e).collect();
    for b in beds {
        world.get_mut::<Bed>(b).unwrap().owner = None;
    }
    // Anyone carrying it drops it; anyone it was carrying is dropped.
    crate::medical::release_carry(world, pawn);
    crate::social::on_death(world, pawn, &name);
    world.res_mut::<Reservations>().release_all(pawn);
    world.despawn(pawn);
    if let (Some(p), Some(race), Some(cd)) = (pos, race, defs.items.find("corpse")) {
        let spawned = items::spawn_items(world, cd, 1, p);
        if let Some(&e) = spawned.first() {
            world.insert(
                e,
                Corpse { name, race, appearance, cause: cause.to_string(), died_tick: tick, was: pawn },
            );
        }
    }
}

const CORPSE_INTERVAL: u64 = 250;

/// Rotting corpses foul the air around them.
pub fn sys_corpses(world: &mut World) {
    let tick = world.res::<Clock>().tick;
    let v: Vec<P3> = {
        let corpses = world.read::<Corpse>();
        let pos = world.read::<Position>();
        corpses.entities().iter().filter_map(|e| pos.get(*e).map(|p| p.0)).collect()
    };
    world.res_mut::<CorpsePositions>().0 = v;
    if !tick.is_multiple_of(CORPSE_INTERVAL) {
        return;
    }
    let defs = defs(world);
    let Some(filth) = defs.core.field_filth else { return };
    let rotting: Vec<P3> = {
        let corpses = world.read::<Corpse>();
        let pos = world.read::<Position>();
        corpses
            .iter()
            .filter(|(_, c)| c.age_days(tick) > ROT_DAYS)
            .filter_map(|(e, _)| pos.get(e).map(|p| p.0))
            .collect()
    };
    let map = world.res::<Map>();
    let mut env = world.res_mut::<crate::env::Env>();
    for p in rotting {
        if map.in_bounds(p) {
            env.queue_add(filth, map.idx(p), 0.15);
        }
    }
}

/// Buries the carried corpse in the grave.
pub fn bury(world: &mut World, pawn: Entity, grave: Entity) -> bool {
    let Some(item) = world.get::<Inventory>(pawn).and_then(|i| i.carrying) else { return false };
    let Some(c) = world.get_cloned::<Corpse>(item) else { return false };
    let Some(mut g) = world.get_mut::<Grave>(grave) else { return false };
    if g.occupant.is_some() {
        return false;
    }
    g.occupant = Some(c.name.clone());
    drop(g);
    world.get_mut::<Inventory>(pawn).unwrap().carrying = None;
    world.despawn(item);
    let who = jobs::pawn_name(world, pawn);
    jobs::log_about(world, LogKind::Info, pawn, format!("{who} buried {}.", c.name));
    true
}
