use std::path::Path;

use ecs::Entity;
use sim::components::{Item, Pawn, Position};
use sim::death::Corpse;
use sim::defs::Defs;
use sim::health::{self, DamageInfo, Health};
use sim::jobs;
use sim::resources::{SimMeta, TICKS_PER_DAY, TICKS_PER_HOUR};
use sim::worldgen::WorldGenParams;
use sim::{P3, Sim};

fn defs() -> Defs {
    Defs::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/defs"), None).expect("defs load")
}

fn sim() -> Sim {
    Sim::new(defs(), 33, &WorldGenParams { width: 48, height: 48, levels: 5 })
}

fn pawns(s: &Sim) -> Vec<Entity> {
    s.world.read::<Pawn>().entities().to_vec()
}

fn part(s: &Sim, name: &str) -> usize {
    s.defs().bodies.items[0].part(name).expect("part")
}

fn hit(s: &mut Sim, p: Entity, dtype: &str, amount: f32, part_name: Option<&str>, non_lethal: bool) {
    let d = s.defs();
    let info = DamageInfo {
        dtype: d.damage_types.expect(dtype),
        amount,
        part: part_name.map(|n| part(s, n)),
        non_lethal,
        surface: false,
    };
    health::apply_damage(&mut s.world, p, &info).expect("has health");
}

fn health_of(s: &Sim, p: Entity) -> Health {
    s.world.get_cloned::<Health>(p).expect("health")
}

#[test]
fn colonists_start_healthy() {
    let s = sim();
    let d = s.defs();
    assert_eq!(pawns(&s).len(), 4, "three colonists and a relative");
    for p in pawns(&s) {
        let h = health_of(&s, p);
        assert!(h.hediffs.is_empty());
        for (id, _) in d.capacities.iter() {
            assert!((h.capacity(id) - 1.0).abs() < 1e-4, "{} not full", d.capacities[id].name);
        }
        assert!(!h.downed);
    }
    assert_eq!(d.bodies.items[0].parts.len(), 50);
}

#[test]
fn cuts_bleed_and_tending_stops_it() {
    let mut s = sim();
    // Nobody doctors here, so the wound keeps bleeding.
    let doctor = s.defs().work_types.expect("doctor");
    for q in pawns(&s) {
        s.world.get_mut::<sim::components::WorkPriorities>(q).unwrap().0[doctor.index()] = 0;
    }
    let p = pawns(&s)[0];
    hit(&mut s, p, "cut", 10.0, Some("left_leg"), false);
    let h = health_of(&s, p);
    assert!(h.bleeding > 0.0);
    assert!(h.pain > 0.0);
    let moving = s.defs().capacities.expect("moving");
    assert!(h.capacity(moving) < 1.0, "a cut leg slows walking");
    // Blood loss builds up over a few hours.
    s.run_ticks(3 * TICKS_PER_HOUR);
    let bl = s.defs().hediffs.expect("blood_loss");
    let h = health_of(&s, p);
    assert!(h.has(bl), "blood loss from bleeding");
    // Tend it (pawn tends itself here) and the bleeding stops.
    jobs::interrupt(&mut s.world, p, "test");
    sim::medical::complete_tend(&mut s.world, p, p);
    let h = health_of(&s, p);
    assert_eq!(h.bleeding, 0.0);
    assert!(h.hediffs.iter().filter(|x| s.defs().hediffs[x.def].tendable).all(|x| x.tended.is_some()));
}

#[test]
fn destroying_the_brain_kills_and_leaves_a_corpse() {
    let mut s = sim();
    let p = pawns(&s)[1];
    let name = jobs::pawn_name(&s.world, p);
    hit(&mut s, p, "cut", 100.0, Some("brain"), false);
    assert!(!s.world.is_alive(p));
    assert_eq!(pawns(&s).len(), 3);
    let corpses: Vec<Corpse> = s.world.read::<Corpse>().values().to_vec();
    assert_eq!(corpses.len(), 1);
    assert_eq!(corpses[0].name, name);
    // Others grieve.
    let died = s.defs().thoughts.expect("colonist_died");
    let grieving = pawns(&s)
        .into_iter()
        .filter(|o| {
            s.world.get::<sim::components::Mood>(*o).is_some_and(|m| m.memories.iter().any(|x| x.about.as_deref() == Some(&name)))
        })
        .count();
    assert!(grieving >= 1, "someone remembers {name} ({died:?})");
}

#[test]
fn fist_fights_never_destroy_vital_parts() {
    let mut s = sim();
    let p = pawns(&s)[0];
    for _ in 0..10 {
        hit(&mut s, p, "blunt", 50.0, Some("brain"), true);
        hit(&mut s, p, "blunt", 50.0, Some("heart"), true);
    }
    assert!(s.world.is_alive(p), "non-lethal damage must not kill");
    let d = s.defs();
    let body = &d.bodies.items[0];
    let h = health_of(&s, p);
    assert!(health::part_hp(&d, body, &h, part(&s, "brain")) >= 1.0);
    assert!(h.downed, "but it hurts enough to go down");
}

