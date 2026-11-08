//! Pawn decision making: needs → mental state → work by priority → idle.

use std::rc::Rc;

use ecs::{Entity, World};

use crate::components::*;
use crate::defs::*;
use crate::geom::{NEIGHBORS8, P3};
use crate::farm;
use crate::items;
use crate::jobs::{Job, JobKind, Toil, WorkTarget};
use crate::map::{Designation, Map};
use crate::path::{Goal, Pathfinder};
use crate::resources::*;
use crate::rng::SimRng;

/// Max path cost for work searches (~250 orthogonal steps).
const SEARCH_COST: u32 = 2500;

fn defs(world: &World) -> Rc<Defs> {
    world.res::<Rc<Defs>>().clone()
}

fn pos_of(world: &World, e: Entity) -> Option<P3> {
    world.get::<Position>(e).map(|p| p.0)
}

fn need_value(world: &World, pawn: Entity, role: NeedRole) -> Option<f32> {
    let defs = defs(world);
    let race = world.get::<Pawn>(pawn)?.race;
    let (id, _) = defs.needs.iter().find(|(_, n)| n.role == Some(role))?;
    crate::stats::race_need(&defs, race, id)?;
    world.get::<Needs>(pawn)?.values.get(id.index()).copied()
}

fn reserve(world: &World, pawn: Entity, keys: &[ResKey]) -> bool {
    let mut r = world.res_mut::<Reservations>();
    if keys.iter().any(|k| r.blocked(*k, pawn)) {
        return false;
    }
    for k in keys {
        r.try_reserve(*k, pawn);
    }
    true
}

/// Chooses a new job for an idle pawn.
pub fn think(world: &World, pawn: Entity) -> Job {
    let Some(pos) = pos_of(world, pawn) else { return wait_job(60) };
    if let Some(ms) = world.get_cloned::<MentalState>(pawn) {
        return mental_job(world, pawn, pos, ms.kind);
    }
    let defs = defs(world);
    let hunger = need_value(world, pawn, NeedRole::Food).unwrap_or(1.0);
    let rest = need_value(world, pawn, NeedRole::Rest).unwrap_or(1.0);
    let rec = need_value(world, pawn, NeedRole::Recreation);
    let shift: i32 = world
        .get::<Quirks>(pawn)
        .map(|q| q.0.iter().map(|q| defs.quirks[*q].sleep_shift).sum())
        .unwrap_or(0);
    let night = world.res::<Clock>().is_night_shifted(shift);

    if hunger < 0.3
        && let Some(j) = eat_job(world, pawn, pos)
    {
        return j;
    }
    if rest < 0.25 || (night && rest < 0.8) {
        return sleep_job(world, pawn, pos);
    }
    if let Some(r) = rec
        && r < 0.2
        && let Some(j) = relax_job(world, pawn, pos)
    {
        return j;
    }

    let prios = world.get_cloned::<WorkPriorities>(pawn).unwrap_or_default();
    for level in 1..=4u8 {
        for (wt, _) in defs.work_types.iter() {
            if prios.0.get(wt.index()).copied().unwrap_or(0) != level {
                continue;
            }
            if let Some(j) = give_work(world, &defs, pawn, pos, wt) {
                return j;
            }
        }
    }
    if let Some(r) = rec
        && r < 0.7
        && let Some(j) = relax_job(world, pawn, pos)
    {
        return j;
    }
    wander_job(world, pos, JobKind::Wander, 80).unwrap_or_else(|| wait_job(90))
}

fn give_work(world: &World, defs: &Defs, pawn: Entity, pos: P3, wt: WorkTypeId) -> Option<Job> {
    let core = &defs.core;
    if wt == core.wt_hauling {
        haul_job(world, pawn, pos)
    } else if wt == core.wt_mining {
        mining_job(world, pawn, pos)
    } else if wt == core.wt_construction {
        construction_job(world, defs, pawn, pos)
    } else if wt == core.wt_plants {
        plant_job(world, defs, pawn, pos)
    } else if Some(wt) == core.wt_cleaning {
        clean_job(world, defs, pawn, pos)
    } else {
        craft_job(world, defs, pawn, pos, wt)
    }
}

