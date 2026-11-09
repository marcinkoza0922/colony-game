//! Health: body parts, hediffs (injuries, illnesses, conditions), capacities,
//! pain, bleeding, healing, downed state and death.
//!
//! All of it is data-driven (`defs/bodies`, `hediffs`, `capacities`,
//! `damage_types`). Combat (M5) will reuse [`apply_damage`].

use std::rc::Rc;

use ecs::{Entity, World};
use serde::{Deserialize, Serialize};

use crate::components::*;
use crate::defs::*;
use crate::env::Env;
use crate::geom::P3;
use crate::jobs;
use crate::map::Map;
use crate::resources::*;
use crate::rng::SimRng;
use crate::stats;

/// One health condition on a pawn.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hediff {
    pub def: HediffId,
    /// Body part index (`None` = whole body).
    pub part: Option<u8>,
    pub severity: f32,
    /// Highest severity reached (scar size).
    pub peak: f32,
    /// Tend quality 0..1 while tended.
    pub tended: Option<f32>,
    /// Ticks until the tend wears off (hediffs with `tend_hours`).
    pub tend_left: u32,
    /// Healing multiplier from the medicine used.
    pub heal_mult: f32,
    /// Illness immunity progress 0..1.
    pub immunity: f32,
    /// Age in ticks.
    pub age: u32,
    /// The once-per-wound infection roll has happened.
    pub infection_rolled: bool,
}

impl Hediff {
    pub fn new(def: HediffId, part: Option<usize>, severity: f32) -> Self {
        Self {
            def,
            part: part.map(|p| p as u8),
            severity,
            peak: severity,
            tended: None,
            tend_left: 0,
            heal_mult: 1.0,
            immunity: 0.0,
            age: 0,
            infection_rolled: false,
        }
    }

