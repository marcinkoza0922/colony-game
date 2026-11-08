use std::path::Path;

use sim::components::{Item, Plant, Position};
use sim::defs::Defs;
use sim::env::{self, Env};
use sim::farm;
use sim::map::Map;
use sim::orders;
use sim::resources::{Clock, GameLog, SimMeta, TICKS_PER_DAY, TICKS_PER_HOUR, Zones};
use sim::worldgen::WorldGenParams;
use sim::{P3, Sim};

fn defs() -> Defs {
    Defs::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/defs"), None).expect("defs load")
}

fn sim() -> Sim {
    Sim::new(defs(), 21, &WorldGenParams { width: 48, height: 48, levels: 5 })
}

/// Jumps the clock to `day` (1-based) at `hour` and settles the environment.
fn set_time(s: &mut Sim, day: u64, hour: u64) {
    // The clock starts at 08:00 on day 1.
    let t = (day - 1) * TICKS_PER_DAY + hour * TICKS_PER_HOUR;
    s.world.res_mut::<Clock>().tick = t.saturating_sub(8 * TICKS_PER_HOUR);
    for _ in 0..40 {
        env::update_env(&s.world);
    }
}

/// Up to `n` fertile, zoneable surface cells near the landing site.
fn fertile_cells(s: &Sim, n: usize) -> Vec<P3> {
    let d = s.defs();
    let start = s.world.res::<SimMeta>().start;
    let mut v: Vec<P3> = (-12..=12)
        .flat_map(|dy| (-12..=12).map(move |dx| start.offset(dx, dy, 0)))
        .filter(|p| p.chebyshev(start) >= 4)
        .filter(|p| orders::grow_zone_cell_valid(&s.world, *p))
        .filter(|p| farm::fertility_at(&d, &s.world.res::<Map>(), *p) >= 1.0)
        .collect();
    v.sort_by_key(|p| (p.manhattan(start), p.y, p.x));
    v.truncate(n);
    v
}

fn count_plants(s: &Sim, name: &str) -> usize {
    let id = s.defs().plants.expect(name);
    s.world.read::<Plant>().iter().filter(|(_, p)| p.def == id).count()
}

fn count_items(s: &Sim, name: &str) -> u32 {
    let id = s.defs().items.expect(name);
    s.world.read::<Item>().iter().filter(|(_, it)| it.def == id).map(|(_, it)| it.count).sum()
}

#[test]
fn colonists_sow_and_harvest_a_grow_zone() {
    let mut s = sim();
    set_time(&mut s, 20, 8); // early summer
    let cells = fertile_cells(&s, 12);
    assert!(cells.len() >= 8, "need fertile ground near start");
    let zone = orders::create_grow_zone(&mut s.world, &cells).expect("zone");
    // Somewhere to haul the harvest so cells free up for resowing.
    let start = s.world.res::<SimMeta>().start;
    let stock: Vec<P3> = (-1..=1).flat_map(|dy| (-1..=1).map(move |dx| start.offset(dx, dy, 0))).collect();
    orders::create_stockpile(&mut s.world, &stock).expect("stockpile");
    let ashroot = s.defs().plants.expect("ashroot");
    orders::set_grow_zone(&mut s.world, zone, ashroot, true);
    let seeds_before = count_items(&s, "ashroot_seeds");

    s.run_ticks(TICKS_PER_DAY / 2);
    let sown = count_plants(&s, "ashroot");
    assert!(sown >= cells.len() / 2, "only {sown} of {} cells sown", cells.len());
    assert!(count_items(&s, "ashroot_seeds") <= seeds_before - sown as u32);
    // No foreign plants survive in the zone.
    {
        let map = s.world.res::<Map>();
        let plants = s.world.read::<Plant>();
        for p in &cells {
            if let Some(e) = map.cell(*p).plant {
                assert_eq!(plants.get(e).unwrap().def, ashroot, "foreign plant left at {p}");
            }
        }
    }

    // Fast-forward growth, then colonists should harvest.
    for (_, pl) in s.world.write::<Plant>().iter_mut() {
        if pl.def == ashroot {
            pl.growth = 0.999;
        }
    }
    let food_before = count_items(&s, "ashroot");
    s.run_ticks(TICKS_PER_DAY);
    assert!(count_items(&s, "ashroot") >= food_before + 12, "harvest produced too little food");
    // Harvest returned seeds and the zone gets resown.
    assert!(count_plants(&s, "ashroot") > 0);
}

#[test]
fn frost_damages_then_kills_crops() {
    let mut s = sim();
    let wheat = s.defs().plants.expect("rustwheat");
    let cells = fertile_cells(&s, 6);
    for p in &cells {
        let old = s.world.res::<Map>().cell(*p).plant;
        if let Some(e) = old {
            sim::jobs::remove_plant(&mut s.world, e);
        }
        let e = s.world.spawn((Plant::new(wheat, 0.5), Position(*p)));
        s.world.res_mut::<Map>().cell_mut(*p).plant = Some(e);
    }
    set_time(&mut s, 53, 2); // deep winter night
    let temp = s.defs().env_fields.expect("temperature");
    let t = env::field_at(&s.world, temp, cells[0]);
    assert!(t < 0.0, "expected frost, got {t}");

    // A few hours hurt the crop without killing it.
    s.run_ticks(TICKS_PER_HOUR * 4);
    let healths: Vec<f32> = s.world.read::<Plant>().iter().filter(|(_, p)| p.def == wheat).map(|(_, p)| p.health).collect();
    assert_eq!(healths.len(), cells.len());
    assert!(healths.iter().all(|h| *h < 1.0 && *h > 0.0), "{healths:?}");

    // A few winter days kill it.
    s.run_ticks(TICKS_PER_DAY * 3);
    assert_eq!(count_plants(&s, "rustwheat"), 0);
    assert!(s.world.res::<GameLog>().entries.iter().any(|e| e.text.contains("died of frost")));
}

#[test]
fn gloomcap_grows_only_in_the_dark() {
    let mut s = sim();
    set_time(&mut s, 20, 12); // summer noon
    let d = s.defs();
    let p = fertile_cells(&s, 1)[0];
    let (gloom, ash) = (&d.plants[d.plants.expect("gloomcap")], &d.plants[d.plants.expect("ashroot")]);
    let rate = |s: &Sim, pd| farm::growth_info(&d, &s.world.res::<Map>(), &s.world.res::<Env>(), pd, p).rate;
    assert!(rate(&s, gloom) < 0.05, "gloomcap grows in sunlight");
    assert!(rate(&s, ash) > 0.5, "ashroot should grow at noon");
    // Under a roof it is dark: the fungus grows, the tuber doesn't.
    s.world.res_mut::<Map>().set_roof(p, true);
    for _ in 0..3 {
        env::update_env(&s.world);
    }
    assert!(rate(&s, gloom) > 0.5);
    assert!(rate(&s, ash) < 0.05);
}

#[test]
fn farming_survives_save_load_deterministically() {
    let mut a = sim();
    set_time(&mut a, 20, 8);
    let cells = fertile_cells(&a, 10);
    orders::create_grow_zone(&mut a.world, &cells).expect("zone");
    a.run_ticks(4000);
    let bytes = a.save_bytes().unwrap();
    let mut b = sim();
    b.load_bytes(&bytes).unwrap();
    assert_eq!(b.world.res::<Zones>().growing.len(), 1);
    a.run_ticks(6000);
    b.run_ticks(6000);
    assert_eq!(a.state_hash(), b.state_hash());
}