pub fn wait_job(ticks: u32) -> Job {
    Job::new(JobKind::Wait, vec![Toil::Wait { ticks }])
}

fn random_reachable(world: &World, pos: P3, max_cost: u32, filter: impl Fn(&Map, P3) -> bool) -> Option<P3> {
    let cells = {
        let map = world.res::<Map>();
        let mut pf = world.res_mut::<Pathfinder>();
        let mut v = pf.reachable(&map, pos, max_cost);
        v.retain(|p| *p != pos && filter(&map, *p));
        v
    };
    let i = world.res_mut::<SimRng>().index(cells.len());
    cells.get(i).copied()
}

fn wander_job(world: &World, pos: P3, kind: JobKind, cost: u32) -> Option<Job> {
    let dest = random_reachable(world, pos, cost, |_, _| true)?;
    let wait = world.res_mut::<SimRng>().range_i32(60, 240) as u32;
    Some(Job::new(kind, vec![Toil::GoTo(Goal::On(dest)), Toil::Wait { ticks: wait }]))
}

fn relax_job(world: &World, _pawn: Entity, pos: P3) -> Option<Job> {
    let dest = random_reachable(world, pos, 400, |m, p| !m.is_roofed(p))
        .or_else(|| random_reachable(world, pos, 200, |_, _| true))?;
    Some(Job::new(JobKind::Relax, vec![Toil::GoTo(Goal::On(dest)), Toil::Wait { ticks: 900 }]))
}

fn mental_job(world: &World, pawn: Entity, pos: P3, kind: BreakKind) -> Job {
    match kind {
        BreakKind::Sulking => wander_job(world, pos, JobKind::MentalBreak, 200).unwrap_or_else(|| wait_job(120)),
        BreakKind::Hiding => {
            let bed = world.read::<Bed>().iter().find(|(_, b)| b.owner == Some(pawn)).map(|(e, _)| e);
            let mut toils = Vec::new();
            if let Some(bp) = bed.and_then(|b| pos_of(world, b)) {
                toils.push(Toil::GoTo(Goal::On(bp)));
            }
            toils.push(Toil::Wait { ticks: 900 });
            let mut j = Job::new(JobKind::MentalBreak, toils);
            j.kind = JobKind::MentalBreak;
            j
        }
        BreakKind::FoodBinge => eat_job(world, pawn, pos)
            .map(|mut j| {
                j.kind = JobKind::MentalBreak;
                j
            })
            .or_else(|| wander_job(world, pos, JobKind::MentalBreak, 120))
            .unwrap_or_else(|| wait_job(120)),
    }
}

// --------------------------------------------------------------------- needs

fn eat_job(world: &World, pawn: Entity, pos: P3) -> Option<Job> {
    let defs = defs(world);
    let find = |want_meal: bool| -> Option<(Entity, P3)> {
        let map = world.res::<Map>();
        let itemst = world.read::<Item>();
        let res = world.res::<Reservations>();
        let mut pf = world.res_mut::<Pathfinder>();
        pf.search(&map, pos, SEARCH_COST, |p| {
            let e = map.cell(p).item?;
            let it = itemst.get(e)?;
            let d = &defs.items[it.def];
            if d.nutrition <= 0.0 || res.blocked(ResKey::Entity(e), pawn) {
                return None;
            }
            if want_meal && d.category != ItemCategory::Meal {
                return None;
            }
            Some((e, p))
        })
        .map(|(r, _)| r)
    };
    let (item, ipos) = find(true).or_else(|| find(false))?;
    if !reserve(world, pawn, &[ResKey::Entity(item)]) {
        return None;
    }
    let it = world.get_cloned::<Item>(item)?;
    let nut = defs.items[it.def].nutrition.max(0.01);
    let cur = need_value(world, pawn, NeedRole::Food).unwrap_or(0.0);
    let count = (((1.0 - cur) / nut).ceil() as u32).clamp(1, it.count);
    Some(Job::new(
        JobKind::Eat,
        vec![Toil::GoTo(Goal::On(ipos)), Toil::PickUp { item, count }, Toil::Eat { ticks: 120 }],
    ))
}

