//! Stats, modifiers and the condition evaluator used by quirks, needs and
//! thoughts.

use ecs::{Entity, World};
use serde::{Deserialize, Serialize};

use crate::components::*;
use crate::defs::*;
use crate::geom::P3;
use crate::jobs::JobKind;
use crate::map::Map;
use crate::resources::{Clock, PawnPositions};

/// Cached stat values per pawn (indexed by `StatId`), refreshed periodically.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StatCache(pub Vec<f32>);

impl StatCache {
    pub fn get(&self, defs: &Defs, s: StatId) -> f32 {
        self.0.get(s.index()).copied().unwrap_or(defs.stats[s].default)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Working,
    Sleeping,
    Other,
}

/// Everything a condition might need to know about one pawn.
pub struct Ctx<'a> {
    pub defs: &'a Defs,
    pub map: &'a Map,
    pub clock: &'a Clock,
    pub others: &'a [(Entity, P3)],
    pub me: Entity,
    pub pos: P3,
    pub race: RaceId,
    pub quirks: &'a [QuirkId],
    pub needs: &'a [f32],
    pub mood: f32,
    pub activity: Activity,
}

impl Ctx<'_> {
    pub fn sleep_shift(&self) -> i32 {
        self.quirks.iter().map(|q| self.defs.quirks[*q].sleep_shift).sum()
    }
}

fn count_others(ctx: &Ctx, r: i32) -> usize {
    ctx.others
        .iter()
        .filter(|(e, p)| *e != ctx.me && p.z == ctx.pos.z && (p.x - ctx.pos.x).abs() <= r && (p.y - ctx.pos.y).abs() <= r)
        .count()
}

fn count_plants(ctx: &Ctx, r: i32) -> usize {
    let mut n = 0;
    for dy in -r..=r {
        for dx in -r..=r {
            if let Some(c) = ctx.map.get(ctx.pos.offset(dx, dy, 0))
                && c.plant.is_some()
            {
                n += 1;
            }
        }
    }
    n
}

pub fn eval(c: &Condition, ctx: &Ctx) -> bool {
    match c {
        Condition::Always => true,
        Condition::Underground => ctx.map.is_roofed(ctx.pos),
        Condition::Outdoors => !ctx.map.is_roofed(ctx.pos),
        Condition::Night => ctx.clock.is_night_shifted(ctx.sleep_shift()),
        Condition::Day => !ctx.clock.is_night_shifted(ctx.sleep_shift()),
        Condition::Alone => count_others(ctx, 8) == 0,
        Condition::NearOthers(r) => count_others(ctx, *r as i32) > 0,
        Condition::NearPlants(r) => count_plants(ctx, *r as i32) >= 3,
        Condition::NeedBelow(n, v) => ctx.needs.get(n.index()).is_some_and(|x| x < v),
        Condition::NeedAbove(n, v) => ctx.needs.get(n.index()).is_some_and(|x| x > v),
        Condition::MoodBelow(v) => ctx.mood < *v,
        Condition::MoodAbove(v) => ctx.mood > *v,
        Condition::Working => ctx.activity == Activity::Working,
        Condition::Sleeping => ctx.activity == Activity::Sleeping,
        Condition::HasQuirk(q) => ctx.quirks.contains(q),
        Condition::IsRace(r) => ctx.race == *r,
        Condition::All(cs) => cs.iter().all(|c| eval(c, ctx)),
        Condition::Any(cs) => cs.iter().any(|c| eval(c, ctx)),
        Condition::Not(c) => !eval(c, ctx),
    }
}

/// Which race needs apply, with decay rate.
pub fn race_need(defs: &Defs, race: RaceId, need: NeedId) -> Option<f32> {
    defs.races[race].needs.iter().find(|n| n.need == need).map(|n| n.decay)
}

pub fn has_need(defs: &Defs, race: RaceId, need: NeedId) -> bool {
    race_need(defs, race, need).is_some()
}

