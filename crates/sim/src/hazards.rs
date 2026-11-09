//! Where M2 injuries and illness come from: work accidents and cave-ins,
//! random illness, and food poisoning. (Exposure is data-driven through
//! hediff triggers; fights live in `social`.)

use std::rc::Rc;

use ecs::{Entity, World};

use crate::components::*;
use crate::defs::*;
use crate::health::{self, DamageInfo};
use crate::jobs::{self, WorkTarget};
use crate::map::Map;
use crate::resources::*;
use crate::rng::SimRng;
use crate::stats;

/// Ticks between accident rolls while working.
const ACCIDENT_INTERVAL: u64 = 250;

fn defs(world: &World) -> Rc<Defs> {
    world.res::<Rc<Defs>>().clone()
}

/// Accident odds per roll and the damage types it can cause.
fn accident_profile(defs: &Defs, world: &World, target: WorkTarget) -> Option<(f32, &'static [&'static str], Option<SkillId>)> {
    let core = &defs.core;
    Some(match target {
        WorkTarget::Mine(_) | WorkTarget::DigStairs(_) => (0.004, &["cut", "crush"], Some(core.skill_mining)),
        WorkTarget::Build(_) | WorkTarget::Roof(..) => (0.003, &["crush", "cut"], Some(core.skill_construction)),
        WorkTarget::Chop(_) | WorkTarget::Cut(_) => (0.002, &["cut"], Some(core.skill_botany)),
        WorkTarget::Craft { bench, .. } => {
            let fire = world.get::<Building>(bench).is_some_and(|b| defs.buildings[b.def].light.is_some());
            let skill = world
                .get::<Workbench>(bench)
                .and_then(|wb| wb.bills.first().map(|b| defs.recipes[b.recipe].skill));
            (0.003, if fire { &["burn", "cut"] } else { &["cut", "crush"] }, skill)
        }
        _ => return None,
    })
}

/// Rolls for an accident during work. Err ends the job (cave-in).
pub fn work_accident(world: &mut World, pawn: Entity, target: WorkTarget) -> Result<(), String> {
    let tick = world.res::<Clock>().tick;
    if !(tick + pawn.index() as u64 * 13).is_multiple_of(ACCIDENT_INTERVAL) {
        return Ok(());
    }
    let defs = defs(world);
    if matches!(target, WorkTarget::Mine(_)) && cave_in(world, &defs, pawn, target)? {
        return Err("cave-in".into());
    }
    let Some((base, kinds, skill)) = accident_profile(&defs, world, target) else { return Ok(()) };
    let lvl = skill
        .and_then(|s| world.get::<Skills>(pawn).map(|sk| sk.levels.get(s.index()).map_or(0, |l| l.level)))
        .unwrap_or(0);
    let mult = stats::stat_by_name(world, pawn, "accident_chance").unwrap_or(1.0);
    let chance = base * mult * (1.5 - lvl as f32 * 0.06).clamp(0.3, 1.5);
    let (hit, pick, amount) = {
        let mut rng = world.res_mut::<SimRng>();
        (rng.chance(chance), rng.index(kinds.len()), rng.range_f32(2.0, 9.0))
    };
    if !hit {
        return Ok(());
    }
    let Some(dt) = defs.damage_types.find(kinds[pick]) else { return Ok(()) };
    let Some(r) = health::apply_damage(world, pawn, &DamageInfo { dtype: dt, amount, part: None, non_lethal: false, surface: true })
    else {
        return Ok(());
    };
    if world.has::<Pawn>(pawn)
        && let Some(body) = health::body_of(&defs, world, pawn)
    {
        let name = jobs::pawn_name(world, pawn);
        let what = &defs.damage_types[dt].label;
        jobs::log_about(world, LogKind::Bad, pawn, format!("Accident! {name} suffered a {what} to the {}.", body.parts[r.part].label));
    }
    Ok(())
}

