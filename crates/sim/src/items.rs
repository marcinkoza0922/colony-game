//! Item stack helpers: spawning, placing, merging and counting.

use std::rc::Rc;

use ecs::{Entity, World};

use crate::components::{Item, Position};
use crate::defs::{Defs, ItemId};
use crate::geom::P3;
use crate::map::Map;
use crate::path::Pathfinder;
use crate::resources::Zones;

fn defs(world: &World) -> Rc<Defs> {
    world.res::<Rc<Defs>>().clone()
}

/// Finds a cell near `near` that can take (some of) `def`: an empty walkable
/// cell, or one holding a non-full stack of the same def.
fn find_drop_cell(world: &World, def: ItemId, near: P3, exclude: Option<Entity>) -> Option<P3> {
    let defs = defs(world);
    let stack = defs.items[def].stack;
    let map = world.res::<Map>();
    let items = world.read::<Item>();
    let mut pf = world.res_mut::<Pathfinder>();
    let ok = |p: P3| -> bool {
        let Some(c) = map.get(p) else { return false };
        if !map.is_walkable(p) || c.ramp.is_some() {
            return false;
        }
        match c.item {
            None => true,
            Some(e) if Some(e) == exclude => false,
            Some(e) => items.get(e).is_some_and(|it| it.def == def && it.count < stack),
        }
    };
    if ok(near) {
        return Some(near);
    }
    pf.search(&map, near, 600, |p| ok(p).then_some(p)).map(|(p, _)| p)
}

/// Places `count` of `def` on the map near `near`, merging into existing
/// stacks where possible. Returns entities that received items.
pub fn spawn_items(world: &mut World, def: ItemId, mut count: u32, near: P3) -> Vec<Entity> {
    let stack = defs(world).items[def].stack;
    let mut out = Vec::new();
    while count > 0 {
        let Some(cell) = find_drop_cell(world, def, near, None) else { break };
        let existing = world.res::<Map>().cell(cell).item;
        match existing {
            Some(e) => {
                let mut it = world.get_mut::<Item>(e).unwrap();
                let add = (stack - it.count).min(count);
                it.count += add;
                count -= add;
                out.push(e);
            }
            None => {
                let n = count.min(stack);
                let e = world.spawn((Item { def, count: n }, Position(cell)));
                world.res_mut::<Map>().cell_mut(cell).item = Some(e);
                count -= n;
                out.push(e);
            }
        }
    }
    out
}

/// Puts an existing (carried / position-less) item entity onto the map near
/// `near`. Merges into a stack if needed (despawning the entity if fully merged).
pub fn drop_item(world: &mut World, item: Entity, near: P3) {
    let Some(it) = world.get_cloned::<Item>(item) else { return };
    let stack = defs(world).items[it.def].stack;
    let Some(cell) = find_drop_cell(world, it.def, near, Some(item)) else {
        // Nowhere to put it: destroy rather than leave a ghost entity.
        world.despawn(item);
        return;
    };
    let existing = world.res::<Map>().cell(cell).item;
    match existing {
        Some(e) => {
            let room = {
                let other = world.get::<Item>(e).unwrap();
                stack - other.count
            };
            let moved = room.min(it.count);
            world.get_mut::<Item>(e).unwrap().count += moved;
            let left = it.count - moved;
            if left == 0 {
                world.despawn(item);
            } else {
                world.get_mut::<Item>(item).unwrap().count = left;
                drop_item(world, item, near);
            }
        }
        None => {
            world.insert(item, Position(cell));
            world.res_mut::<Map>().cell_mut(cell).item = Some(item);
        }
    }
}

/// Removes an item entity from its map cell (it keeps existing, position-less).
pub fn lift_item(world: &mut World, item: Entity) {
    if let Some(Position(p)) = world.remove::<Position>(item) {
        let mut map = world.res_mut::<Map>();
        if let Some(c) = map.get_mut(p)
            && c.item == Some(item)
        {
            c.item = None;
        }
    }
}

/// Removes an item entity from the world entirely.
pub fn destroy_item(world: &mut World, item: Entity) {
    lift_item(world, item);
    world.despawn(item);
}

/// Splits `count` off an on-map stack. Returns the carried entity (either the
/// original, lifted off the map, or a new split stack).
pub fn take_from_stack(world: &mut World, item: Entity, count: u32) -> Option<Entity> {
    let it = world.get_cloned::<Item>(item)?;
    if count == 0 {
        return None;
    }
    if count >= it.count {
        lift_item(world, item);
        Some(item)
    } else {
        world.get_mut::<Item>(item).unwrap().count -= count;
        Some(world.spawn((Item { def: it.def, count },)))
    }
}

/// Is the item resting in a stockpile that accepts it?
pub fn is_stored(world: &World, item: Entity) -> bool {
    let Some(pos) = world.get::<Position>(item).map(|p| p.0) else { return false };
    let Some(def) = world.get::<Item>(item).map(|i| i.def) else { return false };
    let map = world.res::<Map>();
    let Some(zone) = map.cell(pos).zone else { return false };
    let zones = world.res::<Zones>();
    let defs = defs(world);
    zones.get(zone).is_some_and(|z| z.accepts(&defs, def))
}

/// Total of `def` lying on the map (not carried).
pub fn colony_count(world: &World, def: ItemId) -> u32 {
    let items = world.read::<Item>();
    let pos = world.read::<Position>();
    items.iter().filter(|(e, it)| it.def == def && pos.contains(*e)).map(|(_, it)| it.count).sum()
}