    pub fn part(&self) -> Option<usize> {
        self.part.map(|p| p as usize)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Health {
    pub hediffs: Vec<Hediff>,
    /// Cached capacity values (indexed by `CapacityId`).
    pub capacities: Vec<f32>,
    /// Cached total pain 0..1.
    pub pain: f32,
    /// Cached blood loss per day.
    pub bleeding: f32,
    pub downed: bool,
}

impl Health {
    pub fn capacity(&self, c: CapacityId) -> f32 {
        self.capacities.get(c.index()).copied().unwrap_or(1.0)
    }

    pub fn has(&self, h: HediffId) -> bool {
        self.hediffs.iter().any(|x| x.def == h)
    }

    /// Anything a doctor could treat right now.
    pub fn needs_tending(&self, defs: &Defs) -> bool {
        self.hediffs.iter().any(|h| defs.hediffs[h.def].tendable && h.tended.is_none())
    }

    /// Should rest in bed (injured, ill, downed, or waiting for a doctor).
    pub fn wants_bed_rest(&self, defs: &Defs) -> bool {
        self.downed
            || self.bleeding > 0.0
            || self.needs_tending(defs)
            || self.hediffs.iter().any(|h| {
                let d = &defs.hediffs[h.def];
                match d.kind {
                    HediffKind::Illness => h.severity > 0.3 || d.lethal_at.is_some() && h.immunity < 1.0,
                    HediffKind::Injury => h.severity > 6.0,
                    HediffKind::Condition => d.lethal_at.is_some() && h.severity > 0.2,
                    _ => false,
                }
            })
    }
}

/// HP lost on a part from injuries and scars.
pub fn part_damage(defs: &Defs, h: &Health, part: usize) -> f32 {
    h.hediffs
        .iter()
        .filter(|x| x.part() == Some(part) && matches!(defs.hediffs[x.def].kind, HediffKind::Injury | HediffKind::Scar))
        .map(|x| x.severity)
        .sum()
}

/// The part or one of its ancestors is missing.
pub fn is_missing(defs: &Defs, body: &BodyDef, h: &Health, part: usize) -> bool {
    h.hediffs.iter().any(|x| {
        defs.hediffs[x.def].kind == HediffKind::MissingPart
            && x.part().is_some_and(|m| m == part || body.is_ancestor(m, part))
    })
}

pub fn part_hp(defs: &Defs, body: &BodyDef, h: &Health, part: usize) -> f32 {
    if is_missing(defs, body, h, part) {
        return 0.0;
    }
    (body.parts[part].hp - part_damage(defs, h, part)).max(0.0)
}

/// 0..1 working fraction of a part.
pub fn part_efficiency(defs: &Defs, body: &BodyDef, h: &Health, part: usize) -> f32 {
    let max = body.parts[part].hp.max(1.0);
    (part_hp(defs, body, h, part) / max).clamp(0.0, 1.0)
}

fn sev_pain(d: &HediffDef, x: &Hediff) -> f32 {
    let base = d.pain * x.severity;
    let stage = d.stage(x.severity).map(|s| s.pain).unwrap_or(0.0);
    base + stage
}

/// Recomputes cached capacities, pain and bleeding.
pub fn recompute(defs: &Defs, body: &BodyDef, h: &mut Health) {
    let ncap = defs.capacities.len();
    // Part contributions.
    let mut sum = vec![0.0f32; ncap];
    let mut wsum = vec![0.0f32; ncap];
    let mut crit = vec![1.0f32; ncap];
    for (i, p) in body.parts.iter().enumerate() {
        if p.capacities.is_empty() && p.critical_for.is_empty() {
            continue;
        }
        let eff = part_efficiency(defs, body, h, i);
        for (c, w) in &p.capacities {
            sum[c.index()] += w * eff;
            wsum[c.index()] += w;
        }
        for c in &p.critical_for {
            crit[c.index()] *= eff;
        }
    }
    // Pain and hediff offsets.
    let mut pain = 0.0;
    let mut offs = vec![0.0f32; ncap];
    let mut bleeding = 0.0;
    for x in &h.hediffs {
        let d = &defs.hediffs[x.def];
        pain += sev_pain(d, x);
        if let Some(st) = d.stage(x.severity) {
            for (c, v) in &st.capacities {
                offs[c.index()] += v;
            }
        }
        if x.tended.is_none() && d.bleed > 0.0 {
            let fresh = d.kind != HediffKind::MissingPart || x.age < TICKS_PER_DAY as u32 * 2;
            if fresh {
                bleeding += d.bleed * x.severity;
            }
        }
    }
    let pain = pain.clamp(0.0, 1.0);
    let mut caps = vec![1.0f32; ncap];
    for (id, cd) in defs.capacities.iter() {
        let i = id.index();
        let mut v = if wsum[i] > 0.0 { sum[i] / wsum[i] } else { 1.0 };
        v *= crit[i];
        v += offs[i] + cd.pain * pain;
        for (dep, w) in &cd.depends {
            let o = caps[dep.index()].min(1.0);
            v *= 1.0 + (o - 1.0) * w;
        }
        caps[i] = v.clamp(0.0, 1.5);
    }
    h.capacities = caps;
    h.pain = pain;
    h.bleeding = bleeding;
}

fn defs(world: &World) -> Rc<Defs> {
    world.res::<Rc<Defs>>().clone()
}

pub fn body_of<'a>(defs: &'a Defs, world: &World, pawn: Entity) -> Option<&'a BodyDef> {
    let race = world.get::<Pawn>(pawn)?.race;
    Some(&defs.bodies[defs.races[race].body])
}

pub fn is_downed(world: &World, pawn: Entity) -> bool {
    world.get::<Health>(pawn).is_some_and(|h| h.downed)
}

/// Picks a body part for a hit by walking down the tree by coverage.
/// `surface` skips internal organs (accidents, fists).
pub fn random_hit_part(defs: &Defs, body: &BodyDef, h: &Health, rng: &mut SimRng, surface: bool) -> usize {
    let mut cur = 0;
    loop {
        let roll = rng.f32();
        let mut acc = 0.0;
        let mut next = None;
        for c in body.children(cur) {
            if is_missing(defs, body, h, c) || surface && body.parts[c].inside {
                continue;
            }
            acc += body.parts[c].coverage;
            if roll < acc {
                next = Some(c);
                break;
            }
        }
        match next {
            Some(n) => cur = n,
            None => return cur,
        }
    }
}

/// A random present part with a tag.
fn random_tagged_part(defs: &Defs, body: &BodyDef, h: &Health, tag: &str, rng: &mut SimRng) -> Option<usize> {
    let parts: Vec<usize> =
        (0..body.parts.len()).filter(|&i| body.parts[i].has_tag(tag) && !is_missing(defs, body, h, i)).collect();
    rng.pick(&parts).copied()
}

