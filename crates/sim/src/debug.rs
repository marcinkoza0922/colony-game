//! Debug/inspection helpers: ASCII map dumps and colony reports. Used by the
//! headless `simrun` tool, tests, and the in-game debug overlay.

use std::fmt::Write;
use std::rc::Rc;

use ecs::{Entity, World};

use crate::components::*;
use crate::defs::Defs;
use crate::geom::P3;
use crate::jobs::PawnDebug;
use crate::map::{Designation, Map};
use crate::resources::*;
use crate::stats::StatCache;

/// ASCII view of one level around `center` (radius `r`).
///
/// Legend: `#` solid, `.` floor, `_` open air (no floor), `^` ramp, `X` stairs,
/// `W` wall, `D` door, `B` bed, `T` workbench, `?` blueprint, `t` tree,
/// `*` plant, `i` item, `@` pawn, `m` mine designation, `s` stockpile floor.
pub fn ascii_level(world: &World, z: i32, center: P3, r: i32) -> String {
    let map = world.res::<Map>();
    let defs = world.res::<Rc<Defs>>().clone();
    let pawns = world.res::<PawnPositions>();
    let plants = world.read::<Plant>();
    let bps = world.read::<Blueprint>();
    let mut s = String::new();
    for y in (center.y - r)..=(center.y + r) {
        for x in (center.x - r)..=(center.x + r) {
            let p = P3::new(x, y, z);
            let Some(c) = map.get(p) else {
                s.push(' ');
                continue;
            };
            let ch = if pawns.0.iter().any(|(_, q)| *q == p) {
                '@'
            } else if c.solid.is_some() {
                if c.designation == Designation::Mine { 'm' } else { '#' }
            } else if let Some(b) = c.building {
                if bps.contains(b) {
                    '?'
                } else {
                    match c.building_kind {
                        Some(crate::defs::BuildingKind::Wall) => 'W',
                        Some(crate::defs::BuildingKind::Door) => 'D',
                        Some(crate::defs::BuildingKind::Bed) => 'B',
                        Some(crate::defs::BuildingKind::Workbench) => 'T',
                        Some(crate::defs::BuildingKind::Stairs) => 'X',
                        _ => 'F',
                    }
                }
            } else if c.item.is_some() {
                'i'
            } else if let Some(pl) = c.plant {
                if plants.get(pl).is_some_and(|pp| defs.plants[pp.def].tree) { 't' } else { '*' }
            } else if c.ramp.is_some() {
                '^'
            } else if !map.is_supported(p) {
                '_'
            } else if c.zone.is_some() {
                's'
            } else {
                '.'
            };
            s.push(ch);
        }
        s.push('\n');
    }
    s
}

