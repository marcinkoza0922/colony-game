//! Social life: opinions, interactions, relationships (friends, rivals,
//! lovers, partners, family), race attitudes, social activities (hanging out,
//! games, shared meals, partners) and social fights.

use std::rc::Rc;

use ecs::{Entity, World};
use serde::{Deserialize, Serialize};

use crate::components::*;
use crate::defs::*;
use crate::geom::P3;
use crate::health::{self, DamageInfo};
use crate::jobs::{self, Job, JobKind, Toil};
use crate::path::Goal;
use crate::pawn_sys::{add_memory, add_memory_about};
use crate::resources::*;
use crate::rng::SimRng;
use crate::stats;

/// A remembered event that shapes one colonist's opinion of another.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpinionMemory {
    pub label: String,
    pub value: f32,
    pub ticks_left: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Opinion {
    pub other: Entity,
    pub memories: Vec<OpinionMemory>,
    /// 0..1; fades race attitudes as they get to know each other.
    pub familiarity: f32,
}

/// Per-colonist social state.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Social {
    pub opinions: Vec<Opinion>,
    pub relations: Vec<(Entity, RelKind)>,
    /// Tick of the next possible interaction.
    pub next_chat: u64,
    /// Current speech bubble (interaction, tick shown).
    pub bubble: Option<(InteractionId, u64)>,
    pub last_lovin: u64,
    /// When the current romance reached its stage (lover → partner timing).
    pub romance_since: u64,
}

impl Social {
    pub fn relation(&self, other: Entity) -> Vec<RelKind> {
        self.relations.iter().filter(|(e, _)| *e == other).map(|(_, k)| *k).collect()
    }
    pub fn has(&self, other: Entity, k: RelKind) -> bool {
        self.relations.iter().any(|(e, r)| *e == other && *r == k)
    }
    pub fn with(&self, k: RelKind) -> Vec<Entity> {
        self.relations.iter().filter(|(_, r)| *r == k).map(|(e, _)| *e).collect()
    }
    pub fn romantic(&self) -> Option<Entity> {
        self.relations.iter().find(|(_, r)| r.is_romantic()).map(|(e, _)| *e)
    }
    fn entry(&mut self, other: Entity) -> &mut Opinion {
        if let Some(i) = self.opinions.iter().position(|o| o.other == other) {
            return &mut self.opinions[i];
        }
        self.opinions.push(Opinion { other, memories: Vec::new(), familiarity: 0.0 });
        self.opinions.last_mut().unwrap()
    }
}

/// Social activities run as toils.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SocialToil {
    /// Hang out (near a friend or a gathering spot): recreation, and chats
    /// come more often.
    Hangout { ticks: u32 },
    /// Play at a game table, with whoever else is playing there.
    Game { table: Entity, ticks: u32 },
    /// Brawl with a colonist.
    Brawl { opponent: Entity },
}

pub enum Step {
    Running,
    Done,
    Fail(String),
}

const SOCIAL_INTERVAL: u64 = 60;
/// Interaction range (cells, same level).
const CHAT_RANGE: i32 = 5;
const FRIEND_AT: f32 = 30.0;
const FRIEND_KEEP: f32 = 15.0;
const RIVAL_AT: f32 = -30.0;
const RIVAL_KEEP: f32 = -15.0;
/// Recreation per day while socializing (relaxing alone gives 6).
const SOCIAL_REC_PER_DAY: f32 = 8.0;

fn defs(world: &World) -> Rc<Defs> {
    world.res::<Rc<Defs>>().clone()
}

fn pos_of(world: &World, e: Entity) -> Option<P3> {
    world.get::<Position>(e).map(|p| p.0)
}

fn ticks_of_days(d: f32) -> u32 {
    (d * TICKS_PER_DAY as f32) as u32
}

// ------------------------------------------------------------------ opinions

/// Standing opinion from a relationship.
fn relation_standing(k: RelKind) -> f32 {
    match k {
        RelKind::Partner => 30.0,
        RelKind::Lover => 20.0,
        RelKind::Parent | RelKind::Child => 25.0,
        RelKind::Sibling => 15.0,
        RelKind::Friend | RelKind::Rival => 0.0,
    }
}

/// Race attitude of `me` toward `other`, after quirks and familiarity.
pub fn race_bias(defs: &Defs, world: &World, me: Entity, other: Entity, familiarity: f32) -> f32 {
    let (Some(a), Some(b)) = (world.get::<Pawn>(me).map(|p| p.race), world.get::<Pawn>(other).map(|p| p.race)) else {
        return 0.0;
    };
    if a == b {
        return 0.0;
    }
    let base = defs.races[a].attitudes.iter().find(|(r, _)| *r == b).map(|(_, v)| *v).unwrap_or(0.0);
    let mult: f32 = world.get::<Quirks>(me).map(|q| q.0.iter().map(|q| defs.quirks[*q].race_bias).product()).unwrap_or(1.0);
    base * mult * (1.0 - familiarity.clamp(0.0, 1.0))
}

