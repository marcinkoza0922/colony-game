//! Medicine: doctors tend wounds and illnesses, rescue downed colonists to
//! bed and feed patients. Patients rest in bed until they are well.

use std::rc::Rc;

use ecs::{Entity, World};
use serde::{Deserialize, Serialize};

use crate::components::*;
use crate::defs::*;
use crate::geom::P3;
use crate::health::{self, Health};
use crate::jobs::{self, Job, JobKind, Toil, WorkTarget};
use crate::map::Map;
use crate::path::Goal;
use crate::resources::*;
use crate::rng::SimRng;
use crate::stats;

/// On a pawn that is carrying another (downed) pawn.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct CarryingPawn(pub Entity);

/// Base work for one tending session.
pub const TEND_WORK: f32 = 300.0;
/// Quality cap and potency when tending without medicine.
const NO_MEDICINE: MedicineDef = MedicineDef { potency: 0.5, max_quality: 0.7, heal_mult: 1.0 };

fn defs(world: &World) -> Rc<Defs> {
    world.res::<Rc<Defs>>().clone()
}

fn pos_of(world: &World, e: Entity) -> Option<P3> {
    world.get::<Position>(e).map(|p| p.0)
}

/// Keeps carried pawns at their carrier's position.
pub fn sync_carried(world: &mut World) {
    let pairs: Vec<(Entity, Entity)> = world.read::<CarryingPawn>().iter().map(|(c, p)| (c, p.0)).collect();
    for (carrier, carried) in pairs {
        match (pos_of(world, carrier), world.is_alive(carried)) {
            (Some(p), true) => {
                world.insert(carried, Position(p));
            }
            _ => {
                world.remove::<CarryingPawn>(carrier);
            }
        }
    }
}

/// Is someone carrying this pawn?
pub fn carrier_of(world: &World, pawn: Entity) -> Option<Entity> {
    world.read::<CarryingPawn>().iter().find(|(_, c)| c.0 == pawn).map(|(e, _)| e)
}

/// Breaks any carry involving `e` (as carrier or carried).
pub fn release_carry(world: &mut World, e: Entity) {
    world.remove::<CarryingPawn>(e);
    if let Some(c) = carrier_of(world, e) {
        world.remove::<CarryingPawn>(c);
        jobs::interrupt(world, c, "patient gone");
    }
}

/// Lying down as a patient (in a bed or not) and ready for a doctor.
pub fn resting(world: &World, pawn: Entity) -> bool {
    world.get::<CurrentJob>(pawn).is_some_and(|j| matches!(j.0.toils.get(j.0.step), Some(Toil::Rest { .. })))
}

pub fn in_bed(world: &World, pawn: Entity) -> bool {
    stats::with_ctx(world, pawn, |ctx| stats::eval(&Condition::InBed, ctx)).unwrap_or(false)
}

/// Lying on a bed cell (even if not resting yet).
fn on_bed_cell(world: &World, pawn: Entity) -> Option<Entity> {
    let p = pos_of(world, pawn)?;
    let map = world.res::<Map>();
    let c = map.get(p)?;
    (c.building_kind == Some(BuildingKind::Bed)).then_some(c.building).flatten()
}

/// A bed for a patient: its own, else a free one (medical beds first).
pub fn find_patient_bed(world: &World, patient: Entity, near: P3, actor: Entity) -> Option<(Entity, P3)> {
    let defs = defs(world);
    let beds: Vec<(Entity, Option<Entity>, P3, bool)> = {
        let beds = world.read::<Bed>();
        beds.iter()
            .filter_map(|(e, b)| {
                let p = pos_of(world, e)?;
                let medical = world.get::<Building>(e).is_some_and(|bb| defs.buildings[bb.def].medical);
                Some((e, b.owner, p, medical))
            })
            .collect()
    };
    let res = world.res::<Reservations>();
    let occupied = |bp: P3| {
        world.read::<Pawn>().entities().iter().any(|o| *o != patient && pos_of(world, *o) == Some(bp))
    };
    let free = |e: Entity, owner: Option<Entity>, bp: P3| {
        (owner.is_none() || owner == Some(patient) || owner.is_some_and(|o| !world.is_alive(o)))
            && !res.blocked(ResKey::Entity(e), actor)
            && !occupied(bp)
    };
    // Medical beds, then the patient's own, then any free bed.
    let pick = beds
        .iter()
        .filter(|(e, o, p, m)| *m && free(*e, *o, *p))
        .min_by_key(|(_, _, p, _)| p.manhattan(near))
        .or_else(|| beds.iter().find(|(e, o, p, _)| *o == Some(patient) && free(*e, *o, *p)))
        .or_else(|| beds.iter().filter(|(e, o, p, _)| free(*e, *o, *p)).min_by_key(|(_, _, p, _)| p.manhattan(near)));
    pick.map(|(e, _, p, _)| (*e, *p))
}