#[test]
fn lost_hand_takes_its_fingers_and_lowers_manipulation() {
    let mut s = sim();
    let p = pawns(&s)[0];
    hit(&mut s, p, "cut", 3.0, Some("left_thumb"), false);
    hit(&mut s, p, "cut", 100.0, Some("left_hand"), false);
    let d = s.defs();
    let body = &d.bodies.items[0];
    let h = health_of(&s, p);
    assert!(health::is_missing(&d, body, &h, part(&s, "left_thumb")));
    assert!(!h.hediffs.iter().any(|x| x.part() == Some(part(&s, "left_thumb"))), "thumb injury folded in");
    let manip = d.capacities.expect("manipulation");
    assert!(h.capacity(manip) < 0.85 && h.capacity(manip) > 0.4, "manipulation {}", h.capacity(manip));
}

#[test]
fn downed_colonist_is_rescued_to_bed_and_tended() {
    let mut s = sim();
    let d = s.defs();
    let start = s.world.res::<SimMeta>().start;
    let bed_pos = (0..8)
        .map(|i| start.offset(-4 + i, -5, 0))
        .find(|p| s.world.res::<sim::map::Map>().is_walkable(*p) && s.world.res::<sim::map::Map>().cell(*p).item.is_none())
        .expect("bed spot");
    jobs::spawn_building(&mut s.world, d.buildings.expect("medical_bed"), 0, bed_pos);
    let p = pawns(&s)[0];
    // Heavy bleeding wounds, then enough blood loss to go down.
    hit(&mut s, p, "cut", 8.0, Some("left_arm"), false);
    hit(&mut s, p, "cut", 8.0, Some("right_leg"), false);
    health::add_hediff(&mut s.world, p, d.hediffs.expect("blood_loss"), None, Some(0.62));
    assert!(health_of(&s, p).downed);
    s.run_ticks(TICKS_PER_HOUR * 4);
    assert!(s.world.is_alive(p), "rescued in time");
    assert_eq!(s.world.get::<Position>(p).map(|x| x.0), Some(bed_pos), "carried to the medical bed");
    let h = health_of(&s, p);
    assert_eq!(h.bleeding, 0.0, "wounds tended");
}

#[test]
fn flu_runs_its_course_with_care() {
    let mut s = sim();
    let d = s.defs();
    let p = pawns(&s)[2];
    let flu = d.hediffs.expect("flu");
    health::add_hediff(&mut s.world, p, flu, None, None);
    s.run_ticks(TICKS_PER_DAY * 7);
    assert!(s.world.is_alive(p), "flu should rarely kill a tended colonist");
    assert!(!health_of(&s, p).has(flu), "recovered after a week");
}

#[test]
fn hypothermia_builds_in_the_cold_and_fades_in_warmth() {
    let d = defs();
    let hyp = d.hediffs.expect("hypothermia");
    let t = d.hediffs[hyp].trigger.as_ref().unwrap();
    let curve = &t.curve.as_ref().unwrap().1;
    assert!(sim::defs::eval_curve(curve, -20.0) > 1.0);
    assert!(sim::defs::eval_curve(curve, 20.0) < 0.0);
}

#[test]
fn corpses_get_buried_in_graves() {
    let mut s = sim();
    let d = s.defs();
    let start = s.world.res::<SimMeta>().start;
    let p = pawns(&s)[3];
    hit(&mut s, p, "cut", 100.0, Some("heart"), false);
    assert!(!s.world.is_alive(p));
    let spot: P3 = (0..10)
        .map(|i| start.offset(-5 + i, 5, 0))
        .find(|q| s.world.res::<sim::map::Map>().is_walkable(*q) && s.world.res::<sim::map::Map>().cell(*q).item.is_none())
        .expect("grave spot");
    let grave = d.buildings.expect("grave");
    jobs::spawn_building(&mut s.world, grave, 0, spot);
    s.run_ticks(TICKS_PER_HOUR * 6);
    assert_eq!(s.world.read::<Corpse>().len(), 0, "corpse buried");
    let graves: Vec<_> = s.world.read::<sim::death::Grave>().values().to_vec();
    assert!(graves.iter().any(|g| g.occupant.is_some()));
    let corpse_items = s.world.read::<Item>().iter().filter(|(_, it)| it.def == d.items.expect("corpse")).count();
    assert_eq!(corpse_items, 0);
}

#[test]
fn save_load_keeps_health_and_social_state() {
    let mut a = sim();
    let p = pawns(&a)[0];
    hit(&mut a, p, "cut", 6.0, Some("left_arm"), false);
    a.run_ticks(TICKS_PER_HOUR * 2);
    let bytes = a.save_bytes().unwrap();
    let mut b = sim();
    b.load_bytes(&bytes).unwrap();
    assert_eq!(a.state_hash(), b.state_hash());
    a.run_ticks(2000);
    b.run_ticks(2000);
    assert_eq!(a.state_hash(), b.state_hash(), "diverged after load");
}