/// What `me` thinks of `other`, -100..100.
pub fn opinion_of(world: &World, me: Entity, other: Entity) -> f32 {
    let defs = defs(world);
    let Some(s) = world.get::<Social>(me) else { return 0.0 };
    let (mem, fam) = s
        .opinions
        .iter()
        .find(|o| o.other == other)
        .map(|o| (o.memories.iter().map(|m| m.value).sum::<f32>(), o.familiarity))
        .unwrap_or((0.0, 0.0));
    let rel: f32 = s.relation(other).into_iter().map(relation_standing).sum();
    (mem + rel + race_bias(&defs, world, me, other, fam)).clamp(-100.0, 100.0)
}

/// Opinion breakdown for the UI: (label, value).
pub fn opinion_breakdown(world: &World, me: Entity, other: Entity) -> Vec<(String, f32)> {
    let defs = defs(world);
    let mut out = Vec::new();
    let Some(s) = world.get::<Social>(me) else { return out };
    for k in s.relation(other) {
        let v = relation_standing(k);
        if v != 0.0 {
            out.push((k.label().to_string(), v));
        }
    }
    let fam = s.opinions.iter().find(|o| o.other == other).map(|o| o.familiarity).unwrap_or(0.0);
    let rb = race_bias(&defs, world, me, other, fam);
    if rb.abs() >= 0.5 {
        out.push(("Race attitude".to_string(), rb));
    }
    if let Some(o) = s.opinions.iter().find(|o| o.other == other) {
        for m in &o.memories {
            out.push((m.label.clone(), m.value));
        }
    }
    out
}

/// Adds an opinion memory (same label merges, capped at 5x one event).
pub fn add_opinion(world: &World, me: Entity, other: Entity, label: &str, value: f32, days: f32) {
    if me == other {
        return;
    }
    let Some(mut s) = world.get_mut::<Social>(me) else { return };
    let ticks = ticks_of_days(days);
    let e = s.entry(other);
    match e.memories.iter_mut().find(|m| m.label == label) {
        Some(m) => {
            let cap = value.abs() * 5.0;
            m.value = (m.value + value).clamp(-cap, cap);
            m.ticks_left = m.ticks_left.max(ticks);
        }
        None => e.memories.push(OpinionMemory { label: label.to_string(), value, ticks_left: ticks }),
    }
}

fn add_familiarity(world: &World, a: Entity, b: Entity, v: f32) {
    for (x, y) in [(a, b), (b, a)] {
        if let Some(mut s) = world.get_mut::<Social>(x) {
            let e = s.entry(y);
            e.familiarity = (e.familiarity + v).min(1.0);
        }
    }
}

pub fn set_relation(world: &World, a: Entity, b: Entity, k: RelKind) {
    if let Some(mut s) = world.get_mut::<Social>(a)
        && !s.has(b, k)
    {
        s.relations.push((b, k));
    }
    if let Some(mut s) = world.get_mut::<Social>(b)
        && !s.has(a, k.mirror())
    {
        s.relations.push((a, k.mirror()));
    }
}

pub fn clear_relation(world: &World, a: Entity, b: Entity, k: RelKind) {
    if let Some(mut s) = world.get_mut::<Social>(a) {
        s.relations.retain(|(e, r)| !(*e == b && *r == k));
    }
    if let Some(mut s) = world.get_mut::<Social>(b) {
        s.relations.retain(|(e, r)| !(*e == a && *r == k.mirror()));
    }
}

fn has_rel(world: &World, a: Entity, b: Entity, k: RelKind) -> bool {
    world.get::<Social>(a).is_some_and(|s| s.has(b, k))
}

fn is_family(world: &World, a: Entity, b: Entity) -> bool {
    world.get::<Social>(a).is_some_and(|s| s.relation(b).iter().any(|k| k.is_family()))
}

/// Romantic compatibility 0..1 by race.
pub fn compatibility(defs: &Defs, world: &World, a: Entity, b: Entity) -> f32 {
    let (Some(ra), Some(rb)) = (world.get::<Pawn>(a).map(|p| p.race), world.get::<Pawn>(b).map(|p| p.race)) else {
        return 0.0;
    };
    let f = |x: RaceId, y: RaceId| {
        if x == y {
            1.0
        } else {
            defs.races[x].romance.iter().find(|(r, _)| *r == y).map(|(_, v)| *v).unwrap_or(defs.races[x].romance_default)
        }
    };
    f(ra, rb).min(f(rb, ra))
}

// -------------------------------------------------------------- availability

fn can_socialize(world: &World, e: Entity) -> bool {
    if !world.has::<Social>(e) || health::is_downed(world, e) || world.has::<MentalState>(e) {
        return false;
    }
    match world.get::<CurrentJob>(e) {
        Some(j) => !j.is_sleeping() && !matches!(j.0.kind, JobKind::Fight | JobKind::Downed),
        None => true,
    }
}