#[derive(Clone, Debug)]
pub struct DamageInfo {
    pub dtype: DamageTypeId,
    pub amount: f32,
    /// Specific part, or a random one by coverage.
    pub part: Option<usize>,
    /// Never destroys parts nor takes vital parts below 1 HP (fistfights).
    pub non_lethal: bool,
    /// Random hits land on outer parts only (no internal organs).
    pub surface: bool,
}

/// Result of a hit, for logging.
#[derive(Clone, Debug)]
pub struct DamageResult {
    pub part: usize,
    pub dealt: f32,
    pub destroyed: bool,
    pub killed: bool,
}

/// Damages a pawn. Returns what happened (None if it has no health).
pub fn apply_damage(world: &mut World, pawn: Entity, info: &DamageInfo) -> Option<DamageResult> {
    let defs = defs(world);
    let body = body_of(&defs, world, pawn)?;
    let dt = &defs.damage_types[info.dtype];
    let mut h = world.get_cloned::<Health>(pawn)?;
    let part = match info.part {
        Some(p) if p < body.parts.len() && !is_missing(&defs, body, &h, p) => p,
        _ => {
            let mut rng = world.res_mut::<SimRng>();
            random_hit_part(&defs, body, &h, &mut rng, info.surface)
        }
    };
    let hp = part_hp(&defs, body, &h, part);
    let pd = &body.parts[part];
    let spare = info.non_lethal && (pd.has_tag("vital") || pd.kills);
    let can_destroy = dt.destroys && !info.non_lethal;
    let mut dealt = info.amount.min(hp);
    if (!can_destroy || spare) && dealt >= hp {
        dealt = (hp - 1.0).max(0.0);
    }
    let mut destroyed = false;
    if dealt > 0.0 {
        // Merge into an existing fresh injury of the same kind on the part.
        match h.hediffs.iter_mut().find(|x| x.def == dt.injury && x.part() == Some(part) && x.tended.is_none()) {
            Some(x) => {
                x.severity += dealt;
                x.peak = x.peak.max(x.severity);
            }
            None => h.hediffs.push(Hediff::new(dt.injury, Some(part), dealt)),
        }
        if can_destroy && dealt >= hp {
            destroy_part(&defs, body, &mut h, part);
            destroyed = true;
        }
    }
    recompute(&defs, body, &mut h);
    world.insert(pawn, h);
    let killed = destroyed && pd.kills;
    if killed {
        kill(world, pawn, &format!("destroyed {}", pd.label));
    } else {
        check_state(world, pawn);
    }
    Some(DamageResult { part, dealt, destroyed, killed })
}

/// Replaces a part's (and its children's) injuries with a missing-part entry.
fn destroy_part(defs: &Defs, body: &BodyDef, h: &mut Health, part: usize) {
    h.hediffs.retain(|x| match x.part() {
        Some(p) => p != part && !body.is_ancestor(part, p),
        None => true,
    });
    if let Some(m) = defs.health_ids().missing {
        h.hediffs.push(Hediff::new(m, Some(part), body.parts[part].hp));
    }
}

/// Adds (or worsens) a hediff. Severity defaults to the def's `initial`.
pub fn add_hediff(world: &mut World, pawn: Entity, def: HediffId, part: Option<usize>, severity: Option<f32>) {
    let defs = defs(world);
    let Some(body) = body_of(&defs, world, pawn) else { return };
    let d = &defs.hediffs[def];
    let sev = severity.unwrap_or(d.initial.max(0.01));
    let Some(mut h) = world.get_cloned::<Health>(pawn) else { return };
    if let Some(p) = part
        && is_missing(&defs, body, &h, p)
    {
        return;
    }
    match h.hediffs.iter_mut().find(|x| x.def == def && x.part() == part) {
        Some(x) if d.kind != HediffKind::Injury || x.tended.is_none() => {
            x.severity += sev;
            if let Some(m) = d.max_severity {
                x.severity = x.severity.min(m);
            }
            x.peak = x.peak.max(x.severity);
        }
        _ => h.hediffs.push(Hediff::new(def, part, sev)),
    }
    recompute(&defs, body, &mut h);
    world.insert(pawn, h);
    check_state(world, pawn);
}

