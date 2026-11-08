//! Player orders: the only way the player influences the simulation.
//! Every order is applied between ticks and is deterministic.

use std::rc::Rc;

use ecs::{Entity, World};
use serde::{Deserialize, Serialize};

use crate::components::*;
use crate::defs::*;
use crate::geom::P3;
use crate::jobs;
use crate::map::{Designation, Map};
use crate::resources::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesignateTool {
    Mine,
    DigStairs,
    Chop,
    Harvest,
    BuildRoof,
    RemoveRoof,
    /// Clears designations and cancels blueprints.
    Cancel,
}

fn defs(world: &World) -> Rc<Defs> {
    world.res::<Rc<Defs>>().clone()
}

/// Can this tool apply to the cell? (Used for previews too.)
pub fn designation_valid(world: &World, p: P3, tool: DesignateTool) -> bool {
    let map = world.res::<Map>();
    let Some(c) = map.get(p) else { return false };
    let defs = defs(world);
    match tool {
        DesignateTool::Mine => c.solid.is_some(),
        DesignateTool::DigStairs => {
            c.solid.is_none() && map.is_walkable(p) && p.z > 0 && map.is_solid(p.down()) && c.building.is_none()
        }
        DesignateTool::Chop => c.plant.is_some_and(|e| world.get::<Plant>(e).is_some_and(|pl| defs.plants[pl.def].tree)),
        DesignateTool::Harvest => c.plant.is_some_and(|e| {
            world.get::<Plant>(e).is_some_and(|pl| defs.plants[pl.def].harvest.as_ref().is_some_and(|h| pl.growth >= h.min_growth))
        }),
        DesignateTool::BuildRoof => c.solid.is_none() && !c.roof && c.designation == Designation::None,
        DesignateTool::RemoveRoof => c.solid.is_none() && c.roof,
        DesignateTool::Cancel => c.designation != Designation::None || c.building.is_some_and(|b| world.has::<Blueprint>(b)),
    }
}

/// Applies a designation tool to many cells. Returns how many changed.
pub fn designate(world: &mut World, cells: &[P3], tool: DesignateTool) -> usize {
    let mut n = 0;
    for &p in cells {
        if !designation_valid(world, p, tool) {
            continue;
        }
        n += 1;
        let d = match tool {
            DesignateTool::Mine => Designation::Mine,
            DesignateTool::DigStairs => Designation::DigStairs,
            DesignateTool::Chop => Designation::Chop,
            DesignateTool::Harvest => Designation::Harvest,
            DesignateTool::BuildRoof => Designation::BuildRoof,
            DesignateTool::RemoveRoof => Designation::RemoveRoof,
            DesignateTool::Cancel => {
                let bp = world.res::<Map>().cell(p).building.filter(|b| world.has::<Blueprint>(*b));
                if let Some(bp) = bp {
                    cancel_blueprint(world, bp);
                }
                Designation::None
            }
        };
        world.res_mut::<Map>().cell_mut(p).designation = d;
    }
    n
}

fn cancel_blueprint(world: &mut World, bp: Entity) {
    let Some(b) = world.get_cloned::<Blueprint>(bp) else { return };
    let p = world.get::<Position>(bp).unwrap().0;
    let defs = defs(world);
    // Refund delivered materials.
    for (c, n) in defs.buildings[b.def].cost.iter().zip(&b.delivered) {
        if *n > 0 {
            crate::items::spawn_items(world, c.item, *n, p);
        }
    }
    world.res_mut::<Map>().set_building(p, None, None);
    world.despawn(bp);
}

pub fn can_place(world: &World, p: P3, def: BuildingId) -> bool {
    let map = world.res::<Map>();
    let Some(c) = map.get(p) else { return false };
    let defs = defs(world);
    let bd = &defs.buildings[def];
    if !bd.buildable || c.building.is_some() || c.ramp.is_some() || !map.is_walkable(p) {
        return false;
    }
    // Workbenches/beds want a free cell; walls displace items when built.
    true
}