fn socializing(world: &World, e: Entity) -> bool {
    world.get::<CurrentJob>(e).is_some_and(|j| {
        j.0.kind == JobKind::Socialize
            || j.0.kind == JobKind::Eat && crate::stats::with_ctx(world, e, |c| stats::eval(&Condition::AtTable, c)).unwrap_or(false)
    })
}

// ------------------------------------------------------------------- system

pub fn sys_social(world: &mut World) {
    let tick = world.res::<Clock>().tick;
    if !tick.is_multiple_of(SOCIAL_INTERVAL) {
        return;
    }
    let pawns: Vec<Entity> = world.read::<Social>().entities().to_vec();
    // Forget old memories; drop opinions of the dead.
    for &p in &pawns {
        let mut s = world.get_mut::<Social>(p).unwrap();
        for o in &mut s.opinions {
            for m in &mut o.memories {
                m.ticks_left = m.ticks_left.saturating_sub(SOCIAL_INTERVAL as u32);
            }
            o.memories.retain(|m| m.ticks_left > 0);
        }
        if s.bubble.is_some_and(|(_, t)| tick > t + 150) {
            s.bubble = None;
        }
    }
    if tick.is_multiple_of(SOCIAL_INTERVAL * 5) {
        update_bonds(world, &pawns);
    }
    if tick.is_multiple_of(TICKS_PER_HOUR) {
        romance_hourly(world, &pawns);
    }
    for &p in &pawns {
        try_interact(world, p, tick);
    }
}

/// Friends and rivals form and fade with mutual opinion.
fn update_bonds(world: &mut World, pawns: &[Entity]) {
    for (i, &a) in pawns.iter().enumerate() {
        for &b in &pawns[i + 1..] {
            let o = (opinion_of(world, a, b) + opinion_of(world, b, a)) * 0.5;
            let friends = has_rel(world, a, b, RelKind::Friend);
            let rivals = has_rel(world, a, b, RelKind::Rival);
            let (na, nb) = (jobs::pawn_name(world, a), jobs::pawn_name(world, b));
            if !friends && o >= FRIEND_AT {
                set_relation(world, a, b, RelKind::Friend);
                jobs::log_about(world, LogKind::Good, a, format!("{na} and {nb} are now friends."));
            } else if friends && o < FRIEND_KEEP {
                clear_relation(world, a, b, RelKind::Friend);
                jobs::log_about(world, LogKind::Info, a, format!("{na} and {nb} have drifted apart."));
            }
            if !rivals && o <= RIVAL_AT {
                set_relation(world, a, b, RelKind::Rival);
                jobs::log_about(world, LogKind::Bad, a, format!("{na} and {nb} have become rivals."));
            } else if rivals && o > RIVAL_KEEP {
                clear_relation(world, a, b, RelKind::Rival);
                jobs::log_about(world, LogKind::Good, a, format!("{na} and {nb} buried the hatchet."));
            }
        }
    }
}

/// Lovers may become partners; souring romances may break up.
fn romance_hourly(world: &mut World, pawns: &[Entity]) {
    let defs = defs(world);
    let tick = world.res::<Clock>().tick;
    for (i, &a) in pawns.iter().enumerate() {
        for &b in &pawns[i + 1..] {
            let lovers = has_rel(world, a, b, RelKind::Lover);
            let partners = has_rel(world, a, b, RelKind::Partner);
            if !lovers && !partners {
                continue;
            }
            let (oa, ob) = (opinion_of(world, a, b), opinion_of(world, b, a));
            let since = world.get::<Social>(a).map(|s| s.romance_since).unwrap_or(0);
            let roll = world.res_mut::<SimRng>().f32();
            let (na, nb) = (jobs::pawn_name(world, a), jobs::pawn_name(world, b));
            if oa.min(ob) < 0.0 && roll < 0.2 / 24.0 * (1.0 - oa.min(ob) / 50.0) {
                let k = if partners { RelKind::Partner } else { RelKind::Lover };
                clear_relation(world, a, b, k);
                for (x, y) in [(a, b), (b, a)] {
                    add_opinion(world, x, y, "Broke up", -15.0, 10.0);
                    if let Some(t) = defs.thoughts.find("broke_up") {
                        add_memory_about(world, x, t, Some(jobs::pawn_name(world, y)));
                    }
                }
                jobs::log_about(world, LogKind::Bad, a, format!("{na} and {nb} have broken up."));
                continue;
            }
            if lovers && oa.min(ob) >= 50.0 && tick.saturating_sub(since) > 3 * TICKS_PER_DAY && roll < 0.35 / 24.0 {
                clear_relation(world, a, b, RelKind::Lover);
                set_relation(world, a, b, RelKind::Partner);
                for x in [a, b] {
                    if let Some(mut s) = world.get_mut::<Social>(x) {
                        s.romance_since = tick;
                    }
                    if let Some(t) = defs.thoughts.find("became_partners") {
                        add_memory(world, x, t);
                    }
                }
                share_bed(world, a, b);
                jobs::log_about(world, LogKind::Good, a, format!("{na} and {nb} are now partners."));
            }
        }
    }
}

