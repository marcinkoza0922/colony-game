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
/// bill, beds, a stonecutter, and some mining if rock is nearby.
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
    let plan: Vec<(P3, &str)> = std::iter::once((start.offset(-3, -2, 0), "campfire"))
        .chain((0..3).map(|i| (start.offset(-3 + i * 2, 3, 0), "bed")))
        .chain(std::iter::once((start.offset(-1, -3, 0), "stonecutter")))
        .chain(std::iter::once((start.offset(1, -3, 0), "workbench")))
        .chain((-3..=3).map(|x| (start.offset(x, -5, 0), "wood_wall")))
        .collect();
    for (p, name) in plan {
        if orders::place_blueprint(w, p, b(name), 0).is_none() {
            eprintln!("scenario: could not place {name} at {p}");
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
    orders::designate(w, &ripe, DesignateTool::Harvest);
}