/// Death and downed checks after health changed.
pub fn check_state(world: &mut World, pawn: Entity) {
    let defs = defs(world);
    let Some(body) = body_of(&defs, world, pawn) else { return };
    let Some(h) = world.get_cloned::<Health>(pawn) else { return };
    // Death.
    for x in &h.hediffs {
        let d = &defs.hediffs[x.def];
        if d.lethal_at.is_some_and(|l| x.severity >= l) {
            kill(world, pawn, &d.label.to_lowercase());
            return;
        }
    }
    for (id, cd) in defs.capacities.iter() {
        if cd.vital && h.capacity(id) <= 0.0 {
            kill(world, pawn, &format!("no {}", cd.label.to_lowercase()));
            return;
        }
    }
    // Downed.
    let mut downed = h.pain > body.pain_shock;
    for (id, cd) in defs.capacities.iter() {
        if cd.downed_below.is_some_and(|t| h.capacity(id) < t) {
            downed = true;
        }
    }
    if downed != h.downed {
        world.get_mut::<Health>(pawn).unwrap().downed = downed;
        let name = jobs::pawn_name(world, pawn);
        if downed {
            jobs::interrupt(world, pawn, "downed");
            let why = if h.pain > body.pain_shock { " (pain shock)" } else { "" };
            jobs::log_about(world, LogKind::Bad, pawn, format!("{name} is down{why}."));
        } else {
            jobs::interrupt(world, pawn, "got up");
            jobs::log_about(world, LogKind::Good, pawn, format!("{name} is back on their feet."));
        }
    }
}

/// Ends a pawn's life: leaves a corpse where it fell.
pub fn kill(world: &mut World, pawn: Entity, cause: &str) {
    if !world.has::<Pawn>(pawn) {
        return;
    }
    jobs::interrupt(world, pawn, "died");
    let name = jobs::pawn_name(world, pawn);
    jobs::log_about(world, LogKind::Bad, pawn, format!("{name} has died ({cause})."));
    crate::death::on_death(world, pawn, cause);
}

const HEALTH_INTERVAL: u64 = 60;

/// Per-pawn data gathered from the stat context before updating.
struct Gathered {
    triggers: Vec<(HediffId, f32)>,
    in_bed: bool,
    bed_factor: f32,
    filth: f32,
}

fn gather(world: &World, defs: &Defs, pawn: Entity) -> Option<Gathered> {
    stats::with_ctx(world, pawn, |ctx| {
        let mut triggers = Vec::new();
        for (id, d) in defs.hediffs.iter() {
            let Some(t) = &d.trigger else { continue };
            let rate = if stats::eval(&t.when, ctx) {
                match &t.curve {
                    Some((f, c)) => {
                        let v = if ctx.map.in_bounds(ctx.pos) { ctx.env.get(*f, ctx.map.idx(ctx.pos)) } else { 0.0 };
                        eval_curve(c, v)
                    }
                    None => t.gain_per_day,
                }
            } else {
                -t.loss_per_day
            };
            triggers.push((id, rate));
        }
        let in_bed = stats::eval(&Condition::InBed, ctx);
        let bed_factor = if in_bed {
            ctx.map
                .get(ctx.pos)
                .and_then(|c| c.building)
                .and_then(|b| world.get::<Building>(b).map(|bb| defs.buildings[bb.def].heal_factor))
                .unwrap_or(1.0)
        } else {
            1.0
        };
        let filth = defs
            .core
            .field_filth
            .filter(|_| ctx.map.in_bounds(ctx.pos))
            .map(|f| ctx.env.get(f, ctx.map.idx(ctx.pos)))
            .unwrap_or(0.0);
        Gathered { triggers, in_bed, bed_factor, filth }
    })
}

