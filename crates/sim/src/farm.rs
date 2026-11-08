//! Farming: plant growth and health driven by environment fields, soil
//! fertility, and the cell rules used by sow / harvest / clear work givers.

use std::collections::BTreeMap;
use std::rc::Rc;

use ecs::{Entity, World};

use crate::components::{Plant, Position};
use crate::defs::*;
use crate::env::Env;
use crate::geom::P3;
use crate::map::Map;
use crate::resources::*;

/// Ticks between plant updates.
pub const PLANT_INTERVAL: u64 = 250;

/// Fertility of the floor under `p` (0 when there is no solid block below).
pub fn fertility_at(defs: &Defs, map: &Map, p: P3) -> f32 {
    map.get(p.down()).and_then(|c| c.solid).map(|t| defs.terrain[t].fertility).unwrap_or(0.0)
}

/// What currently limits a plant's growth.
#[derive(Clone, Debug, PartialEq)]
pub enum Limit {
    Field(EnvFieldId),
    Fertility,
}

#[derive(Clone, Debug)]
pub struct GrowthInfo {
    /// Multiplier on the base growth rate (1 = full speed).
    pub rate: f32,
    /// The weakest factor below 1, if any.
    pub limit: Option<(Limit, f32)>,
}

/// Growth-rate multiplier for a plant of `pd` at cell `p`.
pub fn growth_info(defs: &Defs, map: &Map, env: &Env, pd: &PlantDef, p: P3) -> GrowthInfo {
    let mut rate = 1.0;
    let mut limit: Option<(Limit, f32)> = None;
    let mut consider = |l: Limit, f: f32| {
        rate *= f;
        if f < 0.999 && limit.as_ref().is_none_or(|(_, m)| f < *m) {
            limit = Some((l, f));
        }
    };
    if map.in_bounds(p) {
        let idx = map.idx(p);
        for g in &pd.growth {
            consider(Limit::Field(g.field), g.eval(env.get(g.field, idx)).max(0.0));
        }
    }
    if let Some(c) = &pd.crop {
        let fert = fertility_at(defs, map, p);
        consider(Limit::Fertility, (1.0 + (fert - 1.0) * c.fertility_weight).max(0.0));
    }
    GrowthInfo { rate, limit }
}

/// Harm entries currently active at `p`.
pub fn active_harm<'a>(map: &Map, env: &Env, pd: &'a PlantDef, p: P3) -> Vec<&'a PlantHarm> {
    if !map.in_bounds(p) {
        return Vec::new();
    }
    let idx = map.idx(p);
    pd.harm.iter().filter(|h| h.applies(env.get(h.field, idx))).collect()
}

/// Growth fraction gained per day at rate 1.
pub fn base_growth_per_day(pd: &PlantDef) -> f32 {
    1.0 / pd.grow_days.max(0.01)
}

/// The growing zone (and its crop) at `p`, if any.
pub fn grow_zone_at(zones: &Zones, map: &Map, p: P3) -> Option<(u16, PlantId, bool)> {
    let z = zones.grow(map.get(p)?.zone?)?;
    Some((z.id, z.crop, z.sow))
}

/// Should a farmer sow `crop` here now? (Zone crop, empty cell, fertile
/// enough, and the crop would not currently be harmed.)
pub fn sow_target(defs: &Defs, zones: &Zones, map: &Map, env: &Env, p: P3) -> Option<PlantId> {
    let (_, crop, sow) = grow_zone_at(zones, map, p)?;
    if !sow {
        return None;
    }
    let c = map.get(p)?;
    if c.plant.is_some() || c.building.is_some() || c.item.is_some() || !map.is_walkable(p) {
        return None;
    }
    let pd = &defs.plants[crop];
    let cd = pd.crop.as_ref()?;
    if fertility_at(defs, map, p) < cd.min_fertility || !active_harm(map, env, pd, p).is_empty() {
        return None;
    }
    Some(crop)
}

/// Is the plant on a growing-zone cell where it does not belong (and should
/// be cut)? Ripe harvestable plants are harvested first instead.
pub fn is_foreign(zones: &Zones, map: &Map, pl: &Plant, p: P3) -> bool {
    grow_zone_at(zones, map, p).is_some_and(|(_, crop, _)| crop != pl.def)
}

/// Can this plant be harvested now?
pub fn is_ripe(defs: &Defs, pl: &Plant) -> bool {
    defs.plants[pl.def].harvest.as_ref().is_some_and(|h| pl.growth >= h.min_growth)
}

/// Grows plants, applies harm and healing, and removes dead plants.
pub fn sys_plants(world: &mut World) {
    if !world.res::<Clock>().tick.is_multiple_of(PLANT_INTERVAL) {
        return;
    }
    let defs = world.res::<Rc<Defs>>().clone();
    let dt = PLANT_INTERVAL as f32 / TICKS_PER_DAY as f32;
    let mut dead: Vec<Entity> = Vec::new();
    // (plant label, cause) -> count, for one aggregated log line each.
    let mut deaths: BTreeMap<(String, String), u32> = BTreeMap::new();
    {
        let map = world.res::<Map>();
        let env = world.res::<Env>();
        let pos = world.read::<Position>();
        let mut plants = world.write::<Plant>();
        for (e, pl) in plants.iter_mut() {
            let Some(p) = pos.get(e).map(|p| p.0) else { continue };
            let pd = &defs.plants[pl.def];
            let info = growth_info(&defs, &map, &env, pd, p);
            pl.growth = (pl.growth + info.rate * base_growth_per_day(pd) * dt).min(1.0);
            let harm = active_harm(&map, &env, pd, p);
            if harm.is_empty() {
                pl.health = (pl.health + pd.heal_per_day * dt).min(1.0);
            } else {
                pl.health -= harm.iter().map(|h| h.per_day).sum::<f32>() * dt;
                if pl.health <= 0.0 {
                    dead.push(e);
                    let cause = harm.first().map(|h| h.label.clone()).filter(|l| !l.is_empty());
                    let cause = cause.unwrap_or_else(|| "harsh conditions".into());
                    *deaths.entry((pd.label.clone(), cause)).or_default() += 1;
                }
            }
        }
    }
    for e in dead {
        crate::jobs::remove_plant(world, e);
    }
    for ((label, cause), n) in deaths {
        let text = if n == 1 { format!("A {label} plant died of {cause}.") } else { format!("{n} {label} plants died of {cause}.") };
        crate::jobs::log(world, LogKind::Bad, text);
    }
}
