use std::path::Path;

use sim::{Sim, defs::Defs, geom::P3, map::Map, path::{Goal, Pathfinder}, scenario, worldgen::WorldGenParams};

fn defs() -> Defs {
    Defs::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/defs"), None).expect("defs load")
}

fn small() -> WorldGenParams {
    WorldGenParams { width: 64, height: 64, levels: 5 }
}

fn run(sim: &mut Sim, ticks: u64) {
    for i in 0..ticks {
        sim.tick();
        if i % 600 == 0 {
            scenario::ensure_bills(sim);
            scenario::periodic_orders(sim);
        }
    }
}

#[test]
fn defs_load_and_resolve() {
    let d = defs();
    assert!(d.races.len() >= 3);
    let elf = d.races.expect("elf");
    assert!(d.races[elf].needs.iter().any(|n| d.needs[n.need].name == "nature"));
    let r = d.recipes.expect("cook_simple_meal");
    assert_eq!(d.buildings[d.recipes[r].bench].name, "campfire");
}

#[test]
fn defs_reject_unknown_reference() {
    let mut src = Defs::read_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/defs")).unwrap();
    let items = src.iter_mut().find(|(k, _)| *k == "items").unwrap();
    items.1.push(("bad.ron".into(), r#"[(name: "x", label: "X", sprite: "s", eat_thought: Some("nope"))]"#.into()));
    let err = Defs::from_sources(&src, None).unwrap_err();
    assert!(err.contains("nope"), "{err}");
}

#[test]
fn hot_reload_keeps_ids_stable() {
    let d1 = defs();
    let mut src = Defs::read_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/defs")).unwrap();
    // Prepend a new item: it must be appended to the id space, not shift others.
    let items = src.iter_mut().find(|(k, _)| *k == "items").unwrap();
    items.1.insert(0, ("aaa.ron".into(), r#"[(name: "new_thing", label: "New", sprite: "s")]"#.into()));
    let d2 = Defs::from_sources(&src, Some(&d1)).unwrap();
    assert_eq!(d1.items.find("wood"), d2.items.find("wood"));
    assert_eq!(d2.items.find("new_thing").unwrap().index(), d1.items.len());
}

#[test]
fn deterministic_runs() {
    let mut a = Sim::new(defs(), 7, &small());
    let mut b = Sim::new(defs(), 7, &small());
    scenario::basic_orders(&mut a);
    scenario::basic_orders(&mut b);
    run(&mut a, 6000);
    run(&mut b, 6000);
    assert_eq!(a.state_hash(), b.state_hash());
    let c = Sim::new(defs(), 8, &small());
    assert_ne!(Sim::new(defs(), 7, &small()).state_hash(), c.state_hash());
}

#[test]
fn save_load_continues_identically() {
    let mut a = Sim::new(defs(), 11, &small());
    scenario::basic_orders(&mut a);
    run(&mut a, 4000);
    let bytes = a.save_bytes().unwrap();
    let mut b = Sim::new(defs(), 999, &small());
    let skipped = b.load_bytes(&bytes).unwrap();
    assert!(skipped.is_empty(), "skipped {skipped:?}");
    assert_eq!(a.state_hash(), b.state_hash());
    run(&mut a, 4000);
    run(&mut b, 4000);
    assert_eq!(a.state_hash(), b.state_hash(), "diverged after load");
}

#[test]
fn load_rejects_garbage() {
    let mut s = Sim::new(defs(), 1, &small());
    let h = s.state_hash();
    assert!(s.load_bytes(b"nonsense").is_err());
    assert!(s.load_bytes(b"COLONY01garbage").is_err());
    assert_eq!(s.state_hash(), h, "failed load must not modify the sim");
}

#[test]
fn pathfinding_on_flat_ground_and_ramps() {
    let d = defs();
    let soil = d.terrain.expect("soil");
    let mut map = Map::new(10, 10, 3);
    for y in 0..10 {
        for x in 0..10 {
            map.set_solid(P3::new(x, y, 0), Some(soil));
            if x >= 5 {
                map.set_solid(P3::new(x, y, 1), Some(soil));
            }
        }
    }
    let mut pf = Pathfinder::default();
    // No ramp: the plateau at z=2 is unreachable from z=1.
    assert!(pf.find_path(&map, P3::new(0, 0, 1), Goal::On(P3::new(8, 8, 2)), 10_000).is_none());
    map.cell_mut(P3::new(4, 4, 1)).ramp = Some(sim::geom::Dir4::E);
    let path = pf.find_path(&map, P3::new(0, 0, 1), Goal::On(P3::new(8, 8, 2)), 10_000).expect("path via ramp");
    assert!(path.contains(&P3::new(4, 4, 1)) && path.contains(&P3::new(5, 4, 2)));
    // And back down.
    assert!(pf.find_path(&map, P3::new(8, 8, 2), Goal::On(P3::new(0, 0, 1)), 10_000).is_some());
}

#[test]
fn colony_does_work_over_a_day() {
    let mut s = Sim::new(defs(), 3, &small());
    scenario::basic_orders(&mut s);
    run(&mut s, 30_000);
    let w = &s.world;
    assert!(w.read::<sim::components::Building>().len() >= 3, "{}", sim::debug::colony_report(w));
    let fails: u32 = w.read::<sim::jobs::PawnDebug>().values().iter().map(|d| d.jobs_failed).sum();
    let done: u32 = w.read::<sim::jobs::PawnDebug>().values().iter().map(|d| d.jobs_done).sum();
    assert!(done > 20 && done < 5000, "jobs done {done}");
    assert!(fails * 5 < done.max(1), "too many failures: {fails}/{done}\n{}", sim::debug::colony_report(w));
}