/// Progresses every pawn's hediffs.
pub fn sys_health(world: &mut World) {
    let tick = world.res::<Clock>().tick;
    if !tick.is_multiple_of(HEALTH_INTERVAL) {
        return;
    }
    let defs = defs(world);
    let ids = defs.health_ids();
    let dt = HEALTH_INTERVAL as f32 / TICKS_PER_DAY as f32;
    let pawns: Vec<Entity> = world.read::<Health>().entities().to_vec();
    for pawn in pawns {
        if !world.is_alive(pawn) || !world.has::<Pawn>(pawn) {
            continue;
        }
        let Some(body) = body_of(&defs, world, pawn) else { continue };
        let Some(g) = gather(world, &defs, pawn) else { continue };
        let Some(mut h) = world.get_cloned::<Health>(pawn) else { continue };
        let heal_stat = stats::stat_by_name(world, pawn, "injury_heal").unwrap_or(1.0);
        let immunity_stat = stats::stat_by_name(world, pawn, "immunity_gain").unwrap_or(1.0);
        let rest_mult = if g.in_bed { 1.5 * g.bed_factor } else { 1.0 };
        let mut new: Vec<Hediff> = Vec::new();
        let mut scars: Vec<(usize, f32)> = Vec::new();
        let mut infections: Vec<(HediffId, usize)> = Vec::new();
        let mut spawns: Vec<(HediffId, String, f32)> = Vec::new();
        let bleeding = h.bleeding;
        {
            let mut rng = world.res_mut::<SimRng>();
            for x in &mut h.hediffs {
                let d = &defs.hediffs[x.def];
                let prev_age = x.age;
                x.age = x.age.saturating_add(HEALTH_INTERVAL as u32);
                if x.tended.is_some() && d.tend_hours.is_some() {
                    x.tend_left = x.tend_left.saturating_sub(HEALTH_INTERVAL as u32);
                    if x.tend_left == 0 {
                        x.tended = None;
                    }
                }
                let q = x.tended.unwrap_or(0.0);
                match d.kind {
                    HediffKind::Injury => {
                        // Infection roll once the wound is half a day old.
                        let half_day = TICKS_PER_DAY as u32 / 2;
                        if !x.infection_rolled && prev_age < half_day && x.age >= half_day {
                            x.infection_rolled = true;
                            if let Some((inf, ch)) = d.infection {
                                let tend = if x.tended.is_some() { 1.0 - 0.85 * q } else { 1.0 };
                                let p = ch * tend * (1.0 + g.filth.min(3.0) * 0.5);
                                if let Some(part) = x.part()
                                    && rng.chance(p)
                                {
                                    infections.push((inf, part));
                                }
                            }
                        }
                        let tend_mult = 1.0 + (d.tended_heal - 1.0) * q;
                        let heal = d.heal_per_day * tend_mult * x.heal_mult * heal_stat * rest_mult * dt;
                        x.severity -= heal;
                        if x.severity <= 0.0
                            && d.scar_chance > 0.0
                            && rng.chance(d.scar_chance)
                            && let Some(p) = x.part()
                        {
                            scars.push((p, (x.peak * 0.25).max(1.0)));
                        }
                    }
                    HediffKind::Illness => {
                        if let Some(im) = &d.immunity {
                            x.immunity = (x.immunity + im.immunity_per_day * immunity_stat * rest_mult.min(1.5) * dt).min(1.0);
                            if x.immunity >= 1.0 {
                                x.severity -= im.recover_per_day * dt;
                            } else {
                                let f = 1.0 + (im.tended_factor - 1.0) * q;
                                x.severity += im.severity_per_day * f * dt;
                            }
                        } else {
                            x.severity -= d.heal_per_day * dt;
                        }
                    }
                    HediffKind::Condition => {
                        if Some(x.def) == ids.blood_loss {
                            if bleeding > 0.0 {
                                x.severity += bleeding * dt;
                            } else {
                                x.severity -= d.heal_per_day * rest_mult * dt;
                            }
                        } else if d.trigger.is_none() {
                            x.severity -= d.heal_per_day * dt;
                        }
                    }
                    HediffKind::MissingPart | HediffKind::Scar => {}
                }
                if let Some(m) = d.max_severity {
                    x.severity = x.severity.min(m);
                }
                x.peak = x.peak.max(x.severity);
                if let Some(st) = d.stage(x.severity)
                    && let Some((inj, tag, per_day)) = &st.spawns
                    && rng.chance(per_day * dt)
                {
                    spawns.push((*inj, tag.clone(), st.spawn_severity));
                }
            }
            // Exposure triggers.
            for (id, rate) in &g.triggers {
                let d = &defs.hediffs[*id];
                match h.hediffs.iter_mut().find(|x| x.def == *id) {
                    Some(x) => {
                        x.severity += rate * dt;
                        if let Some(m) = d.max_severity {
                            x.severity = x.severity.min(m);
                        }
                        x.peak = x.peak.max(x.severity);
                    }
                    None if *rate > 0.0 => new.push(Hediff::new(*id, None, rate * dt)),
                    None => {}
                }
            }
            // Blood loss starts with bleeding.
            if let Some(bl) = ids.blood_loss
                && bleeding > 0.0
                && !h.has(bl)
            {
                new.push(Hediff::new(bl, None, bleeding * dt));
            }
            for (inj, tag, sev) in &spawns {
                if let Some(p) = random_tagged_part(&defs, body, &h, tag, &mut rng) {
                    new.push(Hediff::new(*inj, Some(p), *sev));
                }
            }
        }
        h.hediffs.retain(|x| x.severity > 0.0 || matches!(defs.hediffs[x.def].kind, HediffKind::MissingPart));
        h.hediffs.extend(new);
        if let Some(sc) = ids.scar {
            for (p, s) in scars {
                h.hediffs.push(Hediff::new(sc, Some(p), s));
            }
        }
        let mut infected = Vec::new();
        for (inf, part) in infections {
            if !h.hediffs.iter().any(|x| x.def == inf && x.part() == Some(part)) {
                h.hediffs.push(Hediff::new(inf, Some(part), defs.hediffs[inf].initial.max(0.01)));
                infected.push(part);
            }
        }
        recompute(&defs, body, &mut h);
        world.insert(pawn, h);
        if !infected.is_empty() {
            let name = jobs::pawn_name(world, pawn);
            for p in infected {
                jobs::log_about(world, LogKind::Bad, pawn, format!("{name}'s {} is infected.", body.parts[p].label));
            }
        }
        check_state(world, pawn);
    }
}