/// Partners move into a double bed if one of them owns one.
pub fn share_bed(world: &World, a: Entity, b: Entity) {
    let defs = defs(world);
    let mut beds = world.write::<Bed>();
    let builds = world.read::<Building>();
    for (x, y) in [(a, b), (b, a)] {
        let double = beds.iter().find(|(e, bd)| {
            bd.owner == Some(x) && builds.get(*e).is_some_and(|bb| defs.buildings[bb.def].slots >= 2)
        });
        if let Some((e, _)) = double {
            for (_, bd) in beds.iter_mut() {
                if bd.owner == Some(y) {
                    bd.owner = None;
                }
                if bd.second == Some(y) {
                    bd.second = None;
                }
            }
            beds.get_mut(e).unwrap().second = Some(y);
            return;
        }
    }
}

fn try_interact(world: &mut World, a: Entity, tick: u64) {
    let defs = defs(world);
    if world.get::<Social>(a).is_none_or(|s| tick < s.next_chat) || !can_socialize(world, a) {
        return;
    }
    let Some(pa) = pos_of(world, a) else { return };
    let others: Vec<Entity> = world
        .res::<PawnPositions>()
        .0
        .iter()
        .filter(|(e, p)| {
            *e != a && p.z == pa.z && (p.x - pa.x).abs() <= CHAT_RANGE && (p.y - pa.y).abs() <= CHAT_RANGE
        })
        .map(|(e, _)| *e)
        .filter(|e| can_socialize(world, *e))
        .collect();
    let chatty: f32 =
        world.get::<Quirks>(a).map(|q| q.0.iter().map(|q| defs.quirks[*q].chattiness).product()).unwrap_or(1.0);
    let boost = if socializing(world, a) { 3.0 } else { 1.0 };
    let (b, gap) = {
        let mut rng = world.res_mut::<SimRng>();
        let gap = (rng.range_f32(1500.0, 4500.0) / (chatty * boost).max(0.1)) as u64;
        (rng.pick(&others).copied(), gap)
    };
    world.get_mut::<Social>(a).unwrap().next_chat = tick + if b.is_some() { gap } else { 300 };
    let Some(b) = b else { return };
    let Some(iid) = choose_interaction(world, &defs, a, b) else { return };
    interact(world, a, b, iid);
}

fn choose_interaction(world: &World, defs: &Defs, a: Entity, b: Entity) -> Option<InteractionId> {
    let op = opinion_of(world, a, b);
    let quirks = world.get::<Quirks>(a).map(|q| q.0.clone()).unwrap_or_default();
    let romance_ok = romance_allowed(world, defs, a, b);
    let weights: Vec<f32> = defs
        .interactions
        .iter()
        .map(|(_, d)| {
            if d.romance && !romance_ok {
                return 0.0;
            }
            let mut w = d.weight;
            if !d.opinion_curve.is_empty() {
                w *= eval_curve(&d.opinion_curve, op);
            }
            for (q, m) in &d.quirk_weights {
                if quirks.contains(q) {
                    w *= m;
                }
            }
            w.max(0.0)
        })
        .collect();
    let i = world.res_mut::<SimRng>().weighted(&weights)?;
    Some(InteractionId::from_index(i))
}

fn adult(defs: &Defs, world: &World, e: Entity) -> bool {
    world.get::<Pawn>(e).is_some_and(|p| p.age >= defs.races[p.race].adult_age.0)
}

fn romance_allowed(world: &World, defs: &Defs, a: Entity, b: Entity) -> bool {
    if !adult(defs, world, a) || !adult(defs, world, b) || is_family(world, a, b) {
        return false;
    }
    let current = world.get::<Social>(a).and_then(|s| s.romantic());
    if current == Some(b) {
        return false;
    }
    let op = opinion_of(world, a, b);
    // Already taken: only strays for someone much more liked than the partner.
    if let Some(c) = current
        && op < opinion_of(world, a, c) + 30.0
    {
        return false;
    }
    compatibility(defs, world, a, b) > 0.0 && op >= 20.0
}