fn sleep_job(world: &World, pawn: Entity, pos: P3) -> Job {
    let beds: Vec<(Entity, Option<Entity>, P3)> = {
        let beds = world.read::<Bed>();
        beds.iter().filter_map(|(e, b)| pos_of(world, e).map(|p| (e, b.owner, p))).collect()
    };
    let own = beds.iter().find(|(_, o, _)| *o == Some(pawn));
    let pick = own.or_else(|| {
        beds.iter()
            .filter(|(e, o, _)| {
                (o.is_none() || o.is_some_and(|o| !world.is_alive(o)))
                    && !world.res::<Reservations>().blocked(ResKey::Entity(*e), pawn)
            })
            .min_by_key(|(_, _, p)| p.manhattan(pos))
    });
    if let Some((bed, _, bpos)) = pick.copied()
        && reserve(world, pawn, &[ResKey::Entity(bed)])
    {
        if let Some(mut b) = world.get_mut::<Bed>(bed) {
            b.owner = Some(pawn);
        }
        return Job::new(JobKind::Sleep, vec![Toil::GoTo(Goal::On(bpos)), Toil::Sleep { bed: Some(bed) }]);
    }
    Job::new(JobKind::Sleep, vec![Toil::Sleep { bed: None }])
}

// ---------------------------------------------------------------------- work

fn mining_job(world: &World, pawn: Entity, pos: P3) -> Option<Job> {
    let found = {
        let map = world.res::<Map>();
        let res = world.res::<Reservations>();
        let mut pf = world.res_mut::<Pathfinder>();
        pf.search(&map, pos, SEARCH_COST, |p| {
            for (dx, dy) in std::iter::once((0, 0)).chain(NEIGHBORS8) {
                let q = p.offset(dx, dy, 0);
                let Some(c) = map.get(q) else { continue };
                if res.blocked(ResKey::Cell(q), pawn) {
                    continue;
                }
                match c.designation {
                    Designation::Mine if c.solid.is_some() && q != p => return Some((q, false)),
                    Designation::DigStairs if map.is_solid(q.down()) => return Some((q, true)),
                    _ => {}
                }
            }
            None
        })
        .map(|(r, _)| r)
    };
    let (target, stairs) = found?;
    if !reserve(world, pawn, &[ResKey::Cell(target)]) {
        return None;
    }
    Some(if stairs {
        Job::new(JobKind::DigStairs, vec![Toil::GoTo(Goal::Near(target)), Toil::Work(WorkTarget::DigStairs(target))])
    } else {
        Job::new(JobKind::Mine, vec![Toil::GoTo(Goal::Adjacent(target)), Toil::Work(WorkTarget::Mine(target))])
    })
}

/// Max cells sown in one trip.
const SOW_BATCH: u32 = 6;

#[derive(Clone, Copy)]
enum PlantTask {
    Chop,
    Harvest,
    Cut,
}

/// Plants work: designated chop/harvest and growing-zone harvest/clearing
/// (nearest first), then sowing.
fn plant_job(world: &World, defs: &Defs, pawn: Entity, pos: P3) -> Option<Job> {
    tend_job(world, defs, pawn, pos).or_else(|| sow_job(world, defs, pawn, pos))
}