/// Computes all stats for a pawn.
pub fn compute_stats(ctx: &Ctx, skills: &[SkillLevel], thoughts: &[ThoughtId]) -> Vec<f32> {
    let defs = ctx.defs;
    let race = &defs.races[ctx.race];
    let mut add = vec![0.0f32; defs.stats.len()];
    let mut mul = vec![1.0f32; defs.stats.len()];
    let mut apply = |m: &StatMod| {
        if eval(&m.when, ctx) {
            match m.op {
                ModOp::Add(v) => add[m.stat.index()] += v,
                ModOp::Mul(v) => mul[m.stat.index()] *= v,
            }
        }
    };
    race.stat_mods.iter().for_each(&mut apply);
    for q in ctx.quirks {
        defs.quirks[*q].stat_mods.iter().for_each(&mut apply);
    }
    for t in thoughts {
        defs.thoughts[*t].stat_mods.iter().for_each(&mut apply);
    }
    defs.stats
        .iter()
        .map(|(id, sd)| {
            let base = race.base_stats.iter().find(|(s, _)| *s == id).map(|(_, v)| *v).unwrap_or(sd.default);
            let mut v = base;
            if let Some(sk) = sd.skill {
                let lvl = skills.get(sk.index()).map(|s| s.level).unwrap_or(0) as f32;
                v *= sd.skill_base + sd.skill_per_level * lvl;
            }
            v = (v + add[id.index()]) * mul[id.index()];
            v = v.max(sd.min);
            if let Some(mx) = sd.max {
                v = v.min(mx);
            }
            v
        })
        .collect()
}

pub fn activity_of(job: Option<&CurrentJob>) -> Activity {
    match job.map(|j| j.0.kind) {
        None => Activity::Idle,
        Some(JobKind::Sleep) => Activity::Sleeping,
        Some(k) if k.is_work() => Activity::Working,
        Some(_) => Activity::Other,
    }
}

/// Runs `f` with a fully-populated context for `pawn`. Returns None if the pawn
/// lacks required components.
pub fn with_ctx<R>(world: &World, pawn: Entity, f: impl FnOnce(&Ctx) -> R) -> Option<R> {
    let defs = world.res::<std::rc::Rc<Defs>>().clone();
    let map = world.res::<Map>();
    let clock = world.res::<Clock>();
    let others = world.res::<PawnPositions>();
    let pos = world.get::<Position>(pawn)?.0;
    let race = world.get::<Pawn>(pawn)?.race;
    let quirks = world.get::<Quirks>(pawn)?;
    let needs = world.get::<Needs>(pawn)?;
    let mood = world.get::<Mood>(pawn).map(|m| m.value).unwrap_or(50.0);
    let activity = activity_of(world.read::<CurrentJob>().get(pawn));
    let ctx = Ctx {
        defs: &defs,
        map: &map,
        clock: &clock,
        others: &others.0,
        me: pawn,
        pos,
        race,
        quirks: &quirks.0,
        needs: &needs.values,
        mood,
        activity,
    };
    Some(f(&ctx))
}

/// Active thoughts (situational + memories) for stat purposes.
pub fn active_thoughts(world: &World, pawn: Entity) -> Vec<ThoughtId> {
    match world.get::<Mood>(pawn) {
        Some(m) => m.situational.iter().copied().chain(m.memories.iter().map(|x| x.thought)).collect(),
        None => Vec::new(),
    }
}

pub fn refresh_stats(world: &World, pawn: Entity) {
    let thoughts = active_thoughts(world, pawn);
    let skills = world.get::<Skills>(pawn).map(|s| s.levels.clone()).unwrap_or_default();
    if let Some(v) = with_ctx(world, pawn, |ctx| compute_stats(ctx, &skills, &thoughts)) {
        world.defer_insert(pawn, StatCache(v));
    }
}

/// Reads a cached stat (falls back to the def default).
pub fn stat(world: &World, pawn: Entity, s: StatId) -> f32 {
    let defs = world.res::<std::rc::Rc<Defs>>().clone();
    match world.get::<StatCache>(pawn) {
        Some(c) => c.get(&defs, s),
        None => defs.stats[s].default,
    }
}

/// Learning multiplier for a skill: race and quirk aptitudes × learn rate stat.
pub fn aptitude(defs: &Defs, race: RaceId, quirks: &[QuirkId], skill: SkillId) -> f32 {
    let mut a = defs.races[race].aptitudes.iter().filter(|(s, _)| *s == skill).map(|(_, v)| *v).product::<f32>();
    for q in quirks {
        a *= defs.quirks[*q].aptitudes.iter().filter(|(s, _)| *s == skill).map(|(_, v)| *v).product::<f32>();
    }
    a
}