/// Runs one interaction from `a` to `b`.
pub fn interact(world: &mut World, a: Entity, b: Entity, iid: InteractionId) {
    let defs = defs(world);
    let d = defs.interactions[iid].clone();
    let tick = world.res::<Clock>().tick;
    let impact = stats::stat_by_name(world, a, "social_impact").unwrap_or(1.0);
    let (na, nb) = (jobs::pawn_name(world, a), jobs::pawn_name(world, b));
    if let Some(mut s) = world.get_mut::<Social>(a) {
        s.bubble = Some((iid, tick));
    }
    add_familiarity(world, a, b, 0.03);
    if d.romance {
        romance_attempt(world, &defs, a, b, impact);
    } else {
        if d.opinion != 0.0 {
            add_opinion(world, b, a, &d.label, d.opinion * impact, d.memory_days);
        }
        if d.opinion_initiator != 0.0 {
            add_opinion(world, a, b, &d.label, d.opinion_initiator, d.memory_days);
        }
        if let Some(t) = d.thought {
            add_memory_about(world, b, t, Some(na.clone()));
        }
        if let Some(t) = d.thought_initiator {
            add_memory_about(world, a, t, Some(nb.clone()));
        }
    }
    if let Some(sk) = defs.skills.find("social") {
        jobs::gain_xp(world, a, sk, d.xp);
    }
    if !d.log.is_empty() {
        let text = d.log.replace("{a}", &na).replace("{b}", &nb);
        jobs::log_about(world, LogKind::Info, a, text);
    }
    if d.insult {
        maybe_fight(world, &defs, b, a);
    }
}

fn romance_attempt(world: &mut World, defs: &Defs, a: Entity, b: Entity, impact: f32) {
    let compat = compatibility(defs, world, a, b);
    let ob = opinion_of(world, b, a);
    let b_taken = world.get::<Social>(b).and_then(|s| s.romantic());
    let mut chance = compat * ((ob + 20.0) / 100.0).clamp(0.0, 1.0) * impact.min(1.5);
    if b_taken.is_some() {
        chance *= 0.05;
    }
    let roll = world.res_mut::<SimRng>().f32();
    let (na, nb) = (jobs::pawn_name(world, a), jobs::pawn_name(world, b));
    if roll >= chance {
        add_opinion(world, a, b, "Rebuffed me", -5.0, 3.0);
        if let Some(t) = defs.thoughts.find("rebuffed") {
            add_memory_about(world, a, t, Some(nb.clone()));
        }
        return;
    }
    // Leaving someone for the new love: the jilted partner takes it hard.
    for (x, old) in [(a, world.get::<Social>(a).and_then(|s| s.romantic())), (b, b_taken)] {
        let Some(old) = old else { continue };
        for k in [RelKind::Lover, RelKind::Partner] {
            clear_relation(world, x, old, k);
        }
        add_opinion(world, old, x, "Cheated on me", -40.0, 20.0);
        if let Some(t) = defs.thoughts.find("cheated_on") {
            add_memory_about(world, old, t, Some(jobs::pawn_name(world, x)));
        }
        let (nx, no) = (jobs::pawn_name(world, x), jobs::pawn_name(world, old));
        jobs::log_about(world, LogKind::Bad, x, format!("{nx} left {no}."));
    }
    set_relation(world, a, b, RelKind::Lover);
    let tick = world.res::<Clock>().tick;
    for (x, y) in [(a, b), (b, a)] {
        add_opinion(world, x, y, "Romance", 10.0, 10.0);
        if let Some(mut s) = world.get_mut::<Social>(x) {
            s.romance_since = tick;
        }
        if let Some(t) = defs.thoughts.find("new_love") {
            add_memory_about(world, x, t, Some(jobs::pawn_name(world, y)));
        }
    }
    jobs::log_about(world, LogKind::Good, a, format!("{na} and {nb} have become lovers."));
}

// -------------------------------------------------------------------- fights

fn maybe_fight(world: &mut World, defs: &Defs, attacker: Entity, target: Entity) {
    let op = opinion_of(world, attacker, target);
    let rival = has_rel(world, attacker, target, RelKind::Rival);
    if op > -10.0 && !rival {
        return;
    }
    let qmul: f32 =
        world.get::<Quirks>(attacker).map(|q| q.0.iter().map(|q| defs.quirks[*q].fight_chance).product()).unwrap_or(1.0);
    let mood = world.get::<Mood>(attacker).map(|m| m.value).unwrap_or(50.0);
    let chance = 0.12 * qmul * if mood < 30.0 { 2.0 } else { 1.0 } * if rival { 1.5 } else { 1.0 };
    if world.res_mut::<SimRng>().chance(chance) {
        start_fight(world, attacker, target);
    }
}

/// Starts a brawl: the attacker closes in, the target squares up.
pub fn start_fight(world: &mut World, attacker: Entity, target: Entity) {
    if !can_socialize(world, attacker) || !can_socialize(world, target) {
        return;
    }
    let Some(tp) = pos_of(world, target) else { return };
    jobs::interrupt(world, attacker, "fight");
    jobs::interrupt(world, target, "fight");
    world.insert(
        attacker,
        CurrentJob(Job::new(
            JobKind::Fight,
            vec![Toil::GoTo(Goal::Near(tp)), Toil::Social(SocialToil::Brawl { opponent: target })],
        )),
    );
    world.insert(target, CurrentJob(Job::new(JobKind::Fight, vec![Toil::Social(SocialToil::Brawl { opponent: attacker })])));
    let (na, nb) = (jobs::pawn_name(world, attacker), jobs::pawn_name(world, target));
    jobs::log_about(world, LogKind::Bad, attacker, format!("{na} has started a fight with {nb}!"));
}