fn tend_job(world: &World, defs: &Defs, pawn: Entity, pos: P3) -> Option<Job> {
    let found = {
        let map = world.res::<Map>();
        let zones = world.res::<Zones>();
        let plants = world.read::<Plant>();
        let res = world.res::<Reservations>();
        let mut pf = world.res_mut::<Pathfinder>();
        pf.search(&map, pos, SEARCH_COST, |p| {
            let c = map.cell(p);
            let e = c.plant?;
            if res.blocked(ResKey::Entity(e), pawn) {
                return None;
            }
            let pl = plants.get(e)?;
            let ripe = farm::is_ripe(defs, pl);
            match c.designation {
                Designation::Chop => return Some((e, p, PlantTask::Chop)),
                Designation::Harvest if ripe => return Some((e, p, PlantTask::Harvest)),
                _ => {}
            }
            farm::grow_zone_at(&zones, &map, p)?;
            if ripe {
                Some((e, p, PlantTask::Harvest))
            } else if farm::is_foreign(&zones, &map, pl, p) {
                Some((e, p, PlantTask::Cut))
            } else {
                None
            }
        })
        .map(|(r, _)| r)
    };
    let (plant, ppos, task) = found?;
    if !reserve(world, pawn, &[ResKey::Entity(plant)]) {
        return None;
    }
    let (kind, target) = match task {
        PlantTask::Chop => (JobKind::Chop, WorkTarget::Chop(plant)),
        PlantTask::Harvest => (JobKind::Harvest, WorkTarget::Harvest(plant)),
        PlantTask::Cut => (JobKind::Cut, WorkTarget::Cut(plant)),
    };
    Some(Job::new(kind, vec![Toil::GoTo(Goal::Near(ppos)), Toil::Work(target)]))
}

/// Picks up seeds and sows a batch of nearby cells of one growing zone.
fn sow_job(world: &World, defs: &Defs, pawn: Entity, pos: P3) -> Option<Job> {
    // Crops some zone wants sown, with the seeds available for each.
    let mut crops: Vec<(PlantId, ItemId, u32)> = Vec::new();
    for z in world.res::<Zones>().growing.values() {
        if !z.sow || crops.iter().any(|(c, _, _)| *c == z.crop) {
            continue;
        }
        let Some(cd) = defs.plants[z.crop].crop.as_ref() else { continue };
        let n = items_available(world, pawn, cd.seed);
        if n > 0 {
            crops.push((z.crop, cd.seed, n));
        }
    }
    if crops.is_empty() {
        return None;
    }
    let (first, crop) = {
        let map = world.res::<Map>();
        let zones = world.res::<Zones>();
        let env = world.res::<crate::env::Env>();
        let res = world.res::<Reservations>();
        let mut pf = world.res_mut::<Pathfinder>();
        pf.search(&map, pos, SEARCH_COST, |p| {
            let crop = farm::sow_target(defs, &zones, &map, &env, p)?;
            (crops.iter().any(|(c, _, _)| *c == crop) && !res.blocked(ResKey::Cell(p), pawn)).then_some((p, crop))
        })
        .map(|(r, _)| r)?
    };
    let &(_, seed, avail) = crops.iter().find(|(c, _, _)| *c == crop)?;
    let want = avail.min(SOW_BATCH);
    // More cells of the same zone close to the first one.
    let mut cells = vec![first];
    {
        let map = world.res::<Map>();
        let zones = world.res::<Zones>();
        let env = world.res::<crate::env::Env>();
        let res = world.res::<Reservations>();
        let zone = map.cell(first).zone.and_then(|z| zones.grow(z));
        let mut more: Vec<P3> = zone
            .map(|z| z.cells.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| {
                *p != first
                    && p.z == first.z
                    && p.manhattan(first) <= 8
                    && !res.blocked(ResKey::Cell(*p), pawn)
                    && farm::sow_target(defs, &zones, &map, &env, *p) == Some(crop)
            })
            .collect();
        more.sort_by_key(|p| (p.manhattan(first), p.y, p.x));
        cells.extend(more.into_iter().take(want as usize - 1));
    }
    let srcs = gather(world, pawn, pos, seed, cells.len() as u32);
    if srcs.is_empty() {
        return None;
    }
    let mut keys: Vec<ResKey> = srcs.iter().map(|(e, _, _)| ResKey::Entity(*e)).collect();
    keys.extend(cells.iter().map(|p| ResKey::Cell(*p)));
    if !reserve(world, pawn, &keys) {
        return None;
    }
    let mut toils = Vec::new();
    for (e, p, n) in srcs {
        toils.push(Toil::GoTo(Goal::On(p)));
        toils.push(Toil::PickUp { item: e, count: n });
    }
    for p in cells {
        toils.push(Toil::GoTo(Goal::Near(p)));
        toils.push(Toil::Work(WorkTarget::Sow(p, crop)));
    }
    Some(Job::new(JobKind::Sow, toils))
}

