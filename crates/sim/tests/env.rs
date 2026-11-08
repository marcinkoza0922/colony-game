use std::path::Path;

use sim::defs::Defs;
use sim::env::{self, Env, Rooms};
use sim::map::Map;
use sim::resources::SimMeta;
use sim::worldgen::WorldGenParams;
use sim::{P3, Sim};

fn defs() -> Defs {
    Defs::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/defs"), None).expect("defs load")
}

fn sim() -> Sim {
    Sim::new(defs(), 21, &WorldGenParams { width: 48, height: 48, levels: 5 })
}

/// Builds a closed hut: walls around a (2*r+1)^2 interior, one door, roof.
fn build_hut(s: &mut Sim, c: P3, r: i32, roof: bool) -> Vec<P3> {
    let d = s.defs();
    let wall = d.buildings.expect("wood_wall");
    let door = d.buildings.expect("door");
    let w = &mut s.world;
    // Clear any plants/items in the area so the hut is clean.
    let mut interior = Vec::new();
    for dy in -(r + 1)..=(r + 1) {
        for dx in -(r + 1)..=(r + 1) {
            let p = c.offset(dx, dy, 0);
            let plant = w.res::<Map>().cell(p).plant;
            if let Some(pl) = plant {
                sim::jobs::remove_plant(w, pl);
            }
            let edge = dx.abs() == r + 1 || dy.abs() == r + 1;
            if edge {
                let def = if dx == 0 && dy == r + 1 { door } else { wall };
                sim::jobs::spawn_building(w, def, 0, p);
            } else {
                interior.push(p);
                if roof {
                    w.res_mut::<Map>().set_roof(p, true);
                }
            }
        }
    }
    interior
}

fn settle(s: &mut Sim) {
    env::refresh_rooms(&s.world);
    env::update_env(&s.world);
}

#[test]
fn enclosed_roofed_hut_is_an_indoor_room() {
    let mut s = sim();
    let c = s.world.res::<SimMeta>().start.offset(0, -8, 0);
    let inside = build_hut(&mut s, c, 1, true);
    settle(&mut s);
    let map = s.world.res::<Map>();
    let rooms = s.world.res::<Rooms>();
    let room = rooms.indoor_room_at(&map, c).expect("hut should be an indoor room");
    assert_eq!(room.cells.len(), inside.len());
    // The door separates inside from outside.
    let outside = c.offset(0, 4, 0);
    assert!(rooms.indoor_room_at(&map, outside).is_none());
}

#[test]
fn unroofed_enclosure_is_not_indoors() {
    let mut s = sim();
    let c = s.world.res::<SimMeta>().start.offset(0, -8, 0);
    build_hut(&mut s, c, 1, false);
    settle(&mut s);
    let map = s.world.res::<Map>();
    assert!(s.world.res::<Rooms>().indoor_room_at(&map, c).is_none());
}

#[test]
fn bed_makes_a_bedroom_and_campfire_heats_insulated_room() {
    let mut s = sim();
    let d = s.defs();
    let c = s.world.res::<SimMeta>().start.offset(0, -8, 0);
    build_hut(&mut s, c, 1, true);
    sim::jobs::spawn_building(&mut s.world, d.buildings.expect("bed"), 0, c.offset(-1, -1, 0));
    settle(&mut s);
    {
        let map = s.world.res::<Map>();
        let rooms = s.world.res::<Rooms>();
        let room = rooms.indoor_room_at(&map, c).unwrap();
        assert_eq!(room.role, d.room_roles.find("bedroom"));
    }
    // Add a fire, then let the environment run for a while.
    sim::jobs::spawn_building(&mut s.world, d.buildings.expect("campfire"), 0, c.offset(1, 1, 0));
    for _ in 0..60 {
        settle(&mut s);
    }
    let temp = d.core.field_temperature.unwrap();
    let inside = env::field_at(&s.world, temp, c);
    let outside = env::field_at(&s.world, temp, c.offset(0, 8, 0));
    assert!(inside > outside + 10.0, "inside {inside:.1} vs outside {outside:.1}");
    let map = s.world.res::<Map>();
    let rooms = s.world.res::<Rooms>();
    let room = rooms.indoor_room_at(&map, c).unwrap();
    // Campfire wins the role? No: bedroom has higher priority.
    assert_eq!(room.role, d.room_roles.find("bedroom"));
    assert!(room.fields[temp.index()] > outside + 10.0);
}

#[test]
fn foot_traffic_dirties_rooms_and_colonists_clean_them() {
    let mut s = sim();
    let d = s.defs();
    let filth = d.core.field_filth.unwrap();
    let start = s.world.res::<SimMeta>().start;
    let c = start.offset(0, -8, 0);
    let inside = build_hut(&mut s, c, 2, true);
    settle(&mut s);
    // Make the floor filthy.
    {
        let map = s.world.res::<Map>();
        let mut e = s.world.res_mut::<Env>();
        for p in &inside {
            e.queue_add(filth, map.idx(*p), 1.5);
        }
    }
    settle(&mut s);
    let dirty: f32 = inside.iter().map(|p| env::field_at(&s.world, filth, *p)).sum();
    assert!(dirty > 20.0);
    // Make everyone a dedicated cleaner and let time pass.
    let cleaning = d.core.wt_cleaning.unwrap();
    let pawns: Vec<_> = s.world.read::<sim::components::Pawn>().entities().to_vec();
    for p in pawns {
        for (wt, _) in d.work_types.iter() {
            sim::orders::set_priority(&mut s.world, p, wt, if wt == cleaning { 1 } else { 0 });
        }
    }
    s.run_ticks(6000);
    let after: f32 = inside.iter().map(|p| env::field_at(&s.world, filth, *p)).sum();
    assert!(after < dirty * 0.5, "filth {dirty:.1} -> {after:.1}");
}

#[test]
fn environment_survives_save_load_deterministically() {
    let mut a = sim();
    let c = a.world.res::<SimMeta>().start.offset(0, -8, 0);
    build_hut(&mut a, c, 1, true);
    a.run_ticks(3000);
    let bytes = a.save_bytes().unwrap();
    let mut b = sim();
    b.load_bytes(&bytes).unwrap();
    a.run_ticks(3000);
    b.run_ticks(3000);
    assert_eq!(a.state_hash(), b.state_hash());
}
