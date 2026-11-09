use std::path::Path;

use ecs::Entity;
use sim::components::{Mood, Needs, Pawn};
use sim::defs::{Defs, RelKind};
use sim::health::Health;
use sim::jobs;
use sim::map::Map;
use sim::resources::{SimMeta, TICKS_PER_DAY, TICKS_PER_HOUR};
use sim::social::{self, Social};
use sim::worldgen::WorldGenParams;
use sim::{P3, Sim};

fn defs() -> Defs {
    Defs::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/defs"), None).expect("defs load")
}

fn sim(seed: u64) -> Sim {
    Sim::new(defs(), seed, &WorldGenParams { width: 48, height: 48, levels: 5 })
}

fn pawns(s: &Sim) -> Vec<Entity> {
    s.world.read::<Pawn>().entities().to_vec()
}

fn free_cells(s: &Sim, n: usize, dy: i32) -> Vec<P3> {
    let start = s.world.res::<SimMeta>().start;
    let map = s.world.res::<Map>();
    (-8..8)
        .map(|dx| start.offset(dx, dy, 0))
        .filter(|p| map.is_walkable(*p) && map.cell(*p).item.is_none() && map.cell(*p).building.is_none() && map.cell(*p).plant.is_none())
        .take(n)
        .collect()
}

#[test]
fn starting_colony_includes_family() {
    let s = sim(5);
    let fam = pawns(&s)
        .iter()
        .filter(|p| s.world.get::<Social>(**p).is_some_and(|x| x.relations.iter().any(|(_, k)| k.is_family())))
        .count();
    assert_eq!(fam, 2, "exactly one related pair");
    let related: Vec<Entity> = pawns(&s)
        .into_iter()
        .filter(|p| s.world.get::<Social>(*p).is_some_and(|x| x.relations.iter().any(|(_, k)| k.is_family())))
        .collect();
    let (a, b) = (related[0], related[1]);
    let la = jobs::pawn_name(&s.world, a).split_once(' ').map(|x| x.1.to_string());
    let lb = jobs::pawn_name(&s.world, b).split_once(' ').map(|x| x.1.to_string());
    assert_eq!(la, lb, "kin share a family name");
    assert!(social::opinion_of(&s.world, a, b) >= 15.0, "family starts fond");
}

#[test]
fn colonists_get_to_know_each_other() {
    let mut s = sim(8);
    s.run_ticks(TICKS_PER_DAY * 2);
    let ps = pawns(&s);
    let known: usize = ps.iter().map(|p| s.world.get::<Social>(*p).map_or(0, |x| x.opinions.len())).sum();
    assert!(known >= 4, "interactions happened ({known} opinion entries)");
    // Social XP was gained by someone.
    let social = s.defs().skills.expect("social");
    let xp: f32 = ps
        .iter()
        .map(|p| s.world.get::<sim::components::Skills>(*p).map_or(0.0, |sk| sk.levels[social.index()].xp))
        .sum();
    assert!(xp > 0.0);
}

#[test]
fn race_attitudes_fade_with_familiarity() {
    let s = sim(5);
    let d = s.defs();
    let ps = pawns(&s);
    let race = |e: Entity| s.world.get::<Pawn>(e).unwrap().race;
    let elf = ps.iter().copied().find(|p| d.races[race(*p)].name == "elf").unwrap();
    let dwarf = ps.iter().copied().find(|p| d.races[race(*p)].name == "dwarf").unwrap();
    let cold = social::race_bias(&d, &s.world, elf, dwarf, 0.0);
    let warm = social::race_bias(&d, &s.world, elf, dwarf, 0.8);
    assert!(cold < 0.0 || s.world.get::<sim::components::Quirks>(elf).unwrap().0.contains(&d.quirks.expect("open_minded")));
    assert!(warm.abs() <= cold.abs() * 0.21 + 1e-4);
}