/// Nearest stockpile cell (from `from`) that accepts `def` and has room.
fn find_storage(world: &World, pawn: Entity, def: ItemId, from: P3) -> Option<(P3, u32)> {
    let defs = defs(world);
    let stack = defs.items[def].stack;
    let map = world.res::<Map>();
    let zones = world.res::<Zones>();
    let itemst = world.read::<Item>();
    let res = world.res::<Reservations>();
    let mut pf = world.res_mut::<Pathfinder>();
    pf.search(&map, from, SEARCH_COST * 2, |p| {
        let c = map.cell(p);
        let z = zones.get(c.zone?)?;
        if !z.accepts(&defs, def) || res.blocked(ResKey::Cell(p), pawn) || c.ramp.is_some() {
            return None;
        }
        match c.item {
            None => Some((p, stack)),
            Some(e) => {
                let it = itemst.get(e)?;
                (it.def == def && it.count < stack).then_some((p, stack - it.count))
            }
        }
    })
    .map(|(r, _)| r)
}

fn haul_job(world: &World, pawn: Entity, pos: P3) -> Option<Job> {
    if world.res::<Zones>().stockpiles.is_empty() {
        return None;
    }
    let mut candidates: Vec<(Entity, P3)> = Vec::new();
    {
        let map = world.res::<Map>();
        let res = world.res::<Reservations>();
        let mut pf = world.res_mut::<Pathfinder>();
        pf.search::<()>(&map, pos, SEARCH_COST, |p| {
            if let Some(e) = map.cell(p).item
                && !res.blocked(ResKey::Entity(e), pawn)
                && !items::is_stored(world, e)
            {
                candidates.push((e, p));
            }
            (candidates.len() >= 8).then_some(())
        });
    }
    for (item, ipos) in candidates {
        let Some(it) = world.get_cloned::<Item>(item) else { continue };
        let Some((dest, room)) = find_storage(world, pawn, it.def, ipos) else { continue };
        if !reserve(world, pawn, &[ResKey::Entity(item), ResKey::Cell(dest)]) {
            continue;
        }
        let count = it.count.min(room);
        return Some(Job::new(
            JobKind::Haul,
            vec![
                Toil::GoTo(Goal::On(ipos)),
                Toil::PickUp { item, count },
                Toil::GoTo(Goal::On(dest)),
                Toil::Drop { cell: dest },
            ],
        ));
    }
    None
}

/// Finds unreserved stacks of `def` totalling `need`, nearest first.
fn gather(world: &World, pawn: Entity, pos: P3, def: ItemId, need: u32) -> Vec<(Entity, P3, u32)> {
    let mut out = Vec::new();
    let mut got = 0;
    let map = world.res::<Map>();
    let itemst = world.read::<Item>();
    let res = world.res::<Reservations>();
    let mut pf = world.res_mut::<Pathfinder>();
    pf.search::<()>(&map, pos, SEARCH_COST * 2, |p| {
        if let Some(e) = map.cell(p).item
            && let Some(it) = itemst.get(e)
            && it.def == def
            && !res.blocked(ResKey::Entity(e), pawn)
        {
            let take = it.count.min(need - got);
            out.push((e, p, take));
            got += take;
        }
        (got >= need).then_some(())
    });
    if got < need { Vec::new() } else { out }
}