fn fighting(world: &World, e: Entity) -> bool {
    world.get::<CurrentJob>(e).is_some_and(|j| j.0.kind == JobKind::Fight)
}

fn brawl(world: &mut World, me: Entity, job: &mut Job, opponent: Entity) -> Step {
    job.ticks += 1;
    let defs = defs(world);
    let over = |w: &World, e: Entity| !w.is_alive(e) || health::is_downed(w, e) || w.get::<crate::health::Health>(e).is_some_and(|h| h.pain > 0.45);
    if over(world, me) || over(world, opponent) || !fighting(world, opponent) || job.ticks > 900 {
        end_fight(world, &defs, me, opponent);
        return Step::Done;
    }
    let (Some(a), Some(b)) = (pos_of(world, me), pos_of(world, opponent)) else { return Step::Done };
    if !a.is_adjacent8(b) {
        return if job.ticks > 400 { Step::Done } else { Step::Running };
    }
    if let Some(f) = Facing::from_delta(b.x - a.x, b.y - a.y) {
        world.insert(me, f);
    }
    if !job.ticks.is_multiple_of(45) {
        return Step::Running;
    }
    let combat = defs.skills.find("combat");
    let lvl = combat.and_then(|c| world.get::<Skills>(me).map(|s| s.levels.get(c.index()).map_or(0, |l| l.level))).unwrap_or(0);
    let (hit, amount) = {
        let mut rng = world.res_mut::<SimRng>();
        (rng.chance(0.5 + lvl as f32 * 0.02), rng.range_f32(2.0, 6.0))
    };
    if let Some(c) = combat {
        jobs::gain_xp(world, me, c, 20.0);
    }
    if hit && let Some(dt) = defs.damage_types.find("blunt") {
        let lethal = defs.interactions.iter().any(|(_, d)| d.insult && d.lethal_fights);
        health::apply_damage(world, opponent, &DamageInfo { dtype: dt, amount, part: None, non_lethal: !lethal, surface: true });
    }
    Step::Running
}

fn end_fight(world: &mut World, defs: &Defs, me: Entity, opponent: Entity) {
    // Only the first side to notice wraps up.
    let first = fighting(world, opponent) && world.is_alive(opponent);
    if world.is_alive(opponent) && fighting(world, opponent) {
        jobs::interrupt(world, opponent, "fight over");
    }
    if !first {
        return;
    }
    for (x, y) in [(me, opponent), (opponent, me)] {
        if !world.is_alive(x) {
            continue;
        }
        add_opinion(world, x, y, "Fought me", -20.0, 12.0);
        if let Some(t) = defs.thoughts.find("had_fight") {
            add_memory_about(world, x, t, Some(jobs::pawn_name(world, y)));
        }
    }
    let (na, nb) = (jobs::pawn_name(world, me), jobs::pawn_name(world, opponent));
    jobs::log_about(world, LogKind::Info, me, format!("The fight between {na} and {nb} is over."));
}

// ---------------------------------------------------------------- activities

/// Recreation with others: a game table, a friend or partner, or a gathering
/// spot. None if nothing social is available.
pub fn recreation_job(world: &World, pawn: Entity, pos: P3) -> Option<Job> {
    let defs = defs(world);
    let roll = world.res_mut::<SimRng>().f32();
    // Join someone waiting at a game table, or start a game sometimes.
    if let Some(j) = game_job(world, &defs, pos, roll < 0.4) {
        return Some(j);
    }
    // Seek out a partner / friend who is free.
    let mut friends: Vec<(Entity, f32)> = world
        .get::<Social>(pawn)
        .map(|s| {
            s.relations
                .iter()
                .filter(|(_, k)| k.is_romantic() || *k == RelKind::Friend || k.is_family())
                .map(|(e, k)| (*e, if k.is_romantic() { 2.0 } else { 1.0 }))
                .collect()
        })
        .unwrap_or_default();
    friends.retain(|(e, _)| {
        can_socialize(world, *e) && !world.get::<CurrentJob>(*e).is_some_and(|j| j.0.kind.is_work())
    });
    friends.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    if let Some((f, _)) = friends.first()
        && let Some(fp) = pos_of(world, *f)
        && fp.manhattan(pos) < 60
    {
        let ticks = 700;
        return Some(Job::new(
            JobKind::Socialize,
            vec![Toil::GoTo(Goal::Near(fp)), Toil::Social(SocialToil::Hangout { ticks })],
        ));
    }
    // A gathering spot: tables and campfires, if others are around.
    if roll < 0.6 {
        let spot = gathering_spot(world, &defs, pos)?;
        return Some(Job::new(
            JobKind::Socialize,
            vec![Toil::GoTo(Goal::Near(spot)), Toil::Social(SocialToil::Hangout { ticks: 700 })],
        ));
    }
    None
}