/// Patient AI: go to bed and rest.
pub fn rest_job(world: &World, pawn: Entity, pos: P3) -> Option<Job> {
    if let Some(b) = on_bed_cell(world, pawn) {
        return Some(Job::new(JobKind::Rest, vec![Toil::Rest { bed: Some(b) }]));
    }
    let Some((bed, bp)) = find_patient_bed(world, pawn, pos, pawn) else {
        // No bed free: lie down here and wait for a doctor.
        return Some(Job::new(JobKind::Rest, vec![Toil::Rest { bed: None }]));
    };
    {
        let mut r = world.res_mut::<Reservations>();
        if r.blocked(ResKey::Entity(bed), pawn) {
            return None;
        }
        r.try_reserve(ResKey::Entity(bed), pawn);
    }
    Some(Job::new(JobKind::Rest, vec![Toil::GoTo(Goal::On(bp)), Toil::Rest { bed: Some(bed) }]))
}

pub fn downed_job() -> Job {
    Job::new(JobKind::Downed, vec![Toil::Wait { ticks: 250 }])
}

/// Doctor work: rescue the downed, tend the hurt, feed bedridden patients.
pub fn doctor_job(world: &World, defs: &Defs, pawn: Entity, pos: P3) -> Option<Job> {
    rescue_job(world, pawn, pos)
        .or_else(|| tend_job(world, defs, pawn, pos))
        .or_else(|| feed_job(world, defs, pawn, pos))
}

fn patients(world: &World, doctor: Entity) -> Vec<(Entity, P3)> {
    let res = world.res::<Reservations>();
    let hs = world.read::<Health>();
    hs.iter()
        .filter(|(e, _)| *e != doctor && !res.blocked(ResKey::Entity(*e), doctor))
        .filter_map(|(e, _)| pos_of(world, e).map(|p| (e, p)))
        .collect()
}

fn rescue_job(world: &World, pawn: Entity, pos: P3) -> Option<Job> {
    let mut cands: Vec<(Entity, P3)> = patients(world, pawn)
        .into_iter()
        .filter(|(e, _)| health::is_downed(world, *e) && carrier_of(world, *e).is_none() && on_bed_cell(world, *e).is_none())
        .collect();
    cands.sort_by_key(|(e, p)| (p.manhattan(pos), *e));
    for (patient, ppos) in cands {
        let Some((bed, bp)) = find_patient_bed(world, patient, ppos, pawn) else { continue };
        let mut r = world.res_mut::<Reservations>();
        if r.blocked(ResKey::Entity(patient), pawn) || r.blocked(ResKey::Entity(bed), pawn) {
            continue;
        }
        r.try_reserve(ResKey::Entity(patient), pawn);
        r.try_reserve(ResKey::Entity(bed), pawn);
        return Some(Job::new(
            JobKind::Rescue,
            vec![
                Toil::GoTo(Goal::Near(ppos)),
                Toil::CarryPawn { pawn: patient },
                Toil::GoTo(Goal::On(bp)),
                Toil::PlacePawn { pawn: patient },
            ],
        ));
    }
    None
}

/// Best medicine item reachable (highest potency), if any.
fn find_medicine(world: &World, defs: &Defs, pawn: Entity, pos: P3) -> Option<(Entity, P3)> {
    let itemst = world.read::<Item>();
    let posst = world.read::<Position>();
    let res = world.res::<Reservations>();
    itemst
        .iter()
        .filter(|(e, it)| defs.items[it.def].medicine.is_some() && !res.blocked(ResKey::Entity(*e), pawn))
        .filter_map(|(e, it)| posst.get(e).map(|p| (e, p.0, defs.items[it.def].medicine.as_ref().unwrap().potency)))
        .max_by(|a, b| a.2.total_cmp(&b.2).then_with(|| b.1.manhattan(pos).cmp(&a.1.manhattan(pos))))
        .map(|(e, p, _)| (e, p))
}