/// Total HP-weighted health summary 0..1 (for overheads and lists).
pub fn summary(defs: &Defs, h: &Health) -> f32 {
    let mut s = 1.0f32;
    for (id, _) in defs.capacities.iter() {
        s = s.min(h.capacity(id));
    }
    s.clamp(0.0, 1.0)
}

/// Hediffs grouped for display: (part label or "Whole body", entries).
pub fn grouped<'a>(defs: &Defs, body: &'a BodyDef, h: &'a Health) -> Vec<(String, Vec<&'a Hediff>)> {
    let mut out: Vec<(Option<usize>, Vec<&Hediff>)> = Vec::new();
    for x in &h.hediffs {
        if !defs.hediffs[x.def].visible {
            continue;
        }
        match out.iter_mut().find(|(p, _)| *p == x.part()) {
            Some((_, v)) => v.push(x),
            None => out.push((x.part(), vec![x])),
        }
    }
    out.sort_by_key(|(p, _)| p.map_or(-1, |p| p as i32));
    out.into_iter().map(|(p, v)| (p.map_or("Whole body".to_string(), |p| body.parts[p].label.clone()), v)).collect()
}

/// Display line for a hediff: "Cut 6.0" / "Flu (major) 42%".
pub fn describe(defs: &Defs, x: &Hediff) -> String {
    let d = &defs.hediffs[x.def];
    let stage = d.stage(x.severity).map(|s| s.label.as_str()).unwrap_or("");
    let mut s = d.label.clone();
    if !stage.is_empty() {
        s.push_str(&format!(" ({stage})"));
    }
    match d.kind {
        HediffKind::Injury | HediffKind::Scar => s.push_str(&format!(" {:.0}", x.severity)),
        HediffKind::MissingPart => {}
        _ => s.push_str(&format!(" {:.0}%", x.severity * 100.0)),
    }
    if d.kind == HediffKind::Illness && d.immunity.is_some() {
        s.push_str(&format!(", immunity {:.0}%", x.immunity * 100.0));
    }
    if d.tendable {
        s.push_str(match x.tended {
            Some(_) => ", tended",
            None => ", untended",
        });
    }
    s
}

/// Position helper used by others.
pub fn pos(world: &World, e: Entity) -> Option<P3> {
    world.get::<Position>(e).map(|p| p.0)
}

/// Filth at a cell (0 if no filth field).
pub fn filth_at(world: &World, p: P3) -> f32 {
    let defs = defs(world);
    let map = world.res::<Map>();
    match defs.core.field_filth {
        Some(f) if map.in_bounds(p) => world.res::<Env>().get(f, map.idx(p)),
        _ => 0.0,
    }
}