/// Rare collapse while mining under rock: heavy crush damage.
fn cave_in(world: &mut World, defs: &Defs, pawn: Entity, target: WorkTarget) -> Result<bool, String> {
    let WorkTarget::Mine(cell) = target else { return Ok(false) };
    let Some(pp) = world.get::<Position>(pawn).map(|p| p.0) else { return Ok(false) };
    let under_rock = {
        let map = world.res::<Map>();
        map.is_underground(pp) && map.is_solid(cell.up())
    };
    if !under_rock || !world.res_mut::<SimRng>().chance(0.0006) {
        return Ok(false);
    }
    let Some(dt) = defs.damage_types.find("crush") else { return Ok(false) };
    let hits = world.res_mut::<SimRng>().range_i32(2, 4);
    let name = jobs::pawn_name(world, pawn);
    jobs::log_about(world, LogKind::Bad, pawn, format!("Cave-in! Rocks fell on {name}."));
    for _ in 0..hits {
        if !world.has::<Pawn>(pawn) {
            break;
        }
        let amount = world.res_mut::<SimRng>().range_f32(4.0, 12.0);
        health::apply_damage(world, pawn, &DamageInfo { dtype: dt, amount, part: None, non_lethal: false, surface: false });
    }
    Ok(true)
}

/// Base illness chance per colonist per day.
const ILLNESS_PER_DAY: f32 = 0.02;

/// Random illness: rolled hourly; filthy surroundings raise the odds.
pub fn sys_hazards(world: &mut World) {
    let tick = world.res::<Clock>().tick;
    if !tick.is_multiple_of(TICKS_PER_HOUR) {
        return;
    }
    let defs = defs(world);
    let pool: Vec<(HediffId, f32)> =
        defs.hediffs.iter().filter(|(_, d)| d.event_weight > 0.0).map(|(id, d)| (id, d.event_weight)).collect();
    if pool.is_empty() {
        return;
    }
    let pawns: Vec<Entity> = world.read::<health::Health>().entities().to_vec();
    for p in pawns {
        let Some(pos) = world.get::<Position>(p).map(|x| x.0) else { continue };
        let filth = health::filth_at(world, pos);
        let chance = ILLNESS_PER_DAY / 24.0 * (1.0 + filth.min(4.0) * 0.5);
        let (hit, pick) = {
            let mut rng = world.res_mut::<SimRng>();
            let w: Vec<f32> = pool.iter().map(|(_, w)| *w).collect();
            (rng.chance(chance), rng.weighted(&w))
        };
        let Some(i) = pick.filter(|_| hit) else { continue };
        let (h, _) = pool[i];
        if world.get::<health::Health>(p).is_some_and(|x| x.has(h)) {
            continue;
        }
        health::add_hediff(world, p, h, None, None);
        let name = jobs::pawn_name(world, p);
        jobs::log_about(world, LogKind::Bad, p, format!("{name} has fallen ill: {}.", defs.hediffs[h].label.to_lowercase()));
    }
}

/// Food poisoning roll after eating `count` portions of `item`.
pub fn after_eating(world: &mut World, pawn: Entity, item: ItemId, count: u32) {
    let defs = defs(world);
    let base = defs.items[item].poison_chance;
    if base <= 0.0 {
        return;
    }
    let Some(fp) = defs.hediffs.find("food_poisoning") else { return };
    let pos = world.get::<Position>(pawn).map(|p| p.0);
    let filth = pos.map(|p| health::filth_at(world, p)).unwrap_or(0.0);
    let mult = stats::stat_by_name(world, pawn, "food_poison_chance").unwrap_or(1.0);
    let chance = (base * mult * (1.0 + filth.min(4.0) * 0.5)) * (count as f32).sqrt().min(2.0);
    if !world.res_mut::<SimRng>().chance(chance) {
        return;
    }
    health::add_hediff(world, pawn, fp, None, None);
    let name = jobs::pawn_name(world, pawn);
    jobs::log_about(world, LogKind::Bad, pawn, format!("{name} has food poisoning."));
}