fn tend_job(world: &World, defs: &Defs, pawn: Entity, pos: P3) -> Option<Job> {
    let mut cands: Vec<(Entity, P3)> = patients(world, pawn)
        .into_iter()
        .filter(|(e, _)| {
            world.get::<Health>(*e).is_some_and(|h| h.needs_tending(defs))
                && (health::is_downed(world, *e) || resting(world, *e))
                && carrier_of(world, *e).is_none()
        })
        .collect();
    // Self-tend when hurt and nobody else is around to do it.
    let self_tend = world.get::<Health>(pawn).is_some_and(|h| h.needs_tending(defs) && h.bleeding > 0.0);
    if self_tend {
        cands.insert(0, (pawn, pos));
    }
    cands.sort_by_key(|(e, p)| (*e != pawn, p.manhattan(pos), *e));
    let (patient, ppos) = *cands.first()?;
    let med = find_medicine(world, defs, pawn, pos);
    let mut keys = vec![ResKey::Entity(patient)];
    if let Some((m, _)) = med {
        keys.push(ResKey::Entity(m));
    }
    {
        let mut r = world.res_mut::<Reservations>();
        if keys.iter().any(|k| r.blocked(*k, pawn)) {
            return None;
        }
        for k in &keys {
            r.try_reserve(*k, pawn);
        }
    }
    let mut toils = Vec::new();
    if let Some((m, mp)) = med {
        toils.push(Toil::GoTo(Goal::On(mp)));
        toils.push(Toil::PickUp { item: m, count: 1 });
    }
    if patient != pawn {
        toils.push(Toil::GoTo(Goal::Near(ppos)));
    }
    toils.push(Toil::Work(WorkTarget::Tend(patient)));
    Some(Job::new(JobKind::Tend, toils))
}

fn feed_job(world: &World, defs: &Defs, pawn: Entity, pos: P3) -> Option<Job> {
    let food = defs.needs.iter().find(|(_, n)| n.role == Some(NeedRole::Food)).map(|(i, _)| i)?;
    let hungry = patients(world, pawn).into_iter().find(|(e, _)| {
        health::is_downed(world, *e)
            && carrier_of(world, *e).is_none()
            && world.get::<Needs>(*e).and_then(|n| n.values.get(food.index()).copied()).is_some_and(|v| v < 0.3)
    });
    let (patient, ppos) = hungry?;
    let (item, ipos) = {
        let itemst = world.read::<Item>();
        let posst = world.read::<Position>();
        let res = world.res::<Reservations>();
        itemst
            .iter()
            .filter(|(e, it)| {
                let d = &defs.items[it.def];
                d.nutrition > 0.0 && d.category != ItemCategory::Seed && !res.blocked(ResKey::Entity(*e), pawn)
            })
            .filter_map(|(e, it)| {
                posst.get(e).map(|p| (e, p.0, defs.items[it.def].category == ItemCategory::Meal))
            })
            .min_by_key(|(e, p, meal)| (!meal, p.manhattan(pos), *e))
            .map(|(e, p, _)| (e, p))?
    };
    {
        let mut r = world.res_mut::<Reservations>();
        if r.blocked(ResKey::Entity(patient), pawn) || r.blocked(ResKey::Entity(item), pawn) {
            return None;
        }
        r.try_reserve(ResKey::Entity(patient), pawn);
        r.try_reserve(ResKey::Entity(item), pawn);
    }
    let it = world.get::<Item>(item)?.clone();
    let nut = defs.items[it.def].nutrition.max(0.01);
    let count = ((0.9 / nut).ceil() as u32).clamp(1, it.count);
    Some(Job::new(
        JobKind::Feed,
        vec![
            Toil::GoTo(Goal::On(ipos)),
            Toil::PickUp { item, count },
            Toil::GoTo(Goal::Near(ppos)),
            Toil::Feed { patient },
        ],
    ))
}

// --------------------------------------------------------------------- toils

pub fn toil_carry(world: &mut World, carrier: Entity, patient: Entity) -> Result<(), String> {
    let (Some(a), Some(b)) = (pos_of(world, carrier), pos_of(world, patient)) else { return Err("patient gone".into()) };
    if !a.is_adjacent8(b) && a != b {
        return Err("not at patient".into());
    }
    if !health::is_downed(world, patient) {
        return Err("patient got up".into());
    }
    if carrier_of(world, patient).is_some() {
        return Err("already carried".into());
    }
    jobs::interrupt(world, patient, "picked up");
    world.insert(carrier, CarryingPawn(patient));
    world.insert(patient, Position(a));
    Ok(())
}

pub fn toil_place(world: &mut World, carrier: Entity, patient: Entity) -> Result<(), String> {
    if world.get::<CarryingPawn>(carrier).map(|c| c.0) != Some(patient) {
        return Err("not carrying".into());
    }
    world.remove::<CarryingPawn>(carrier);
    if let Some(p) = pos_of(world, carrier) {
        world.insert(patient, Position(p));
    }
    // Claim the bed for the patient if it has none.
    if let Some(b) = on_bed_cell(world, patient)
        && let Some(mut bed) = world.get_mut::<Bed>(b)
        && bed.owner.is_none()
    {
        bed.owner = Some(patient);
    }
    jobs::interrupt(world, patient, "placed in bed");
    let (d, p) = (jobs::pawn_name(world, carrier), jobs::pawn_name(world, patient));
    jobs::log_about(world, LogKind::Info, carrier, format!("{d} rescued {p}."));
    Ok(())
}

