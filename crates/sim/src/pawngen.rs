//! Random colonist generation.

use std::rc::Rc;

use ecs::{Entity, World};

use crate::components::*;
use crate::defs::*;
use crate::geom::P3;
use crate::jobs::PawnDebug;
use crate::rng::SimRng;

pub fn generate_pawn(world: &mut World, race: RaceId, pos: P3) -> Entity {
    let defs = world.res::<Rc<Defs>>().clone();
    let rd = &defs.races[race];
    let mut rng = world.res_mut::<SimRng>().fork();

    let first = rng.pick(&rd.first_names).cloned().unwrap_or_else(|| "Nameless".into());
    let name = match rng.pick(&rd.last_names) {
        Some(l) => format!("{first} {l}"),
        None => first,
    };
    let age = rng.range_i32(rd.adult_age.0 as i32, rd.adult_age.1 as i32) as u32;

    // Quirks: innate ones plus 2–3 rolled, respecting conflicts and race weights.
    let mut quirks: Vec<QuirkId> = rd.innate_quirks.clone();
    let rolls = rng.range_i32(2, 3);
    for _ in 0..rolls {
        let weights: Vec<f32> = defs
            .quirks
            .iter()
            .map(|(id, q)| {
                if q.innate || quirks.contains(&id) {
                    return 0.0;
                }
                let conflict = quirks.iter().any(|h| q.conflicts.contains(h) || defs.quirks[*h].conflicts.contains(&id));
                if conflict {
                    return 0.0;
                }
                let rmul = rd.quirk_weights.iter().find(|(qq, _)| *qq == id).map(|(_, w)| *w).unwrap_or(1.0);
                q.weight * rmul
            })
            .collect();
        if let Some(i) = rng.weighted(&weights) {
            quirks.push(QuirkId::from_index(i));
        }
    }

    // Skills: modest random base, race bonus, and two "specialities".
    let mut levels = vec![SkillLevel::default(); defs.skills.len()];
    for (id, _) in defs.skills.iter() {
        let mut l = rng.range_i32(0, 4);
        l += rd.skill_bonus.iter().filter(|(s, _)| *s == id).map(|(_, b)| *b).sum::<i32>();
        levels[id.index()].level = l.clamp(0, MAX_SKILL as i32) as u8;
        levels[id.index()].xp = rng.range_f32(0.0, 500.0);
    }
    for _ in 0..2 {
        let i = rng.index(levels.len());
        levels[i].level = (levels[i].level as i32 + rng.range_i32(3, 6)).min(MAX_SKILL as i32) as u8;
    }

    // Appearance genes.
    let ap = &rd.appearance;
    let style = rng.range_i32(0, ap.styles.max(1) as i32 - 1) as u8;
    let mut pick_col = |list: &[String]| rng.pick(list).map(|s| parse_hex(s)).unwrap_or([200, 200, 200]);
    let appearance = Appearance {
        skin: pick_col(&ap.skin),
        hair: pick_col(&ap.hair),
        top: pick_col(&ap.top),
        bottom: pick_col(&ap.bottom),
        style,
    };

    // Needs start fairly full.
    let mut needs = vec![1.0f32; defs.needs.len()];
    for rn in &rd.needs {
        needs[rn.need.index()] = rng.range_f32(0.6, 0.95);
    }

    // Work priorities: everything at 3, the pawn's two best skills at 2.
    let mut prios = vec![3u8; defs.work_types.len()];
    let mut ranked: Vec<(u8, usize)> = levels.iter().enumerate().map(|(i, s)| (s.level, i)).collect();
    ranked.sort_by(|a, b| b.cmp(a));
    for (_, si) in ranked.iter().take(2) {
        for (wt, wd) in defs.work_types.iter() {
            if wd.skill.map(|s| s.index()) == Some(*si) {
                prios[wt.index()] = 2;
            }
        }
    }

    let e = world.spawn((
        Pawn { name, race, age },
        Position(pos),
        Needs { values: needs },
        Mood::default(),
        Skills { levels },
        Quirks(quirks),
        WorkPriorities(prios),
        Inventory::default(),
        Facing(4),
        PawnDebug::default(),
    ));
    world.insert(e, appearance);
    let mut h = crate::health::Health::default();
    crate::health::recompute(&defs, &defs.bodies[rd.body], &mut h);
    world.insert(e, h);
    world.insert(e, crate::social::Social::default());
    crate::stats::refresh_stats(world, e);
    world.apply_commands();
    e
}