fn roof_job(world: &World, pawn: Entity, pos: P3) -> Option<Job> {
    let found = {
        let map = world.res::<Map>();
        let res = world.res::<Reservations>();
        let mut pf = world.res_mut::<Pathfinder>();
        pf.search(&map, pos, SEARCH_COST, |p| {
            for (dx, dy) in std::iter::once((0, 0)).chain(NEIGHBORS8) {
                let q = p.offset(dx, dy, 0);
                let Some(c) = map.get(q) else { continue };
                let build = match c.designation {
                    Designation::BuildRoof if !c.roof => true,
                    Designation::RemoveRoof if c.roof => false,
                    _ => continue,
                };
                if !res.blocked(ResKey::Cell(q), pawn) {
                    return Some((q, build));
                }
            }
            None
        })
        .map(|(r, _)| r)
    };
    let (cell, build) = found?;
    if !reserve(world, pawn, &[ResKey::Cell(cell)]) {
        return None;
    }
    Some(Job::new(JobKind::Roof, vec![Toil::GoTo(Goal::Near(cell)), Toil::Work(WorkTarget::Roof(cell, build))]))
}

fn clean_job(world: &World, defs: &Defs, pawn: Entity, pos: P3) -> Option<Job> {
    let filth = defs.core.field_filth?;
    let found = {
        let map = world.res::<Map>();
        let env = world.res::<crate::env::Env>();
        let rooms = world.res::<crate::env::Rooms>();
        let res = world.res::<Reservations>();
        // Cells already cleaned this interval (change still pending).
        let pending: Vec<u32> = env
            .pending
            .iter()
            .filter_map(|op| match op {
                crate::env::EnvOp::Set(f, i, _) if *f == filth => Some(*i),
                _ => None,
            })
            .collect();
        let mut pf = world.res_mut::<Pathfinder>();
        pf.search(&map, pos, SEARCH_COST, |p| {
            let i = map.idx(p);
            (env.get(filth, i) > 0.4
                && rooms.indoor_room_at(&map, p).is_some()
                && !pending.contains(&(i as u32))
                && !res.blocked(ResKey::Cell(p), pawn))
            .then_some(p)
        })
        .map(|(r, _)| r)
    };
    let cell = found?;
    if !reserve(world, pawn, &[ResKey::Cell(cell)]) {
        return None;
    }
    Some(Job::new(JobKind::Clean, vec![Toil::GoTo(Goal::Near(cell)), Toil::Work(WorkTarget::Clean(cell))]))
}

fn construction_job(world: &World, defs: &Defs, pawn: Entity, pos: P3) -> Option<Job> {
    if let Some(j) = roof_job(world, pawn, pos) {
        return Some(j);
    }
    let mut candidates: Vec<(Entity, P3)> = Vec::new();
    {
        let map = world.res::<Map>();
        let bps = world.read::<Blueprint>();
        let res = world.res::<Reservations>();
        let mut pf = world.res_mut::<Pathfinder>();
        pf.search::<()>(&map, pos, SEARCH_COST, |p| {
            for (dx, dy) in std::iter::once((0, 0)).chain(NEIGHBORS8) {
                let q = p.offset(dx, dy, 0);
                if let Some(c) = map.get(q)
                    && let Some(b) = c.building
                    && bps.contains(b)
                    && !res.blocked(ResKey::Entity(b), pawn)
                    && !candidates.iter().any(|(e, _)| *e == b)
                {
                    candidates.push((b, q));
                }
            }
            (candidates.len() >= 10).then_some(())
        });
    }
    for (bp, bpos) in candidates {
        let Some(b) = world.get_cloned::<Blueprint>(bp) else { continue };
        let bd = &defs.buildings[b.def];
        let missing = bd.cost.iter().zip(&b.delivered).find(|(c, d)| **d < c.count).map(|(c, d)| (c.item, c.count - d));
        match missing {
            None => {
                if !reserve(world, pawn, &[ResKey::Entity(bp)]) {
                    continue;
                }
                let goal = if bd.passable() { Goal::Near(bpos) } else { Goal::Adjacent(bpos) };
                return Some(Job::new(JobKind::Build, vec![Toil::GoTo(goal), Toil::Work(WorkTarget::Build(bp))]));
            }
            Some((def, need)) => {
                let stack = defs.items[def].stack;
                let mut srcs = gather(world, pawn, pos, def, need.min(stack));
                if srcs.is_empty() {
                    // Take what there is, if anything (partial delivery).
                    let have = items_available(world, pawn, def);
                    if have == 0 {
                        continue;
                    }
                    srcs = gather(world, pawn, pos, def, have.min(need).min(stack));
                    if srcs.is_empty() {
                        continue;
                    }
                }
                let mut keys = vec![ResKey::Entity(bp)];
                keys.extend(srcs.iter().map(|(e, _, _)| ResKey::Entity(*e)));
                if !reserve(world, pawn, &keys) {
                    continue;
                }
                let mut toils = Vec::new();
                for (e, p, n) in srcs {
                    toils.push(Toil::GoTo(Goal::On(p)));
                    toils.push(Toil::PickUp { item: e, count: n });
                }
                toils.push(Toil::GoTo(Goal::Adjacent(bpos)));
                toils.push(Toil::Deliver { blueprint: bp });
                return Some(Job::new(JobKind::Deliver, toils));
            }
        }
    }
    None
}