pub fn pawn_summary(world: &World, e: Entity) -> String {
    let defs = world.res::<Rc<Defs>>().clone();
    let mut s = String::new();
    let Some(p) = world.get_cloned::<Pawn>(e) else { return s };
    let pos = world.get::<Position>(e).map(|p| p.0).unwrap_or_default();
    let _ = write!(s, "{} ({}, {}) at {}", p.name, defs.races[p.race].label, p.age, pos);
    if let Some(m) = world.get::<Mood>(e) {
        let _ = write!(s, " mood {:.0}->{:.0}", m.value, m.target);
    }
    if let Some(ms) = world.get::<MentalState>(e) {
        let _ = write!(s, " [BREAK: {}]", ms.kind.label());
    }
    if let Some(j) = world.get::<CurrentJob>(e) {
        let _ = write!(s, " | {} (step {}/{})", j.0.kind.label(), j.0.step + 1, j.0.toils.len());
    }
    s.push('\n');
    if let (Some(n), Some(_)) = (world.get::<Needs>(e), Some(())) {
        s.push_str("   needs:");
        for rn in &defs.races[p.race].needs {
            let _ = write!(s, " {}={:.2}", defs.needs[rn.need].name, n.values.get(rn.need.index()).unwrap_or(&1.0));
        }
        s.push('\n');
    }
    if let Some(q) = world.get::<Quirks>(e) {
        let names: Vec<_> = q.0.iter().map(|q| defs.quirks[*q].label.as_str()).collect();
        let _ = writeln!(s, "   quirks: {}", names.join(", "));
    }
    if let Some(sk) = world.get::<Skills>(e) {
        s.push_str("   skills:");
        for (id, sd) in defs.skills.iter() {
            if let Some(l) = sk.levels.get(id.index())
                && l.level > 0
            {
                let _ = write!(s, " {}={}", sd.name, l.level);
            }
        }
        s.push('\n');
    }
    if let Some(m) = world.get::<Mood>(e) {
        let mut thoughts: Vec<String> = m
            .situational
            .iter()
            .map(|t| format!("{}({:+})", defs.thoughts[*t].label, defs.thoughts[*t].mood))
            .collect();
        thoughts.extend(m.memories.iter().map(|x| format!("{}({:+})", defs.thoughts[x.thought].label, defs.thoughts[x.thought].mood)));
        if !thoughts.is_empty() {
            let _ = writeln!(s, "   thoughts: {}", thoughts.join(", "));
        }
    }
    {
        let map = world.res::<Map>();
        let env = world.res::<crate::env::Env>();
        let rooms = world.res::<crate::env::Rooms>();
        let mut parts = Vec::new();
        if map.in_bounds(pos) {
            for (fid, fd) in defs.env_fields.iter() {
                parts.push(format!("{}={}", fd.name, fd.format(env.get(fid, map.idx(pos)))));
            }
        }
        let room = match rooms.indoor_room_at(&map, pos) {
            Some(r) => format!(
                "{} ({} cells, impressiveness {:.1})",
                r.role.map(|x| defs.room_roles[x].label.as_str()).unwrap_or("room"),
                r.cells.len(),
                r.impressiveness
            ),
            None => "outdoors".into(),
        };
        let _ = writeln!(s, "   env: {} | {room}", parts.join(" "));
    }
    if let Some(sc) = world.get::<StatCache>(e) {
        let c = &defs.core;
        let _ = writeln!(
            s,
            "   stats: move={:.2} work={:.2} mine={:.2} build={:.2} plant={:.2}",
            sc.get(&defs, c.stat_move_speed),
            sc.get(&defs, c.stat_work_speed),
            sc.get(&defs, c.stat_mining_speed),
            sc.get(&defs, c.stat_construction_speed),
            sc.get(&defs, c.stat_plant_speed)
        );
    }
    if let Some(d) = world.get::<PawnDebug>(e) {
        let _ = writeln!(s, "   jobs: {} done, {} failed; last fail: {}", d.jobs_done, d.jobs_failed, d.last_fail.as_deref().unwrap_or("-"));
    }
    s
}

/// Multi-line overview of the colony state.
pub fn colony_report(world: &World) -> String {
    let defs = world.res::<Rc<Defs>>().clone();
    let mut s = String::new();
    let clock = world.res::<Clock>().clone();
    let weather = world.res::<crate::env::WeatherState>().current.map(|w| defs.weather[w].label.clone()).unwrap_or_default();
    let amb = crate::env::ambient_values(&defs, &clock, &world.res::<crate::env::WeatherState>());
    let amb_s: Vec<String> = defs.env_fields.iter().filter(|(_, f)| f.kind == crate::defs::FieldKind::Dynamic).map(|(id, f)| format!("{}={}", f.name, f.format(amb[id.index()]))).collect();
    let _ = writeln!(s, "=== {} {} (tick {}) | {} | ambient {} ===", clock.season_name(), clock.time_string(), clock.tick, weather, amb_s.join(" "));
    let pawns: Vec<Entity> = world.read::<Pawn>().entities().to_vec();
    for e in pawns {
        s.push_str(&pawn_summary(world, e));
    }
    // Item totals.
    let mut totals: Vec<(String, u32)> = defs
        .items
        .iter()
        .map(|(id, d)| (d.label.clone(), crate::items::colony_count(world, id)))
        .filter(|(_, n)| *n > 0)
        .collect();
    totals.sort();
    let parts: Vec<String> = totals.iter().map(|(l, n)| format!("{l}: {n}")).collect();
    let _ = writeln!(s, "items: {}", parts.join(", "));
    let _ = writeln!(
        s,
        "entities: {}  plants: {}  buildings: {}  blueprints: {}  reservations: {}",
        world.entity_count(),
        world.read::<Plant>().len(),
        world.read::<Building>().len(),
        world.read::<Blueprint>().len(),
        world.res::<Reservations>().len()
    );
    s
}

/// Recent log lines.
pub fn recent_log(world: &World, n: usize) -> Vec<String> {
    let log = world.res::<GameLog>();
    log.entries
        .iter()
        .rev()
        .take(n)
        .rev()
        .map(|e| {
            let c = Clock { tick: e.tick };
            format!("[{}] {}", c.time_string(), e.text)
        })
        .collect()
}