#[test]
fn fistfights_hurt_but_do_not_kill() {
    let mut s = sim(11);
    let ps = pawns(&s);
    let (a, b) = (ps[0], ps[1]);
    let cells = free_cells(&s, 2, 2);
    s.world.insert(a, sim::components::Position(cells[0]));
    s.world.insert(b, sim::components::Position(cells[1]));
    social::start_fight(&mut s.world, a, b);
    s.run_ticks(1200);
    assert!(s.world.is_alive(a) && s.world.is_alive(b));
    let bruise = s.defs().hediffs.expect("bruise");
    let hurt = [a, b].iter().filter(|p| s.world.get::<Health>(**p).is_some_and(|h| h.has(bruise))).count();
    assert!(hurt >= 1, "someone got bruised");
    for p in [a, b] {
        let fighting = s.world.get::<sim::components::CurrentJob>(p).is_some_and(|j| j.0.kind == jobs::JobKind::Fight);
        assert!(!fighting, "fight is over");
    }
    assert!(social::opinion_of(&s.world, a, b) < 0.0);
}

#[test]
fn flirting_can_start_a_romance() {
    let mut s = sim(13);
    let d = s.defs();
    let ps = pawns(&s);
    // Two unrelated colonists who like each other.
    let (a, b) = ps
        .iter()
        .flat_map(|a| ps.iter().map(move |b| (*a, *b)))
        .find(|(a, b)| {
            a != b
                && !s.world.get::<Social>(*a).unwrap().relation(*b).iter().any(|k| k.is_family())
                && social::compatibility(&d, &s.world, *a, *b) >= 0.3
        })
        .expect("compatible pair");
    for (x, y) in [(a, b), (b, a)] {
        social::add_opinion(&s.world, x, y, "Test", 70.0, 30.0);
    }
    let flirt = d.interactions.expect("flirt");
    for _ in 0..60 {
        social::interact(&mut s.world, a, b, flirt);
        if s.world.get::<Social>(a).unwrap().has(b, RelKind::Lover) {
            break;
        }
    }
    assert!(s.world.get::<Social>(a).unwrap().has(b, RelKind::Lover));
    assert!(s.world.get::<Social>(b).unwrap().has(a, RelKind::Lover), "relations are symmetric");
}

#[test]
fn bored_colonists_play_games_and_eat_at_tables() {
    let mut s = sim(17);
    let d = s.defs();
    let row = free_cells(&s, 8, -3);
    assert!(row.len() >= 4);
    jobs::spawn_building(&mut s.world, d.buildings.expect("dice_table"), 0, row[0]);
    let table = row.windows(2).skip(1).find(|w| w[1] == w[0].offset(1, 0, 0)).expect("two free cells side by side");
    jobs::spawn_building(&mut s.world, d.buildings.expect("table"), 0, table[0]);
    jobs::spawn_building(&mut s.world, d.buildings.expect("chair"), 0, table[1]);
    let rec = d.needs.expect("recreation");
    let hunger = d.needs.expect("hunger");
    for p in pawns(&s) {
        let mut n = s.world.get_mut::<Needs>(p).unwrap();
        n.values[rec.index()] = 0.1;
        n.values[hunger.index()] = 0.2;
    }
    let at_table = d.thoughts.expect("ate_at_table");
    let (mut played, mut ate) = (false, false);
    for _ in 0..(TICKS_PER_HOUR * 8 / 250) {
        s.run_ticks(250);
        played |= pawns(&s).iter().any(|p| {
            s.world
                .get::<Social>(*p)
                .is_some_and(|x| x.opinions.iter().any(|o| o.memories.iter().any(|m| m.label.starts_with("Played"))))
        });
        ate |= pawns(&s)
            .iter()
            .any(|p| s.world.get::<Mood>(*p).is_some_and(|m| m.memories.iter().any(|x| x.thought == at_table)));
    }
    assert!(played, "two colonists played at the dice table");
    assert!(ate, "someone ate at the table");
}