fn items_available(world: &World, pawn: Entity, def: ItemId) -> u32 {
    let itemst = world.read::<Item>();
    let posst = world.read::<Position>();
    let res = world.res::<Reservations>();
    itemst
        .iter()
        .filter(|(e, it)| it.def == def && posst.contains(*e) && !res.blocked(ResKey::Entity(*e), pawn))
        .map(|(_, it)| it.count)
        .sum()
}

pub fn bill_satisfied(world: &World, defs: &Defs, bill: &Bill) -> bool {
    if bill.suspended {
        return true;
    }
    match bill.mode {
        BillMode::Count(n) => bill.done >= n,
        BillMode::Forever => false,
        BillMode::UntilHave(n) => {
            let r = &defs.recipes[bill.recipe];
            r.outputs.first().is_some_and(|o| items::colony_count(world, o.item) >= n)
        }
    }
}

fn craft_job(world: &World, defs: &Defs, pawn: Entity, pos: P3, wt: WorkTypeId) -> Option<Job> {
    let mut benches: Vec<(Entity, P3)> = {
        let wbs = world.read::<Workbench>();
        let res = world.res::<Reservations>();
        wbs.iter()
            .filter(|(e, wb)| {
                !res.blocked(ResKey::Entity(*e), pawn) && wb.bills.iter().any(|b| defs.recipes[b.recipe].work_type == wt)
            })
            .filter_map(|(e, _)| pos_of(world, e).map(|p| (e, p)))
            .collect()
    };
    benches.sort_by_key(|(e, p)| (p.manhattan(pos), *e));
    for (bench, bpos) in benches {
        let wb = world.get_cloned::<Workbench>(bench)?;
        for bill in &wb.bills {
            let r = &defs.recipes[bill.recipe];
            if r.work_type != wt || bill_satisfied(world, defs, bill) {
                continue;
            }
            let mut plan: Vec<(Entity, P3, u32)> = Vec::new();
            let mut ok = true;
            for inp in &r.inputs {
                let stored = wb.stored.iter().find(|(d, _)| *d == inp.item).map(|(_, n)| *n).unwrap_or(0);
                let need = inp.count.saturating_sub(stored);
                if need == 0 {
                    continue;
                }
                let srcs = gather(world, pawn, pos, inp.item, need);
                if srcs.is_empty() {
                    ok = false;
                    break;
                }
                plan.extend(srcs);
            }
            if !ok {
                continue;
            }
            let mut keys = vec![ResKey::Entity(bench)];
            keys.extend(plan.iter().map(|(e, _, _)| ResKey::Entity(*e)));
            if !reserve(world, pawn, &keys) {
                continue;
            }
            let mut toils = Vec::new();
            for (e, p, n) in plan {
                toils.push(Toil::GoTo(Goal::On(p)));
                toils.push(Toil::PickUp { item: e, count: n });
                toils.push(Toil::GoTo(Goal::Adjacent(bpos)));
                toils.push(Toil::Deposit { bench });
            }
            toils.push(Toil::GoTo(Goal::Adjacent(bpos)));
            toils.push(Toil::Work(WorkTarget::Craft { bench, bill: bill.id }));
            return Some(Job::new(JobKind::Craft, toils));
        }
    }
    None
}