fn gathering_spot(world: &World, defs: &Defs, pos: P3) -> Option<P3> {
    let builds = world.read::<Building>();
    let posst = world.read::<Position>();
    builds
        .iter()
        .filter(|(_, b)| {
            let d = &defs.buildings[b.def];
            matches!(d.kind, BuildingKind::Table | BuildingKind::GameTable) || d.name == "campfire"
        })
        .filter_map(|(e, _)| posst.get(e).map(|p| p.0))
        .filter(|p| p.z == pos.z && p.manhattan(pos) < 50)
        .min_by_key(|p| (p.manhattan(pos), p.y, p.x))
}

/// Who is at a game table (in the Game toil).
fn players_at(world: &World, table: Entity) -> Vec<Entity> {
    world
        .read::<CurrentJob>()
        .iter()
        .filter(|(_, j)| matches!(j.0.toils.get(j.0.step), Some(Toil::Social(SocialToil::Game { table: t, .. })) if *t == table))
        .map(|(e, _)| e)
        .collect()
}

fn game_job(world: &World, defs: &Defs, pos: P3, may_start: bool) -> Option<Job> {
    let tables: Vec<(Entity, P3)> = {
        let builds = world.read::<Building>();
        let posst = world.read::<Position>();
        builds
            .iter()
            .filter(|(_, b)| defs.buildings[b.def].kind == BuildingKind::GameTable)
            .filter_map(|(e, _)| posst.get(e).map(|p| (e, p.0)))
            .filter(|(_, p)| p.manhattan(pos) < 80)
            .collect()
    };
    let mut best: Option<(Entity, P3, usize)> = None;
    for (t, p) in tables {
        let n = players_at(world, t).len();
        if n >= 2 || (n == 0 && !may_start) {
            continue;
        }
        if best.is_none_or(|(_, bp, bn)| n > bn || n == bn && p.manhattan(pos) < bp.manhattan(pos)) {
            best = Some((t, p, n));
        }
    }
    let (table, tp, _) = best?;
    Some(Job::new(
        JobKind::Socialize,
        vec![Toil::GoTo(Goal::Adjacent(tp)), Toil::Social(SocialToil::Game { table, ticks: 1500 })],
    ))
}

fn gain_recreation(world: &World, pawn: Entity, per_day: f32) {
    let defs = defs(world);
    if let Some(rec) = defs.needs.iter().find(|(_, n)| n.role == Some(NeedRole::Recreation)).map(|(i, _)| i)
        && let Some(mut n) = world.get_mut::<Needs>(pawn)
        && let Some(v) = n.values.get_mut(rec.index())
    {
        *v = (*v + per_day / TICKS_PER_DAY as f32).min(1.0);
    }
}

/// Runs a social toil.
pub fn step(world: &mut World, pawn: Entity, job: &mut Job, t: SocialToil) -> Step {
    match t {
        SocialToil::Hangout { ticks } => {
            job.ticks += 1;
            gain_recreation(world, pawn, SOCIAL_REC_PER_DAY);
            if job.ticks >= ticks { Step::Done } else { Step::Running }
        }
        SocialToil::Game { table, ticks } => {
            job.ticks += 1;
            let Some(tp) = pos_of(world, table) else { return Step::Fail("table gone".into()) };
            if let Some(pp) = pos_of(world, pawn)
                && let Some(f) = Facing::from_delta(tp.x - pp.x, tp.y - pp.y)
            {
                world.insert(pawn, f);
            }
            let others: Vec<Entity> = players_at(world, table).into_iter().filter(|e| *e != pawn).collect();
            match others.first() {
                Some(&o) => {
                    gain_recreation(world, pawn, SOCIAL_REC_PER_DAY * 1.5);
                    if job.ticks.is_multiple_of(300) {
                        let defs = defs(world);
                        let label = format!("Played {}", defs.buildings[world.get::<Building>(table).unwrap().def].label.to_lowercase());
                        add_opinion(world, pawn, o, &label, 1.0, 4.0);
                        add_familiarity(world, pawn, o, 0.01);
                        if let Some(sk) = defs.skills.find("social") {
                            jobs::gain_xp(world, pawn, sk, 10.0);
                        }
                        if job.ticks == 600
                            && let Some(th) = defs.thoughts.find("played_game")
                        {
                            add_memory_about(world, pawn, th, Some(jobs::pawn_name(world, o)));
                        }
                    }
                    if job.ticks >= ticks { Step::Done } else { Step::Running }
                }
                None => {
                    gain_recreation(world, pawn, SOCIAL_REC_PER_DAY * 0.3);
                    // Give up waiting for a partner after a while.
                    if job.ticks >= ticks / 2 { Step::Done } else { Step::Running }
                }
            }
        }
        SocialToil::Brawl { opponent } => brawl(world, pawn, job, opponent),
    }
}