pub fn toil_feed(world: &mut World, carrier: Entity, patient: Entity) -> Result<(), String> {
    let defs = defs(world);
    let Some(item) = world.get::<Inventory>(carrier).and_then(|i| i.carrying) else { return Err("no food".into()) };
    let Some(it) = world.get_cloned::<Item>(item) else { return Err("no food".into()) };
    let food = defs.needs.iter().find(|(_, n)| n.role == Some(NeedRole::Food)).map(|(i, _)| i).ok_or("no food need")?;
    let nut = defs.items[it.def].nutrition;
    if let Some(mut n) = world.get_mut::<Needs>(patient)
        && let Some(v) = n.values.get_mut(food.index())
    {
        *v = (*v + nut * it.count as f32).min(1.0);
    }
    world.get_mut::<Inventory>(carrier).unwrap().carrying = None;
    world.despawn(item);
    Ok(())
}

/// Bed rest: lie down until well; also restores rest. Ends early when
/// hungry enough to get up and eat (if able).
pub fn toil_rest(world: &mut World, pawn: Entity, job: &mut Job, bed: Option<Entity>) -> bool {
    let defs = defs(world);
    if job.ticks == 0
        && let Some(b) = bed
        && let Some(r) = world.get::<Building>(b).map(|bb| bb.rot)
    {
        world.insert(pawn, Facing(r * 2));
    }
    job.ticks += 1;
    if let Some(rest) = defs.needs.iter().find(|(_, n)| n.role == Some(NeedRole::Rest)).map(|(i, _)| i)
        && let Some(mut n) = world.get_mut::<Needs>(pawn)
        && let Some(v) = n.values.get_mut(rest.index())
    {
        *v = (*v + 1.5 / TICKS_PER_DAY as f32).min(1.0);
    }
    if !job.ticks.is_multiple_of(250) {
        return false;
    }
    let well = !world.get::<Health>(pawn).is_some_and(|h| h.wants_bed_rest(&defs));
    let hungry = defs
        .needs
        .iter()
        .find(|(_, n)| n.role == Some(NeedRole::Food))
        .and_then(|(f, _)| world.get::<Needs>(pawn).and_then(|n| n.values.get(f.index()).copied()))
        .is_some_and(|v| v < 0.2);
    well || hungry
}

/// Work parameters for tending.
pub fn tend_params(world: &World, patient: Entity) -> Result<(f32, StatId, Option<SkillId>), String> {
    let defs = defs(world);
    let h = world.get::<Health>(patient).ok_or("patient gone")?;
    if !h.needs_tending(&defs) {
        return Err("nothing to tend".into());
    }
    let speed = defs.stats.find("tend_speed").unwrap_or(defs.core.stat_work_speed);
    Ok((TEND_WORK, speed, defs.skills.find("medicine")))
}

/// Finishes a tending session: every untended condition gets tended.
pub fn complete_tend(world: &mut World, doctor: Entity, patient: Entity) {
    let defs = defs(world);
    let carried = world.get::<Inventory>(doctor).and_then(|i| i.carrying);
    let med_item = carried.and_then(|c| world.get::<Item>(c).map(|it| it.def)).filter(|d| defs.items[*d].medicine.is_some());
    let med = med_item.and_then(|d| defs.items[d].medicine.clone()).unwrap_or(NO_MEDICINE);
    let skill = stats::stat_by_name(world, doctor, "tend_quality").unwrap_or(0.5);
    let self_penalty = if doctor == patient { 0.7 } else { 1.0 };
    let roll = world.res_mut::<SimRng>().range_f32(0.85, 1.15);
    let q = (skill * med.potency * self_penalty * roll).clamp(0.05, med.max_quality);
    if let Some(mut h) = world.get_mut::<Health>(patient) {
        for x in &mut h.hediffs {
            let d = &defs.hediffs[x.def];
            if d.tendable && x.tended.is_none() {
                x.tended = Some(q);
                x.tend_left = (d.tend_hours.unwrap_or(0.0) * TICKS_PER_HOUR as f32) as u32;
                x.heal_mult = med.heal_mult;
            }
        }
    }
    if let (Some(item), Some(_)) = (carried, med_item) {
        let left = {
            let mut it = world.get_mut::<Item>(item).unwrap();
            it.count -= 1;
            it.count
        };
        if left == 0 {
            world.get_mut::<Inventory>(doctor).unwrap().carrying = None;
            world.despawn(item);
        }
    }
    if let (Some(body), Some(mut h)) = (health::body_of(&defs, world, patient), world.get_cloned::<Health>(patient)) {
        health::recompute(&defs, body, &mut h);
        world.insert(patient, h);
    }
    let (d, p) = (jobs::pawn_name(world, doctor), jobs::pawn_name(world, patient));
    let what = if doctor == patient { "their own wounds".to_string() } else { p };
    jobs::log_about(world, LogKind::Info, doctor, format!("{d} tended {what} ({:.0}%).", q * 100.0));
}
