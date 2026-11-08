//! Per-tick pawn systems: needs, mood & thoughts, mental breaks, stats,
//! plant growth, and the job/AI driver.

use std::rc::Rc;

use ecs::{Entity, World};

use crate::ai;
use crate::components::*;
use crate::defs::*;
use crate::jobs::{self, JobKind};
use crate::map::Map;
use crate::resources::*;
use crate::rng::SimRng;
use crate::stats::{self, StatCache};

fn defs(world: &World) -> Rc<Defs> {
    world.res::<Rc<Defs>>().clone()
}

fn pawns(world: &World) -> Vec<Entity> {
    world.read::<Pawn>().entities().to_vec()
}

pub fn add_memory(world: &World, pawn: Entity, thought: ThoughtId) {
    let defs = defs(world);
    let td = &defs.thoughts[thought];
    if let Some(q) = td.requires_quirk
        && !world.get::<Quirks>(pawn).is_some_and(|qs| qs.0.contains(&q))
    {
        return;
    }
    if let Some(qs) = world.get::<Quirks>(pawn)
        && td.blocked_by_quirks.iter().any(|b| qs.0.contains(b))
    {
        return;
    }
    let ticks = (td.duration_hours.max(0.1) * TICKS_PER_HOUR as f32) as u32;
    if let Some(mut m) = world.get_mut::<Mood>(pawn) {
        let count = m.memories.iter().filter(|x| x.thought == thought).count() as u32;
        if count < td.stack_limit {
            m.memories.push(Memory { thought, ticks_left: ticks });
        } else if let Some(oldest) =
            m.memories.iter_mut().filter(|x| x.thought == thought).min_by_key(|x| x.ticks_left)
        {
            oldest.ticks_left = ticks;
        }
    }
}

// --------------------------------------------------------------------- systems

pub fn sys_clock(world: &mut World) {
    world.res_mut::<Clock>().tick += 1;
}

pub fn sys_positions(world: &mut World) {
    let v: Vec<(Entity, crate::geom::P3)> = {
        let pawns = world.read::<Pawn>();
        let pos = world.read::<Position>();
        pawns.entities().iter().filter_map(|e| pos.get(*e).map(|p| (*e, p.0))).collect()
    };
    world.res_mut::<PawnPositions>().0 = v;
}

const NEED_INTERVAL: u64 = 30;

pub fn sys_needs(world: &mut World) {
    if !world.res::<Clock>().tick.is_multiple_of(NEED_INTERVAL) {
        return;
    }
    let defs = defs(world);
    let dt = NEED_INTERVAL as f32 / TICKS_PER_DAY as f32;
    for pawn in pawns(world) {
        let race = world.get::<Pawn>(pawn).unwrap().race;
        let sleeping = world.get::<CurrentJob>(pawn).is_some_and(|j| j.is_sleeping());
        let deltas: Vec<(usize, f32)> = stats::with_ctx(world, pawn, |ctx| {
            let mut out = Vec::new();
            for rn in &defs.races[race].needs {
                let nd = &defs.needs[rn.need];
                let mut rate = rn.decay;
                if let Some(s) = nd.rate_stat {
                    rate *= world.get::<StatCache>(pawn).map(|c| c.get(&defs, s)).unwrap_or(1.0);
                }
                for q in ctx.quirks {
                    for (n, m) in &defs.quirks[*q].need_rates {
                        if *n == rn.need {
                            rate *= m;
                        }
                    }
                }
                if sleeping && nd.role == Some(NeedRole::Rest) {
                    rate = 0.0;
                }
                let mut d = -rate * dt;
                for (cond, r) in &nd.passive {
                    if stats::eval(cond, ctx) {
                        d += r * dt;
                    }
                }
                out.push((rn.need.index(), d));
            }
            out
        })
        .unwrap_or_default();
        if let Some(mut n) = world.get_mut::<Needs>(pawn) {
            if n.values.len() < defs.needs.len() {
                n.values.resize(defs.needs.len(), 1.0);
            }
            for (i, d) in deltas {
                n.values[i] = (n.values[i] + d).clamp(0.0, 1.0);
            }
        }
    }
}

const MOOD_INTERVAL: u64 = 60;

fn thought_applies(defs: &Defs, td: &ThoughtDef, race: RaceId, quirks: &[QuirkId]) -> bool {
    if let Some(q) = td.requires_quirk
        && !quirks.contains(&q)
    {
        return false;
    }
    if let Some(r) = td.requires_race
        && r != race
    {
        return false;
    }
    let _ = defs;
    !td.blocked_by_quirks.iter().any(|b| quirks.contains(b))
}

pub fn mood_target(defs: &Defs, m: &Mood) -> f32 {
    let mut t = 50.0;
    for id in &m.situational {
        t += defs.thoughts[*id].mood;
    }
    for mem in &m.memories {
        t += defs.thoughts[mem.thought].mood;
    }
    t.clamp(0.0, 100.0)
}

pub fn sys_mood(world: &mut World) {
    if !world.res::<Clock>().tick.is_multiple_of(MOOD_INTERVAL) {
        return;
    }
    let defs = defs(world);
    for pawn in pawns(world) {
        let race = world.get::<Pawn>(pawn).unwrap().race;
        let situational: Vec<ThoughtId> = stats::with_ctx(world, pawn, |ctx| {
            defs.thoughts
                .iter()
                .filter(|(_, td)| {
                    td.when.as_ref().is_some_and(|w| thought_applies(&defs, td, race, ctx.quirks) && stats::eval(w, ctx))
                })
                .map(|(id, _)| id)
                .collect()
        })
        .unwrap_or_default();
        if let Some(mut m) = world.get_mut::<Mood>(pawn) {
            for mem in &mut m.memories {
                mem.ticks_left = mem.ticks_left.saturating_sub(MOOD_INTERVAL as u32);
            }
            m.memories.retain(|x| x.ticks_left > 0);
            m.situational = situational;
            m.target = mood_target(&defs, &m);
            m.value += (m.target - m.value) * 0.08;
        }
    }
}

