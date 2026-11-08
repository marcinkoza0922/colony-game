//! Scripted starting orders, shared by the headless runner and tests so we can
//! exercise the full gameplay loop without a UI.

use ecs::Entity;

use crate::components::{BillMode, Building, Plant, Position};
use crate::geom::P3;
use crate::map::Map;
use crate::orders::{self, DesignateTool};
use crate::resources::SimMeta;
use crate::Sim;

/// Gives the colony a sensible set of orders around the landing site:
/// a stockpile, tree chopping, berry harvesting, a campfire with a cooking
/// bill, beds, a stonecutter, an ashroot field, and some mining if rock is
/// nearby.
pub fn basic_orders(sim: &mut Sim) {
    let defs = sim.defs();
    let w = &mut sim.world;
    let start = w.res::<SimMeta>().start;

    let stock: Vec<P3> = (-3..=3).flat_map(|dy| (4..=9).map(move |dx| start.offset(dx, dy, 0))).collect();
    orders::create_stockpile(w, &stock);

    let mut chop = Vec::new();
    let mut harvest = Vec::new();
    {
        let plants = w.read::<Plant>();
        let pos = w.read::<Position>();
        for (e, pl) in plants.iter() {
            let Some(p) = pos.get(e).map(|p| p.0) else { continue };
            if p.chebyshev(start) > 22 {
                continue;
            }
            let pd = &defs.plants[pl.def];
            if pd.tree {
                chop.push(p);
            } else if pd.harvest.is_some() {
                harvest.push(p);
            }
        }
    }
    chop.truncate(12);
    orders::designate(w, &chop, DesignateTool::Chop);
    orders::designate(w, &harvest, DesignateTool::Harvest);

    let b = |n: &str| defs.buildings.expect(n);
    let mut plan: Vec<(P3, &str)> = vec![
        (start.offset(-3, -2, 0), "campfire"),
        (start.offset(-1, -3, 0), "stonecutter"),
        (start.offset(1, -3, 0), "workbench"),
    ];
    // A walled, roofed barracks (7x5 incl. walls) on the nearest flat site:
    // door in the north wall, three beds and a torch inside.
    let site = find_flat_site(w, start, 7, 5);
    let mut roof = Vec::new();
    if let Some(o) = site {
        for x in 0..7 {
            for y in 0..5 {
                let p = o.offset(x, y, 0);
                let edge = x == 0 || x == 6 || y == 0 || y == 4;
                if edge {
                    plan.push((p, if x == 3 && y == 0 { "door" } else { "wood_wall" }));
                } else {
                    roof.push(p);
                }
            }
        }
        plan.extend([(o.offset(1, 3, 0), "bed"), (o.offset(3, 3, 0), "bed"), (o.offset(5, 3, 0), "bed")]);
        plan.push((o.offset(5, 1, 0), "torch"));
    }
    for (p, name) in plan {
        if orders::place_blueprint(w, p, b(name), 0).is_none() {
            eprintln!("scenario: could not place {name} at {p}");
        }
    }
    orders::designate(w, &roof, DesignateTool::BuildRoof);

    // A small ashroot field.
    if let Some(o) = find_flat_site(w, start, 6, 4) {
        let field: Vec<P3> = (0..4).flat_map(|y| (0..6).map(move |x| o.offset(x, y, 0))).collect();
        if let Some(z) = orders::create_grow_zone(w, &field) {
            orders::set_grow_zone(w, z, defs.plants.expect("ashroot"), true);
        }
    }

    // Mine any solid cells at colony level within range.
    let mine: Vec<P3> = {
        let map = w.res::<Map>();
        let mut v = Vec::new();
        for dy in -20..=20 {
            for dx in -20..=20 {
                let p = start.offset(dx, dy, 0);
                if map.is_solid(p) {
                    v.push(p);
                }
            }
        }
        v.truncate(20);
        v
    };
    orders::designate(w, &mine, DesignateTool::Mine);
}

/// Nearest w x h rectangle (top-left returned) of plain walkable ground with
/// no ramps, zones, buildings or items, avoiding the landing site itself.
fn find_flat_site(world: &ecs::World, start: P3, w: i32, h: i32) -> Option<P3> {
    let map = world.res::<Map>();
    let mut best: Option<(i32, P3)> = None;
    for dy in -18..=18 {
        for dx in -18..=18 {
            let o = start.offset(dx, dy, 0);
            // Keep clear of the camp core (fire, benches, stockpile, supplies).
            if (o.x - 1 <= start.x + 9) && (o.x + w >= start.x - 4) && (o.y - 1 <= start.y + 3) && (o.y + h >= start.y - 4) {
                continue;
            }
            let ok = (0..w).all(|x| {
                (0..h).all(|y| {
                    let p = o.offset(x, y, 0);
                    map.is_walkable(p) && map.get(p).is_some_and(|c| c.ramp.is_none() && c.zone.is_none() && c.building.is_none() && c.item.is_none())
                })
            });
            if ok {
                let d = dx.abs() + dy.abs();
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, o));
                }
            }
        }
    }
    best.map(|(_, o)| o)
}

/// Adds bills to benches once they exist (call periodically).
pub fn ensure_bills(sim: &mut Sim) {
    let defs = sim.defs();
    let w = &mut sim.world;
    let benches: Vec<(Entity, crate::defs::BuildingId)> =
        w.read::<Building>().iter().map(|(e, b)| (e, b.def)).collect();
    for (e, def) in benches {
        let has_bills = w.get::<crate::components::Workbench>(e).is_some_and(|wb| !wb.bills.is_empty());
        if has_bills {
            continue;
        }
        for (rid, r) in defs.recipes.iter() {
            if r.bench == def {
                let mode = if r.name == "cook_simple_meal" { BillMode::UntilHave(15) } else { BillMode::Count(5) };
                orders::add_bill(w, e, rid, mode);
                break;
            }
        }
    }
}

/// What an attentive player would do periodically: harvest ripe bushes near
/// the colony.
pub fn periodic_orders(sim: &mut Sim) {
    let defs = sim.defs();
    let w = &mut sim.world;
    let start = w.res::<SimMeta>().start;
    let ripe: Vec<P3> = {
        let plants = w.read::<Plant>();
        let pos = w.read::<Position>();
        plants
            .iter()
            .filter(|(_, pl)| defs.plants[pl.def].harvest.as_ref().is_some_and(|h| pl.growth >= h.min_growth))
            .filter_map(|(e, _)| pos.get(e).map(|p| p.0))
            .filter(|p| p.chebyshev(start) <= 30)
            .collect()
    };
    // Growing zones are harvested without designations.
    let ripe: Vec<P3> = {
        let map = w.res::<Map>();
        ripe.into_iter().filter(|p| map.cell(*p).zone.is_none_or(|z| w.res::<crate::resources::Zones>().grow(z).is_none())).collect()
    };
    orders::designate(w, &ripe, DesignateTool::Harvest);
}
