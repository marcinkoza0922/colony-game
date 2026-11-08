//! Jobs are sequences of toils executed by a generic state machine.

use std::rc::Rc;

use ecs::{Entity, World};
use serde::{Deserialize, Serialize};

use crate::components::*;
use crate::defs::*;
use crate::env::Env;
use crate::geom::P3;
use crate::items;
use crate::map::{Designation, Map};
use crate::path::{Goal, Pathfinder};
use crate::resources::*;
use crate::stats::{self, stat};

/// Base work units for mining one cell of hardness 1 (1 unit = 1 tick at speed 1).
pub const MINE_WORK: f32 = 240.0;
pub const ROOF_WORK: f32 = 70.0;
pub const CLEAN_WORK: f32 = 50.0;

/// Pawn stepped onto `p`: apply foot-traffic contributions (e.g. filth).
fn foot_traffic(world: &World, p: P3) {
    let defs = defs(world);
    let map = world.res::<Map>();
    if !map.in_bounds(p) {
        return;
    }
    let idx = map.idx(p);
    let roofed = map.is_roofed(p);
    let mut env = world.res_mut::<Env>();
    for (fid, fd) in defs.env_fields.iter() {
        if fd.traffic != 0.0 && (!fd.traffic_indoor_only || roofed) {
            env.queue_add(fid, idx, fd.traffic);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    Mine,
    DigStairs,
    Chop,
    Harvest,
    Sow,
    Cut,
    Haul,
    Deliver,
    Build,
    Craft,
    Roof,
    Clean,
    Eat,
    Sleep,
    Relax,
    Wander,
    MentalBreak,
    Wait,
}

impl JobKind {
    pub fn is_work(self) -> bool {
        matches!(
            self,
            JobKind::Mine
                | JobKind::DigStairs
                | JobKind::Chop
                | JobKind::Harvest
                | JobKind::Sow
                | JobKind::Cut
                | JobKind::Haul
                | JobKind::Deliver
                | JobKind::Build
                | JobKind::Craft
                | JobKind::Roof
                | JobKind::Clean
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            JobKind::Mine => "Mining",
            JobKind::Roof => "Roofing",
            JobKind::Clean => "Cleaning",
            JobKind::DigStairs => "Digging stairs",
            JobKind::Chop => "Chopping",
            JobKind::Harvest => "Harvesting",
            JobKind::Sow => "Sowing",
            JobKind::Cut => "Clearing plants",
            JobKind::Haul => "Hauling",
            JobKind::Deliver => "Delivering materials",
            JobKind::Build => "Building",
            JobKind::Craft => "Crafting",
            JobKind::Eat => "Eating",
            JobKind::Sleep => "Sleeping",
            JobKind::Relax => "Relaxing",
            JobKind::Wander => "Wandering",
            JobKind::MentalBreak => "Having a breakdown",
            JobKind::Wait => "Waiting",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkTarget {
    Mine(P3),
    DigStairs(P3),
    Chop(Entity),
    Harvest(Entity),
    /// Cut down a plant that does not belong in its growing zone.
    Cut(Entity),
    /// Sow a crop on a growing-zone cell using carried seeds.
    Sow(P3, PlantId),
    Build(Entity),
    Craft { bench: Entity, bill: u32 },
    /// Build (true) or remove (false) the roof over a cell.
    Roof(P3, bool),
    Clean(P3),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Toil {
    GoTo(Goal),
    PickUp { item: Entity, count: u32 },
    /// Drop the carried stack into this (stockpile) cell.
    Drop { cell: P3 },
    Deliver { blueprint: Entity },
    Deposit { bench: Entity },
    Work(WorkTarget),
    Eat { ticks: u32 },
    Sleep { bed: Option<Entity> },
    Wait { ticks: u32 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub kind: JobKind,
    pub toils: Vec<Toil>,
    pub step: usize,
    pub ticks: u32,
    pub progress: f32,
    pub work_total: f32,
}

impl Job {
    pub fn new(kind: JobKind, toils: Vec<Toil>) -> Self {
        Self { kind, toils, step: 0, ticks: 0, progress: 0.0, work_total: 0.0 }
    }

    /// 0..1 progress of the current work toil, if any.
    pub fn work_fraction(&self) -> Option<f32> {
        match self.toils.get(self.step) {
            Some(Toil::Work(_)) if self.work_total > 0.0 => Some((self.progress / self.work_total).min(1.0)),
            _ => None,
        }
    }
}

/// Non-saved debugging info for the inspector.
#[derive(Clone, Debug, Default)]
pub struct PawnDebug {
    pub last_fail: Option<String>,
    pub jobs_done: u32,
    pub jobs_failed: u32,
}

enum ToilResult {
    Running,
    Next,
    Fail(String),
}

fn defs(world: &World) -> Rc<Defs> {
    world.res::<Rc<Defs>>().clone()
}

fn pos_of(world: &World, e: Entity) -> Option<P3> {
    world.get::<Position>(e).map(|p| p.0)
}

pub fn tick(world: &World) -> u64 {
    world.res::<Clock>().tick
}

pub fn log(world: &World, kind: LogKind, text: impl Into<String>) {
    let t = tick(world);
    world.res_mut::<GameLog>().push(t, kind, text);
}

/// Log entry about a specific entity (shown as floating text over it).
pub fn log_about(world: &World, kind: LogKind, subject: Entity, text: impl Into<String>) {
    let t = tick(world);
    world.res_mut::<GameLog>().push_about(t, kind, Some(subject), text);
}

pub fn pawn_name(world: &World, e: Entity) -> String {
    world.get::<Pawn>(e).map(|p| p.name.clone()).unwrap_or_else(|| format!("#{e}"))
}

/// Runs one tick of the pawn's current job.
pub fn tick_job(world: &mut World, pawn: Entity) {
    let Some(CurrentJob(mut job)) = world.remove::<CurrentJob>(pawn) else { return };
    let mut outcome: Option<Result<(), String>> = None;
    for _ in 0..8 {
        if job.step >= job.toils.len() {
            outcome = Some(Ok(()));
            break;
        }
        match step_toil(world, pawn, &mut job) {
            ToilResult::Running => break,
            ToilResult::Next => {
                job.step += 1;
                job.ticks = 0;
                job.progress = 0.0;
                job.work_total = 0.0;
            }
            ToilResult::Fail(r) => {
                outcome = Some(Err(r));
                break;
            }
        }
    }
    if outcome.is_none() && job.step >= job.toils.len() {
        outcome = Some(Ok(()));
    }
    match outcome {
        None => {
            world.insert(pawn, CurrentJob(job));
        }
        Some(r) => finish_job(world, pawn, &job, r.err()),
    }
}

/// Ends a job: releases reservations, stops movement, drops anything carried.
pub fn finish_job(world: &mut World, pawn: Entity, job: &Job, fail: Option<String>) {
    world.res_mut::<Reservations>().release_all(pawn);
    world.remove::<Mover>(pawn);
    world.remove::<CurrentJob>(pawn);
    let carried = world.get_mut::<Inventory>(pawn).and_then(|mut i| i.carrying.take());
    if let (Some(item), Some(p)) = (carried, pos_of(world, pawn)) {
        items::drop_item(world, item, p);
    }
    if let Some(mut dbg) = world.get_mut::<PawnDebug>(pawn) {
        match &fail {
            Some(r) => {
                dbg.jobs_failed += 1;
                dbg.last_fail = Some(format!("{}: {r}", job.kind.label()));
            }
            None => dbg.jobs_done += 1,
        }
    }
}

/// Interrupts whatever the pawn is doing.
pub fn interrupt(world: &mut World, pawn: Entity, why: &str) {
    if let Some(CurrentJob(job)) = world.get_cloned::<CurrentJob>(pawn) {
        finish_job(world, pawn, &job, Some(format!("interrupted ({why})")));
    }
}

fn step_cost(a: P3, b: P3) -> f32 {
    if a.x != b.x && a.y != b.y || a.z != b.z { 14.0 } else { 10.0 }
}

fn set_facing(world: &mut World, pawn: Entity, from: P3, to: P3) {
    if let Some(f) = Facing::from_delta(to.x - from.x, to.y - from.y) {
        world.insert(pawn, f);
    }
}

fn start_path(world: &mut World, pawn: Entity, goal: Goal) -> Result<bool, String> {
    let Some(pos) = pos_of(world, pawn) else { return Err("no position".into()) };
    let path = {
        let map = world.res::<Map>();
        let mut pf = world.res_mut::<Pathfinder>();
        pf.find_path(&map, pos, goal, 60_000)
    };
    let Some(path) = path else { return Err(format!("no path to {}", goal.target())) };
    if path.len() <= 1 {
        return Ok(true);
    }
    let sc = step_cost(path[0], path[1]);
    world.insert(pawn, Mover { from: path[0], next: 1, path, progress: 0.0, step_cost: sc });
    Ok(false)
}

fn toil_goto(world: &mut World, pawn: Entity, goal: Goal) -> ToilResult {
    let Some(pos) = pos_of(world, pawn) else { return ToilResult::Fail("no position".into()) };
    let Some(mut mover) = world.get_cloned::<Mover>(pawn) else {
        if goal.satisfied_by(pos) {
            return ToilResult::Next;
        }
        return match start_path(world, pawn, goal) {
            Ok(true) => ToilResult::Next,
            Ok(false) => ToilResult::Running,
            Err(e) => ToilResult::Fail(e),
        };
    };
    let defs = defs(world);
    let speed = stat(world, pawn, defs.core.stat_move_speed);
    mover.progress += speed * 10.0 / TICKS_PER_SECOND as f32;
    let mut cur = pos;
    while let Some(next) = mover.current_step() {
        if !world.res::<Map>().is_walkable(next) {
            // Path blocked: re-plan from here.
            world.remove::<Mover>(pawn);
            return match start_path(world, pawn, goal) {
                Ok(true) => ToilResult::Next,
                Ok(false) => ToilResult::Running,
                Err(e) => ToilResult::Fail(e),
            };
        }
        if mover.progress < mover.step_cost {
            set_facing(world, pawn, cur, next);
            break;
        }
        mover.progress -= mover.step_cost;
        cur = next;
        foot_traffic(world, next);
        mover.from = next;
        mover.next += 1;
        if let Some(n2) = mover.current_step() {
            mover.step_cost = step_cost(next, n2);
        }
    }
    world.insert(pawn, Position(cur));
    if mover.next >= mover.path.len() {
        world.remove::<Mover>(pawn);
        return if goal.satisfied_by(cur) { ToilResult::Next } else { ToilResult::Running };
    }
    world.insert(pawn, mover);
    ToilResult::Running
}

fn carried(world: &World, pawn: Entity) -> Option<Entity> {
    world.get::<Inventory>(pawn).and_then(|i| i.carrying)
}

fn toil_pickup(world: &mut World, pawn: Entity, item: Entity, count: u32) -> ToilResult {
    let Some(pos) = pos_of(world, pawn) else { return ToilResult::Fail("no position".into()) };
    let Some(ipos) = pos_of(world, item) else { return ToilResult::Fail("item gone".into()) };
    if !pos.is_adjacent8(ipos) {
        return ToilResult::Fail("not at item".into());
    }
    let Some(it) = world.get_cloned::<Item>(item) else { return ToilResult::Fail("item gone".into()) };
    let count = count.min(it.count);
    if let Some(cur) = carried(world, pawn) {
        let same = world.get::<Item>(cur).is_some_and(|c| c.def == it.def);
        if !same {
            return ToilResult::Fail("hands full".into());
        }
        // Merge into the carried stack.
        if count >= it.count {
            items::destroy_item(world, item);
        } else {
            world.get_mut::<Item>(item).unwrap().count -= count;
        }
        world.get_mut::<Item>(cur).unwrap().count += count;
        return ToilResult::Next;
    }
    let Some(taken) = items::take_from_stack(world, item, count) else { return ToilResult::Fail("nothing to take".into()) };
    world.get_mut::<Inventory>(pawn).unwrap().carrying = Some(taken);
    ToilResult::Next
}

fn toil_drop(world: &mut World, pawn: Entity, cell: P3) -> ToilResult {
    let Some(item) = carried(world, pawn) else { return ToilResult::Next };
    let pos = pos_of(world, pawn).unwrap_or(cell);
    if pos != cell {
        return ToilResult::Fail("not at drop cell".into());
    }
    world.get_mut::<Inventory>(pawn).unwrap().carrying = None;
    items::drop_item(world, item, cell);
    ToilResult::Next
}

fn toil_deliver(world: &mut World, pawn: Entity, bp: Entity) -> ToilResult {
    let Some(item) = carried(world, pawn) else { return ToilResult::Fail("nothing carried".into()) };
    let Some(mut blue) = world.get_cloned::<Blueprint>(bp) else { return ToilResult::Fail("blueprint gone".into()) };
    let it = world.get_cloned::<Item>(item).unwrap();
    let defs = defs(world);
    let bd = &defs.buildings[blue.def];
    let mut left = it.count;
    for (i, c) in bd.cost.iter().enumerate() {
        if c.item == it.def {
            let need = c.count.saturating_sub(blue.delivered[i]);
            let n = need.min(left);
            blue.delivered[i] += n;
            left -= n;
        }
    }
    world.insert(bp, blue);
    if left == 0 {
        world.get_mut::<Inventory>(pawn).unwrap().carrying = None;
        world.despawn(item);
    } else {
        world.get_mut::<Item>(item).unwrap().count = left;
    }
    ToilResult::Next
}

fn toil_deposit(world: &mut World, pawn: Entity, bench: Entity) -> ToilResult {
    let Some(item) = carried(world, pawn) else { return ToilResult::Fail("nothing carried".into()) };
    if !world.has::<Workbench>(bench) {
        return ToilResult::Fail("bench gone".into());
    }
    let it = world.get_cloned::<Item>(item).unwrap();
    {
        let mut wb = world.get_mut::<Workbench>(bench).unwrap();
        match wb.stored.iter_mut().find(|(d, _)| *d == it.def) {
            Some((_, n)) => *n += it.count,
            None => wb.stored.push((it.def, it.count)),
        }
    }
    world.get_mut::<Inventory>(pawn).unwrap().carrying = None;
    world.despawn(item);
    ToilResult::Next
}

/// Is the target still valid? Returns (total work, speed stat, skill).
fn work_params(world: &World, target: WorkTarget) -> Result<(f32, StatId, Option<SkillId>), String> {
    let defs = defs(world);
    let core = &defs.core;
    let map = world.res::<Map>();
    match target {
        WorkTarget::Roof(p, build) => {
            let c = map.get(p).ok_or("out of bounds")?;
            let want = if build { Designation::BuildRoof } else { Designation::RemoveRoof };
            if c.designation != want || c.roof == build {
                return Err("roof designation removed".into());
            }
            Ok((ROOF_WORK, core.stat_construction_speed, Some(core.skill_construction)))
        }
        WorkTarget::Clean(p) => {
            let f = core.field_filth.ok_or("no filth field")?;
            if !map.in_bounds(p) || world.res::<Env>().get(f, map.idx(p)) < 0.05 {
                return Err("already clean".into());
            }
            Ok((CLEAN_WORK, core.stat_work_speed, None))
        }
        WorkTarget::Mine(p) => {
            let c = map.get(p).ok_or("out of bounds")?;
            let t = c.solid.ok_or("already mined")?;
            if c.designation != Designation::Mine {
                return Err("designation removed".into());
            }
            Ok((MINE_WORK * defs.terrain[t].hardness, core.stat_mining_speed, Some(core.skill_mining)))
        }
        WorkTarget::DigStairs(p) => {
            let c = map.get(p).ok_or("out of bounds")?;
            if c.designation != Designation::DigStairs {
                return Err("designation removed".into());
            }
            let t = map.get(p.down()).and_then(|c| c.solid).ok_or("nothing to dig")?;
            Ok((MINE_WORK * 1.5 * defs.terrain[t].hardness, core.stat_mining_speed, Some(core.skill_mining)))
        }
        WorkTarget::Chop(e) => {
            let pl = world.get::<Plant>(e).ok_or("plant gone")?;
            let p = pos_of(world, e).ok_or("plant gone")?;
            if map.cell(p).designation != Designation::Chop {
                return Err("designation removed".into());
            }
            Ok((defs.plants[pl.def].chop_work, core.stat_plant_speed, Some(core.skill_botany)))
        }
        WorkTarget::Harvest(e) => {
            let pl = world.get::<Plant>(e).ok_or("plant gone")?;
            let p = pos_of(world, e).ok_or("plant gone")?;
            let zoned = crate::farm::grow_zone_at(&world.res::<Zones>(), &map, p).is_some();
            if map.cell(p).designation != Designation::Harvest && !zoned {
                return Err("designation removed".into());
            }
            let h = defs.plants[pl.def].harvest.as_ref().ok_or("not harvestable")?;
            if pl.growth < h.min_growth {
                return Err("not ripe".into());
            }
            Ok((h.work, core.stat_plant_speed, Some(core.skill_botany)))
        }
        WorkTarget::Cut(e) => {
            let pl = world.get::<Plant>(e).ok_or("plant gone")?;
            let p = pos_of(world, e).ok_or("plant gone")?;
            if !crate::farm::is_foreign(&world.res::<Zones>(), &map, &pl, p) {
                return Err("plant now belongs here".into());
            }
            let w = defs.plants[pl.def].chop_work.max(40.0);
            Ok((w, core.stat_plant_speed, Some(core.skill_botany)))
        }
        WorkTarget::Sow(p, crop) => {
            let want = crate::farm::sow_target(&defs, &world.res::<Zones>(), &map, &world.res::<Env>(), p);
            if want != Some(crop) {
                return Err("cell no longer needs sowing".into());
            }
            let cd = defs.plants[crop].crop.as_ref().ok_or("not a crop")?;
            Ok((cd.sow_work, core.stat_plant_speed, Some(core.skill_botany)))
        }
        WorkTarget::Build(bp) => {
            let b = world.get::<Blueprint>(bp).ok_or("blueprint gone")?;
            let bd = &defs.buildings[b.def];
            if bd.cost.iter().zip(&b.delivered).any(|(c, d)| *d < c.count) {
                return Err("materials missing".into());
            }
            Ok((bd.work.max(1.0), core.stat_construction_speed, Some(core.skill_construction)))
        }
        WorkTarget::Craft { bench, bill } => {
            let wb = world.get::<Workbench>(bench).ok_or("bench gone")?;
            let b = wb.bills.iter().find(|b| b.id == bill).ok_or("bill removed")?;
            let r = &defs.recipes[b.recipe];
            for inp in &r.inputs {
                let have = wb.stored.iter().find(|(d, _)| *d == inp.item).map(|(_, n)| *n).unwrap_or(0);
                if have < inp.count {
                    return Err("ingredients missing".into());
                }
            }
            Ok((r.work, r.speed_stat, Some(r.skill)))
        }
    }
}

fn target_cell(world: &World, t: WorkTarget) -> Option<P3> {
    match t {
        WorkTarget::Mine(p) | WorkTarget::DigStairs(p) | WorkTarget::Roof(p, _) | WorkTarget::Clean(p) => Some(p),
        WorkTarget::Sow(p, _) => Some(p),
        WorkTarget::Chop(e) | WorkTarget::Harvest(e) | WorkTarget::Cut(e) | WorkTarget::Build(e) => pos_of(world, e),
        WorkTarget::Craft { bench, .. } => pos_of(world, bench),
    }
}

pub fn gain_xp(world: &mut World, pawn: Entity, skill: SkillId, amount: f32) {
    let defs = defs(world);
    let (race, quirks) = match (world.get::<Pawn>(pawn), world.get::<Quirks>(pawn)) {
        (Some(p), Some(q)) => (p.race, q.0.clone()),
        _ => return,
    };
    let rate = amount * stats::aptitude(&defs, race, &quirks, skill) * stat(world, pawn, defs.core.stat_learn_rate);
    let mut leveled = None;
    if let Some(mut sk) = world.get_mut::<Skills>(pawn) {
        if sk.levels.len() <= skill.index() {
            sk.levels.resize(skill.index() + 1, SkillLevel::default());
        }
        let s = &mut sk.levels[skill.index()];
        if s.level < MAX_SKILL {
            s.xp += rate;
            while s.level < MAX_SKILL && s.xp >= xp_to_next(s.level) {
                s.xp -= xp_to_next(s.level);
                s.level += 1;
                leveled = Some(s.level);
            }
        }
    }
    if let Some(l) = leveled {
        let name = pawn_name(world, pawn);
        log_about(world, LogKind::Good, pawn, format!("{name} is now level {l} in {}.", defs.skills[skill].label));
    }
}

fn toil_work(world: &mut World, pawn: Entity, job: &mut Job, target: WorkTarget) -> ToilResult {
    let (total, speed_stat, skill) = match work_params(world, target) {
        Ok(v) => v,
        // A sow job covers several cells; skip one that stopped needing it.
        Err(_) if matches!(target, WorkTarget::Sow(..)) => return ToilResult::Next,
        Err(e) => return ToilResult::Fail(e),
    };
    if let (Some(tp), Some(pp)) = (target_cell(world, target), pos_of(world, pawn)) {
        if !pp.is_adjacent8(tp) && !(matches!(target, WorkTarget::DigStairs(_)) && pp == tp) {
            return ToilResult::Fail("not at work site".into());
        }
        if tp != pp {
            set_facing(world, pawn, pp, tp);
        }
    }
    let defs = defs(world);
    job.work_total = total;
    let speed = stat(world, pawn, defs.core.stat_work_speed) * stat(world, pawn, speed_stat);
    job.progress += speed.max(0.05);
    if let Some(sk) = skill {
        gain_xp(world, pawn, sk, 1.0);
    }
    if job.progress < total {
        return ToilResult::Running;
    }
    complete_work(world, pawn, target);
    ToilResult::Next
}

fn complete_work(world: &mut World, pawn: Entity, target: WorkTarget) {
    let defs = defs(world);
    match target {
        WorkTarget::Roof(p, build) => {
            let mut map = world.res_mut::<Map>();
            map.set_roof(p, build);
            map.cell_mut(p).designation = Designation::None;
        }
        WorkTarget::Clean(p) => {
            if let Some(f) = defs.core.field_filth {
                let idx = world.res::<Map>().idx(p);
                world.res_mut::<Env>().queue_set(f, idx, 0.0);
            }
        }
        WorkTarget::Mine(p) => {
            let t = world.res::<Map>().cell(p).solid;
            {
                let mut map = world.res_mut::<Map>();
                map.set_solid(p, None);
                map.cell_mut(p).designation = Designation::None;
                map.fix_ramps_around(p);
            }
            if let Some(t) = t {
                drop_terrain_yield(world, &defs, t, p);
            }
            settle_above(world, p);
        }
        WorkTarget::DigStairs(p) => {
            let below = p.down();
            let t = world.res::<Map>().cell(below).solid;
            {
                let mut map = world.res_mut::<Map>();
                map.set_solid(below, None);
                map.cell_mut(p).designation = Designation::None;
                map.fix_ramps_around(below);
            }
            let stairs = defs.core.stairs;
            spawn_building(world, stairs, 0, below);
            if let Some(t) = t {
                drop_terrain_yield(world, &defs, t, p);
            }
        }
        WorkTarget::Chop(e) | WorkTarget::Cut(e) => {
            let Some(pl) = world.get_cloned::<Plant>(e) else { return };
            let p = pos_of(world, e).unwrap();
            let pd = &defs.plants[pl.def];
            for y in &pd.chop_yield {
                let n = ((y.count as f32) * pl.growth).round().max(1.0) as u32;
                items::spawn_items(world, y.item, n, p);
            }
            remove_plant(world, e);
            if matches!(target, WorkTarget::Chop(_)) {
                world.res_mut::<Map>().cell_mut(p).designation = Designation::None;
            }
        }
        WorkTarget::Harvest(e) => {
            let Some(pl) = world.get_cloned::<Plant>(e) else { return };
            let p = pos_of(world, e).unwrap();
            let pd = &defs.plants[pl.def];
            if let Some(h) = &pd.harvest {
                // Yield scales with plant health.
                for y in &h.yields {
                    let n = (y.count as f32 * pl.health.clamp(0.0, 1.0)).round() as u32;
                    if n > 0 {
                        items::spawn_items(world, y.item, n, p);
                    }
                }
                if h.remove {
                    remove_plant(world, e);
                } else {
                    world.get_mut::<Plant>(e).unwrap().growth = h.regrow_to;
                }
            }
            world.res_mut::<Map>().cell_mut(p).designation = Designation::None;
        }
        WorkTarget::Sow(p, crop) => {
            // Use one carried seed.
            let Some(seed) = defs.plants[crop].crop.as_ref().map(|c| c.seed) else { return };
            let Some(item) = carried(world, pawn) else { return };
            let ok = world.get::<Item>(item).is_some_and(|it| it.def == seed && it.count > 0);
            if !ok {
                return;
            }
            let left = {
                let mut it = world.get_mut::<Item>(item).unwrap();
                it.count -= 1;
                it.count
            };
            if left == 0 {
                world.get_mut::<Inventory>(pawn).unwrap().carrying = None;
                world.despawn(item);
            }
            let e = world.spawn((Plant::new(crop, 0.0), Position(p)));
            world.res_mut::<Map>().cell_mut(p).plant = Some(e);
        }
        WorkTarget::Build(bp) => {
            let Some(b) = world.get_cloned::<Blueprint>(bp) else { return };
            let p = pos_of(world, bp).unwrap();
            world.despawn(bp);
            world.res_mut::<Map>().set_building(p, None, None);
            spawn_building(world, b.def, b.rot, p);
            let name = pawn_name(world, pawn);
            log_about(world, LogKind::Info, pawn, format!("{name} built a {}.", defs.buildings[b.def].label));
        }
        WorkTarget::Craft { bench, bill } => {
            let Some(mut wb) = world.get_cloned::<Workbench>(bench) else { return };
            let Some(bi) = wb.bills.iter().position(|b| b.id == bill) else { return };
            let recipe = &defs.recipes[wb.bills[bi].recipe];
            for inp in &recipe.inputs {
                if let Some((_, n)) = wb.stored.iter_mut().find(|(d, _)| *d == inp.item) {
                    *n -= inp.count.min(*n);
                }
            }
            wb.stored.retain(|(_, n)| *n > 0);
            wb.bills[bi].done += 1;
            if let BillMode::Count(n) = wb.bills[bi].mode
                && wb.bills[bi].done >= n
            {
                wb.bills.remove(bi);
            }
            world.insert(bench, wb);
            let p = pos_of(world, bench).unwrap();
            for o in &recipe.outputs {
                items::spawn_items(world, o.item, o.count, p);
            }
        }
    }
}

fn drop_terrain_yield(world: &mut World, defs: &Defs, t: TerrainId, p: P3) {
    let td = &defs.terrain[t];
    if td.drops.is_empty() {
        return;
    }
    let roll = world.res_mut::<crate::rng::SimRng>().f32();
    if roll < td.drop_chance {
        for d in &td.drops {
            items::spawn_items(world, d.item, d.count, p);
        }
    }
}

/// After a cell is opened, things resting above it lose support: move items
/// and plants down / remove them.
fn settle_above(world: &mut World, p: P3) {
    let up = p.up();
    let (item, plant, building) = match world.res::<Map>().get(up) {
        Some(c) => (c.item, c.plant, c.building),
        None => return,
    };
    if world.res::<Map>().is_supported(up) {
        return;
    }
    if let Some(it) = item {
        items::lift_item(world, it);
        items::drop_item(world, it, p);
    }
    if let Some(pl) = plant {
        remove_plant(world, pl);
    }
    if let Some(b) = building {
        world.res_mut::<Map>().set_building(up, None, None);
        world.despawn(b);
    }
}

pub fn remove_plant(world: &mut World, e: Entity) {
    if let Some(Position(p)) = world.get_cloned::<Position>(e) {
        let mut map = world.res_mut::<Map>();
        if map.cell(p).plant == Some(e) {
            map.cell_mut(p).plant = None;
        }
    }
    world.despawn(e);
}

/// Spawns a finished building at `p`, clearing plants and displacing items.
pub fn spawn_building(world: &mut World, def: BuildingId, rot: u8, p: P3) -> Entity {
    let defs = defs(world);
    let bd = &defs.buildings[def];
    let (plant, item) = {
        let c = world.res::<Map>().cell(p).clone();
        (c.plant, c.item)
    };
    if let Some(pl) = plant {
        remove_plant(world, pl);
    }
    let e = world.spawn((Building { def, rot }, Position(p)));
    match bd.kind {
        BuildingKind::Workbench => {
            world.insert(e, Workbench::default());
        }
        BuildingKind::Bed => {
            world.insert(e, Bed::default());
        }
        _ => {}
    }
    world.res_mut::<Map>().set_building(p, Some(e), Some(bd.kind));
    if !bd.passable()
        && let Some(it) = item
    {
        items::lift_item(world, it);
        items::drop_item(world, it, p);
    }
    e
}

fn toil_eat(world: &mut World, pawn: Entity, job: &mut Job, ticks: u32) -> ToilResult {
    let Some(item) = carried(world, pawn) else { return ToilResult::Fail("no food".into()) };
    job.ticks += 1;
    if job.ticks < ticks {
        return ToilResult::Running;
    }
    let defs = defs(world);
    let it = world.get_cloned::<Item>(item).unwrap();
    let idef = &defs.items[it.def];
    let Some(food_need) = defs.needs.iter().find(|(_, n)| n.role == Some(NeedRole::Food)).map(|(id, _)| id) else {
        return ToilResult::Next;
    };
    let cur = world.get::<Needs>(pawn).and_then(|n| n.values.get(food_need.index()).copied()).unwrap_or(1.0);
    let want = ((1.0 - cur) / idef.nutrition.max(0.01)).ceil().max(1.0) as u32;
    let eaten = want.min(it.count);
    if let Some(mut n) = world.get_mut::<Needs>(pawn)
        && let Some(v) = n.values.get_mut(food_need.index())
    {
        *v = (*v + idef.nutrition * eaten as f32).min(1.0);
    }
    if eaten >= it.count {
        world.get_mut::<Inventory>(pawn).unwrap().carrying = None;
        world.despawn(item);
    } else {
        world.get_mut::<Item>(item).unwrap().count -= eaten;
    }
    if let Some(t) = idef.eat_thought {
        crate::pawn_sys::add_memory(world, pawn, t);
    }
    ToilResult::Next
}

fn need_by_role(defs: &Defs, role: NeedRole) -> Option<NeedId> {
    defs.needs.iter().find(|(_, n)| n.role == Some(role)).map(|(id, _)| id)
}

/// Rest restored per day of sleep at rest_factor 1.
const REST_GAIN_PER_DAY: f32 = 3.2;
/// Recreation restored per day of relaxing.
const RECREATION_GAIN_PER_DAY: f32 = 6.0;

fn toil_sleep(world: &mut World, pawn: Entity, job: &mut Job, bed: Option<Entity>) -> ToilResult {
    let defs = defs(world);
    let Some(rest) = need_by_role(&defs, NeedRole::Rest) else { return ToilResult::Next };
    if job.ticks == 0 {
        if let Some(b) = bed {
            let Some(bp) = pos_of(world, b) else { return ToilResult::Fail("bed gone".into()) };
            if pos_of(world, pawn) != Some(bp) {
                return ToilResult::Fail("not in bed".into());
            }
            // Lie along the bed (head toward the headboard).
            if let Some(r) = world.get::<Building>(b).map(|bb| bb.rot) {
                world.insert(pawn, Facing(r * 2));
            }
        } else {
            crate::pawn_sys::add_memory(world, pawn, defs.core.thought_slept_ground);
        }
    }
    job.ticks += 1;
    let factor = bed
        .and_then(|b| world.get::<Building>(b).map(|bb| defs.buildings[bb.def].rest_factor))
        .unwrap_or(0.75);
    let shift: i32 = world
        .get::<Quirks>(pawn)
        .map(|q| q.0.iter().map(|q| defs.quirks[*q].sleep_shift).sum())
        .unwrap_or(0);
    let night = world.res::<Clock>().is_night_shifted(shift);
    let mut done = false;
    if let Some(mut n) = world.get_mut::<Needs>(pawn) {
        let v = &mut n.values[rest.index()];
        *v = (*v + REST_GAIN_PER_DAY * factor / TICKS_PER_DAY as f32).min(1.0);
        // Sleep through the night unless fully rested; by day wake when topped up.
        if *v >= 1.0 || (!night && *v > 0.95) {
            done = true;
        }
        // Hunger wakes the sleeper, but only checked periodically so a starving
        // pawn with no food doesn't thrash between waking and sleeping.
        if let Some(food) = need_by_role(&defs, NeedRole::Food)
            && n.values[food.index()] < 0.08
            && job.ticks.is_multiple_of(600)
        {
            done = true;
        }
    }
    if done {
        if let Some(b) = bed
            && let Some(t) = world.get::<Building>(b).and_then(|bb| defs.buildings[bb.def].sleep_thought)
        {
            crate::pawn_sys::add_memory(world, pawn, t);
        }
        return ToilResult::Next;
    }
    ToilResult::Running
}

fn toil_wait(world: &mut World, pawn: Entity, job: &mut Job, ticks: u32) -> ToilResult {
    job.ticks += 1;
    if job.kind == JobKind::Relax {
        let defs = defs(world);
        if let Some(rec) = need_by_role(&defs, NeedRole::Recreation)
            && let Some(mut n) = world.get_mut::<Needs>(pawn)
            && let Some(v) = n.values.get_mut(rec.index())
        {
            *v = (*v + RECREATION_GAIN_PER_DAY / TICKS_PER_DAY as f32).min(1.0);
        }
    }
    if job.ticks >= ticks { ToilResult::Next } else { ToilResult::Running }
}

fn step_toil(world: &mut World, pawn: Entity, job: &mut Job) -> ToilResult {
    let toil = job.toils[job.step].clone();
    match toil {
        Toil::GoTo(goal) => toil_goto(world, pawn, goal),
        Toil::PickUp { item, count } => toil_pickup(world, pawn, item, count),
        Toil::Drop { cell } => toil_drop(world, pawn, cell),
        Toil::Deliver { blueprint } => toil_deliver(world, pawn, blueprint),
        Toil::Deposit { bench } => toil_deposit(world, pawn, bench),
        Toil::Work(t) => toil_work(world, pawn, job, t),
        Toil::Eat { ticks } => toil_eat(world, pawn, job, ticks),
        Toil::Sleep { bed } => toil_sleep(world, pawn, job, bed),
        Toil::Wait { ticks } => toil_wait(world, pawn, job, ticks),
    }
}

impl CurrentJob {
    /// Actually asleep (on the sleep toil), not just walking to bed.
    pub fn is_sleeping(&self) -> bool {
        self.0.kind == JobKind::Sleep && matches!(self.0.toils.get(self.0.step), Some(Toil::Sleep { .. }))
    }
}
