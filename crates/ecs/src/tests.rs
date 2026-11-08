use serde::{Deserialize, Serialize};

use crate::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Pos(i32, i32);
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Name(String);
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Counter(u32);

fn world() -> World {
    let mut w = World::new();
    w.register::<Pos>();
    w.register::<Name>();
    w
}

#[test]
fn spawn_get_despawn() {
    let mut w = world();
    let a = w.spawn((Pos(1, 2), Name("a".into())));
    let b = w.spawn((Pos(3, 4),));
    assert_eq!(*w.get::<Pos>(a).unwrap(), Pos(1, 2));
    assert!(w.has::<Name>(a));
    assert!(!w.has::<Name>(b));
    assert!(w.despawn(a));
    assert!(!w.is_alive(a));
    assert!(w.get::<Pos>(a).is_none());
    // slot reuse bumps generation; stale handle stays dead
    let c = w.spawn((Pos(9, 9),));
    assert_eq!(c.index(), a.index());
    assert_ne!(c.generation(), a.generation());
    assert!(w.get::<Pos>(a).is_none());
    assert_eq!(w.read::<Pos>().len(), 2);
}

#[test]
fn sparse_set_swap_remove_keeps_lookup() {
    let mut w = world();
    let es: Vec<_> = (0..10).map(|i| w.spawn((Pos(i, i),))).collect();
    w.remove::<Pos>(es[2]);
    w.remove::<Pos>(es[0]);
    for (i, e) in es.iter().enumerate() {
        let p = w.get::<Pos>(*e).map(|p| p.0);
        if i == 0 || i == 2 {
            assert_eq!(p, None);
        } else {
            assert_eq!(p, Some(i as i32));
        }
    }
}

#[test]
fn simultaneous_borrows_and_deferred() {
    let mut w = world();
    let a = w.spawn((Pos(0, 0), Name("x".into())));
    {
        let names = w.read::<Name>();
        let mut pos = w.write::<Pos>();
        for (e, _) in names.iter() {
            pos.get_mut(e).unwrap().0 += 5;
            w.defer_insert(e, Counter(1));
        }
        let spawned = w.spawn_deferred((Pos(7, 7),));
        assert!(w.is_alive(spawned));
    }
    w.apply_commands();
    assert_eq!(w.get::<Pos>(a).unwrap().0, 5);
    assert_eq!(w.get::<Counter>(a).unwrap().0, 1);
    assert_eq!(w.read::<Pos>().len(), 2);
}

#[test]
#[should_panic]
fn double_mut_borrow_panics() {
    let w = world();
    let _a = w.write::<Pos>();
    let _b = w.write::<Pos>();
}

#[test]
fn resources() {
    let mut w = World::new();
    w.insert_resource(Counter(3));
    w.res_mut::<Counter>().0 += 1;
    assert_eq!(w.res::<Counter>().0, 4);
}

#[test]
fn snapshot_roundtrip() {
    let mut reg = SaveRegistry::new();
    reg.component::<Pos>("pos").component::<Name>("name").resource::<Counter>("counter");

    let mut w = world();
    w.insert_resource(Counter(42));
    let a = w.spawn((Pos(1, 1), Name("a".into())));
    let b = w.spawn((Pos(2, 2),));
    w.despawn(a);
    let c = w.spawn((Name("c".into()),));
    let h1 = reg.state_hash(&w).unwrap();
    let bytes = reg.save_bytes(&w).unwrap();

    let mut w2 = world();
    w2.insert_resource(Counter(0));
    w2.spawn((Pos(100, 100),));
    let skipped = reg.load_bytes(&mut w2, &bytes).unwrap();
    assert!(skipped.is_empty());
    assert_eq!(w2.res::<Counter>().0, 42);
    assert_eq!(*w2.get::<Pos>(b).unwrap(), Pos(2, 2));
    assert_eq!(*w2.get::<Name>(c).unwrap(), Name("c".into()));
    assert!(!w2.is_alive(a));
    assert_eq!(reg.state_hash(&w2).unwrap(), h1);
    // allocator state restored: next spawn matches in both worlds
    assert_eq!(w.spawn_empty(), w2.spawn_empty());
}

#[test]
fn schedule_applies_commands_between_systems() {
    fn spawn_sys(w: &mut World) {
        w.spawn_deferred((Pos(0, 0),));
    }
    fn count_sys(w: &mut World) {
        let n = w.read::<Pos>().len() as u32;
        w.res_mut::<Counter>().0 = n;
    }
    let mut w = world();
    w.insert_resource(Counter(0));
    let mut s = Schedule::new();
    s.add("spawn", spawn_sys).add("count", count_sys);
    s.run(&mut w);
    s.run(&mut w);
    assert_eq!(w.res::<Counter>().0, 2);
}