/// When a partner shares the bed at bedtime: lovin'.
pub fn bedtime(world: &mut World, pawn: Entity, bed: Entity) {
    let defs = defs(world);
    let tick = world.res::<Clock>().tick;
    let Some(partner) = world.get::<Social>(pawn).and_then(|s| s.romantic()) else { return };
    let (Some(bp), Some(pp)) = (pos_of(world, bed), pos_of(world, partner)) else { return };
    if bp != pp || !world.get::<CurrentJob>(partner).is_some_and(|j| j.is_sleeping()) {
        return;
    }
    let last = world.get::<Social>(pawn).map(|s| s.last_lovin).unwrap_or(0);
    if tick.saturating_sub(last) < TICKS_PER_DAY / 2 || !world.res_mut::<SimRng>().chance(0.6) {
        return;
    }
    for (x, y) in [(pawn, partner), (partner, pawn)] {
        if let Some(mut s) = world.get_mut::<Social>(x) {
            s.last_lovin = tick;
        }
        add_opinion(world, x, y, "Lovin'", 3.0, 3.0);
        if let Some(t) = defs.thoughts.find("lovin") {
            add_memory_about(world, x, t, Some(jobs::pawn_name(world, y)));
        }
    }
    if let Some(i) = defs.interactions.find("lovin") {
        world.get_mut::<Social>(pawn).unwrap().bubble = Some((i, tick));
    }
}

// --------------------------------------------------------------------- death

/// Grief and cleanup when a colonist dies.
pub fn on_death(world: &mut World, dead: Entity, name: &str) {
    let defs = defs(world);
    let others: Vec<Entity> = world.read::<Social>().entities().iter().copied().filter(|e| *e != dead).collect();
    for o in others {
        let rels = world.get::<Social>(o).map(|s| s.relation(dead)).unwrap_or_default();
        let op = opinion_of(world, o, dead);
        let thought = if rels.contains(&RelKind::Partner) {
            "partner_died"
        } else if rels.contains(&RelKind::Lover) {
            "lover_died"
        } else if rels.iter().any(|k| k.is_family()) {
            "family_died"
        } else if rels.contains(&RelKind::Friend) || op >= 20.0 {
            "friend_died"
        } else if rels.contains(&RelKind::Rival) || op <= -30.0 {
            "rival_died"
        } else {
            "colonist_died"
        };
        if let Some(t) = defs.thoughts.find(thought) {
            add_memory_about(world, o, t, Some(name.to_string()));
        }
        if let Some(mut s) = world.get_mut::<Social>(o) {
            s.relations.retain(|(e, _)| *e != dead);
            s.opinions.retain(|x| x.other != dead);
        }
    }
    let mut beds = world.write::<Bed>();
    for (_, b) in beds.iter_mut() {
        if b.second == Some(dead) {
            b.second = None;
        }
    }
}

// -------------------------------------------------------------------- family

/// Makes `kin` a relative of `of`: sibling, parent or child (ages and last
/// name adjusted to fit).
pub fn make_family(world: &mut World, of: Entity, kin: Entity) {
    let defs = defs(world);
    let (Some(a), Some(b)) = (world.get_cloned::<Pawn>(of), world.get_cloned::<Pawn>(kin)) else { return };
    let rd = &defs.races[a.race];
    let (lo, hi) = (rd.adult_age.0 as i32, rd.adult_age.1 as i32);
    let gen_gap = ((hi - lo) / 3).max(16);
    let (roll, gap) = {
        let mut rng = world.res_mut::<SimRng>();
        (rng.f32(), rng.range_i32(gen_gap * 2 / 3, gen_gap))
    };
    let (kind, age) = if roll < 0.5 {
        (RelKind::Sibling, (a.age as i32 + gap / 4 - gap / 8).clamp(lo, hi))
    } else if a.age as i32 + gap <= hi + gap / 2 {
        (RelKind::Parent, a.age as i32 + gap)
    } else {
        (RelKind::Child, (a.age as i32 - gap).max(lo))
    };
    let last = a.name.split_once(' ').map(|(_, l)| l.to_string());
    let first = b.name.split_once(' ').map(|(f, _)| f.to_string()).unwrap_or(b.name.clone());
    if let Some(mut p) = world.get_mut::<Pawn>(kin) {
        p.age = age.max(1) as u32;
        p.name = match last {
            Some(l) => format!("{first} {l}"),
            None => first,
        };
    }
    // `kin` is the `kind` of `of`.
    set_relation(world, of, kin, kind);
    add_familiarity(world, of, kin, 1.0);
}