pub fn sys_breaks(world: &mut World) {
    let tick = world.res::<Clock>().tick;
    let defs = defs(world);
    for pawn in pawns(world) {
        // Tick down existing breaks.
        if let Some(mut ms) = world.get_mut::<MentalState>(pawn) {
            ms.ticks_left = ms.ticks_left.saturating_sub(1);
            if ms.ticks_left > 0 {
                continue;
            }
        }
        if world.has::<MentalState>(pawn) {
            world.remove::<MentalState>(pawn);
            jobs::interrupt(world, pawn, "recovered");
            let name = jobs::pawn_name(world, pawn);
            jobs::log_about(world, LogKind::Good, pawn, format!("{name} has regained composure."));
            if let Some(t) = defs.thoughts.find("catharsis") {
                add_memory(world, pawn, t);
            }
            continue;
        }
        // Check for new breaks once per hour, staggered per pawn.
        if !(tick + pawn.index() as u64 * 97).is_multiple_of(TICKS_PER_HOUR) {
            continue;
        }
        let mood = world.get::<Mood>(pawn).map(|m| m.value).unwrap_or(50.0);
        let thr = stats::stat(world, pawn, defs.core.stat_break_threshold);
        let (chance, kinds): (f32, &[BreakKind]) = if mood < thr - 30.0 {
            (0.5, &[BreakKind::FoodBinge, BreakKind::Hiding])
        } else if mood < thr - 15.0 {
            (0.25, &[BreakKind::Hiding, BreakKind::FoodBinge, BreakKind::Sulking])
        } else if mood < thr {
            (0.1, &[BreakKind::Sulking])
        } else {
            continue;
        };
        let (roll, pick, hours) = {
            let mut rng = world.res_mut::<SimRng>();
            (rng.f32(), rng.index(kinds.len()), rng.range_i32(3, 8))
        };
        if roll >= chance {
            continue;
        }
        let kind = kinds[pick];
        world.insert(pawn, MentalState { kind, ticks_left: hours as u32 * TICKS_PER_HOUR as u32 });
        jobs::interrupt(world, pawn, "mental break");
        let name = jobs::pawn_name(world, pawn);
        jobs::log_about(world, LogKind::Bad, pawn, format!("{name} has snapped: {}.", kind.label()));
    }
}

pub fn sys_stats(world: &mut World) {
    let tick = world.res::<Clock>().tick;
    for pawn in pawns(world) {
        if (tick + pawn.index() as u64).is_multiple_of(60) || !world.has::<StatCache>(pawn) {
            stats::refresh_stats(world, pawn);
        }
    }
}

/// Unsticks pawns, interrupts jobs for urgent needs, picks new jobs, runs jobs.
pub fn sys_jobs(world: &mut World) {
    let defs = defs(world);
    let food = defs.needs.iter().find(|(_, n)| n.role == Some(NeedRole::Food)).map(|(i, _)| i);
    let rest = defs.needs.iter().find(|(_, n)| n.role == Some(NeedRole::Rest)).map(|(i, _)| i);
    for pawn in pawns(world) {
        unstick(world, pawn);
        if let Some(job) = world.get_cloned::<CurrentJob>(pawn) {
            let needs = world.get_cloned::<Needs>(pawn).unwrap_or_default();
            let low = |n: Option<NeedId>, t: f32| n.and_then(|n| needs.values.get(n.index())).is_some_and(|v| *v < t);
            let urgent = job.0.kind.is_work() && (low(food, 0.12) || low(rest, 0.06));
            let breaking = world.has::<MentalState>(pawn) && job.0.kind != JobKind::MentalBreak;
            if urgent || breaking {
                jobs::interrupt(world, pawn, if urgent { "urgent need" } else { "mental break" });
            }
        }
        let fresh = !world.has::<CurrentJob>(pawn);
        if fresh {
            let job = ai::think(world, pawn);
            world.insert(pawn, CurrentJob(job));
        }
        jobs::tick_job(world, pawn);
        // Guard against zero-length job loops: a job that ends on the tick it
        // was chosen makes the pawn pause briefly before thinking again.
        if fresh && !world.has::<CurrentJob>(pawn) {
            world.insert(pawn, CurrentJob(ai::wait_job(20)));
        }
    }
}

fn unstick(world: &mut World, pawn: Entity) {
    let Some(p) = world.get::<Position>(pawn).map(|p| p.0) else { return };
    if world.has::<Mover>(pawn) || world.res::<Map>().is_walkable(p) {
        return;
    }
    // Find the nearest walkable cell (search across levels).
    let map = world.res::<Map>();
    let mut best: Option<(i32, crate::geom::P3)> = None;
    for r in 1..12 {
        for dz in [0, 1, -1] {
            for dy in -r..=r {
                for dx in -r..=r {
                    let q = p.offset(dx, dy, dz);
                    if map.is_walkable(q) {
                        let d = dx.abs() + dy.abs() + dz.abs() * 2;
                        if best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, q));
                        }
                    }
                }
            }
        }
        if best.is_some() {
            break;
        }
    }
    drop(map);
    if let Some((_, q)) = best {
        world.insert(pawn, Position(q));
        jobs::interrupt(world, pawn, "displaced");
    }
}