pub fn place_blueprint(world: &mut World, p: P3, def: BuildingId, rot: u8) -> Option<Entity> {
    if !can_place(world, p, def) {
        return None;
    }
    let n = defs(world).buildings[def].cost.len();
    let e = world.spawn((Blueprint { def, rot: rot % 4, delivered: vec![0; n], work_done: 0.0 }, Position(p)));
    world.res_mut::<Map>().set_building(p, Some(e), None);
    Some(e)
}

/// Deconstructs a finished building instantly (M1: no work, partial refund).
pub fn deconstruct(world: &mut World, building: Entity) {
    let Some(b) = world.get_cloned::<Building>(building) else { return };
    let p = world.get::<Position>(building).unwrap().0;
    let defs = defs(world);
    if b.def == defs.core.stairs {
        return;
    }
    for c in &defs.buildings[b.def].cost {
        let n = c.count / 2;
        if n > 0 {
            crate::items::spawn_items(world, c.item, n, p);
        }
    }
    world.res_mut::<Map>().set_building(p, None, None);
    world.despawn(building);
}

/// Creates a stockpile over the given cells (cells that are walkable and not
/// already zoned). Returns the zone id.
pub fn create_stockpile(world: &mut World, cells: &[P3]) -> Option<u16> {
    let valid: Vec<P3> = {
        let map = world.res::<Map>();
        cells.iter().copied().filter(|p| map.is_walkable(*p) && map.cell(*p).zone.is_none() && map.cell(*p).ramp.is_none()).collect()
    };
    if valid.is_empty() {
        return None;
    }
    let id = {
        let mut z = world.res_mut::<Zones>();
        let id = z.next_id;
        z.next_id += 1;
        z.stockpiles.insert(
            id,
            Stockpile {
                id,
                name: format!("Stockpile {}", id + 1),
                allow_resources: true,
                allow_raw_food: true,
                allow_meals: true,
                allow_manufactured: true,
                cells: valid.clone(),
            },
        );
        id
    };
    let mut map = world.res_mut::<Map>();
    for p in valid {
        map.cell_mut(p).zone = Some(id);
    }
    Some(id)
}

pub fn remove_zone_cells(world: &mut World, cells: &[P3]) {
    let mut zones = world.res_mut::<Zones>();
    let mut map = world.res_mut::<Map>();
    for &p in cells {
        let Some(c) = map.get_mut(p) else { continue };
        if let Some(z) = c.zone.take()
            && let Some(sp) = zones.stockpiles.get_mut(&z)
        {
            sp.cells.retain(|q| *q != p);
        }
    }
    zones.stockpiles.retain(|_, s| !s.cells.is_empty());
}

pub fn add_bill(world: &mut World, bench: Entity, recipe: RecipeId, mode: BillMode) -> bool {
    let defs = defs(world);
    let Some(b) = world.get::<Building>(bench).map(|b| b.def) else { return false };
    if defs.recipes[recipe].bench != b {
        return false;
    }
    let Some(mut wb) = world.get_mut::<Workbench>(bench) else { return false };
    let id = wb.next_bill_id;
    wb.next_bill_id += 1;
    wb.bills.push(Bill { id, recipe, mode, suspended: false, done: 0 });
    true
}

pub fn remove_bill(world: &mut World, bench: Entity, bill: u32) {
    if let Some(mut wb) = world.get_mut::<Workbench>(bench) {
        wb.bills.retain(|b| b.id != bill);
    }
}

pub fn set_bill(world: &mut World, bench: Entity, bill: u32, mode: BillMode, suspended: bool) {
    if let Some(mut wb) = world.get_mut::<Workbench>(bench)
        && let Some(b) = wb.bills.iter_mut().find(|b| b.id == bill)
    {
        b.mode = mode;
        b.suspended = suspended;
    }
}

pub fn set_priority(world: &mut World, pawn: Entity, wt: WorkTypeId, prio: u8) {
    if let Some(mut wp) = world.get_mut::<WorkPriorities>(pawn) {
        if wp.0.len() <= wt.index() {
            wp.0.resize(wt.index() + 1, 3);
        }
        wp.0[wt.index()] = prio.min(4);
    }
}

/// Debug: force a pawn to drop its job and rethink.
pub fn force_rethink(world: &mut World, pawn: Entity) {
    jobs::interrupt(world, pawn, "player");
}
