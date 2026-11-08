//! Environment: data-defined per-cell fields (temperature, beauty, filth,
//! light, ...), room detection, and weather.
//!
//! Fields are declared in `defs/env_fields`. Things emit into fields via `env`
//! lists on building, item and plant defs. Conditions (`FieldBelow`,
//! `RoomFieldAbove`, `TooLow`, ...) let thoughts, needs, quirks and stats react
//! to any field, so new environmental factors are mostly new def files.

use std::f32::consts::TAU;
use std::rc::Rc;

use ecs::World;
use serde::{Deserialize, Serialize};

use crate::components::{Building, Item, Plant, Position};
use crate::defs::*;
use crate::geom::{NEIGHBORS4, P3};
use crate::map::Map;
use crate::resources::{Clock, GameLog, LogKind, TICKS_PER_HOUR};
use crate::rng::SimRng;

/// Ticks between field updates.
pub const ENV_INTERVAL: u64 = 60;
pub const NO_ROOM: u32 = u32::MAX;
/// Regions larger than this are never "indoors".
const MAX_ROOM_CELLS: usize = 1200;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum EnvOp {
    Add(EnvFieldId, u32, f32),
    Set(EnvFieldId, u32, f32),
}

/// Per-cell field values, one grid per `EnvFieldDef` (indexed by def id).
///
/// Grids only change during `update_env`; changes made between updates
/// (foot traffic, cleaning) are queued in `pending` and applied at the next
/// update, so cached room aggregates are always a pure function of the grids.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Env {
    pub grids: Vec<Vec<f32>>,
    pub pending: Vec<EnvOp>,
}

impl Env {
    #[inline]
    pub fn get(&self, f: EnvFieldId, idx: usize) -> f32 {
        self.grids.get(f.index()).and_then(|g| g.get(idx)).copied().unwrap_or(0.0)
    }

    /// Queues an addition, applied at the next update.
    pub fn queue_add(&mut self, f: EnvFieldId, idx: usize, v: f32) {
        self.pending.push(EnvOp::Add(f, idx as u32, v));
    }

    /// Queues an assignment, applied at the next update.
    pub fn queue_set(&mut self, f: EnvFieldId, idx: usize, v: f32) {
        self.pending.push(EnvOp::Set(f, idx as u32, v));
    }

    fn apply_pending(&mut self) {
        for op in std::mem::take(&mut self.pending) {
            let (f, i, v, set) = match op {
                EnvOp::Add(f, i, v) => (f, i, v, false),
                EnvOp::Set(f, i, v) => (f, i, v, true),
            };
            if let Some(c) = self.grids.get_mut(f.index()).and_then(|g| g.get_mut(i as usize)) {
                *c = if set { v } else { *c + v };
            }
        }
    }

    /// Grows grids to match the field defs and map size (new game, hot reload).
    pub fn ensure(&mut self, defs: &Defs, cells: usize, ambient: &[f32]) {
        for (id, fd) in defs.env_fields.iter() {
            let init = match fd.kind {
                FieldKind::Dynamic => ambient.get(id.index()).copied().unwrap_or(fd.default),
                FieldKind::Static => fd.default,
            };
            if self.grids.len() <= id.index() {
                self.grids.push(vec![init; cells]);
            } else if self.grids[id.index()].len() != cells {
                self.grids[id.index()] = vec![init; cells];
            }
        }
    }
}

// --------------------------------------------------------------------- rooms

#[derive(Clone, Debug, Default)]
pub struct Room {
    pub z: i32,
    pub cells: Vec<u32>,
    /// Enclosed by walls/doors/rock and fully roofed.
    pub indoor: bool,
    pub role: Option<RoomRoleId>,
    /// Aggregated field values (indexed by field id).
    pub fields: Vec<f32>,
    pub impressiveness: f32,
    /// Building counts by def (for roles).
    pub buildings: Vec<(BuildingId, u32)>,
}

/// Connected open regions per level. Derived from the map (not saved).
#[derive(Clone, Debug, Default)]
pub struct Rooms {
    pub struct_rev: u64,
    pub cell_room: Vec<u32>,
    pub rooms: Vec<Room>,
}

impl Rooms {
    pub fn room_at(&self, map: &Map, p: P3) -> Option<&Room> {
        if !map.in_bounds(p) {
            return None;
        }
        let r = *self.cell_room.get(map.idx(p))?;
        if r == NO_ROOM { None } else { self.rooms.get(r as usize) }
    }

    pub fn indoor_room_at(&self, map: &Map, p: P3) -> Option<&Room> {
        self.room_at(map, p).filter(|r| r.indoor)
    }
}

/// Recomputes regions with a 4-connected flood fill per level. Walls, doors
/// and solid terrain separate regions; door cells belong to no region.
pub fn compute_rooms(map: &Map) -> Rooms {
    let n = map.len();
    let mut cell_room = vec![NO_ROOM; n];
    let mut rooms = Vec::new();
    let mut stack = Vec::new();
    for start in 0..n {
        if cell_room[start] != NO_ROOM {
            continue;
        }
        let sp = map.pos_of(start);
        if map.is_room_boundary(sp) {
            continue;
        }
        let id = rooms.len() as u32;
        let mut room = Room { z: sp.z, ..Default::default() };
        let mut edge = false;
        let mut roofed = true;
        cell_room[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let p = map.pos_of(i);
            room.cells.push(i as u32);
            if p.x == 0 || p.y == 0 || p.x == map.w - 1 || p.y == map.h - 1 {
                edge = true;
            }
            if roofed && !map.is_roofed(p) {
                roofed = false;
            }
            for (dx, dy) in NEIGHBORS4 {
                let q = p.offset(dx, dy, 0);
                if !map.in_bounds(q) || map.is_room_boundary(q) {
                    continue;
                }
                let qi = map.idx(q);
                if cell_room[qi] == NO_ROOM {
                    cell_room[qi] = id;
                    stack.push(qi);
                }
            }
        }
        room.indoor = !edge && roofed && room.cells.len() <= MAX_ROOM_CELLS;
        rooms.push(room);
    }
    Rooms { struct_rev: map.struct_rev, cell_room, rooms }
}

// -------------------------------------------------------------------- weather

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WeatherState {
    pub current: Option<WeatherId>,
    pub ticks_left: u32,
}

impl WeatherState {
    pub fn light(&self, defs: &Defs) -> f32 {
        self.current.map_or(1.0, |w| defs.weather[w].light)
    }
}

pub fn sys_weather(world: &mut World) {
    let defs = world.res::<Rc<Defs>>().clone();
    if defs.weather.is_empty() {
        return;
    }
    let done = {
        let mut ws = world.res_mut::<WeatherState>();
        ws.ticks_left = ws.ticks_left.saturating_sub(1);
        ws.ticks_left == 0
    };
    if !done {
        return;
    }
    let (next, hours) = {
        let mut rng = world.res_mut::<SimRng>();
        let weights: Vec<f32> = defs.weather.items.iter().map(|w| w.weight).collect();
        let i = rng.weighted(&weights).unwrap_or(0);
        let wd = &defs.weather.items[i];
        (WeatherId::from_index(i), rng.range_f32(wd.hours.0, wd.hours.1.max(wd.hours.0)))
    };
    let prev = world.res::<WeatherState>().current;
    {
        let mut ws = world.res_mut::<WeatherState>();
        ws.current = Some(next);
        ws.ticks_left = (hours * TICKS_PER_HOUR as f32).max(1.0) as u32;
    }
    if prev.is_some() && prev != Some(next) {
        let tick = world.res::<Clock>().tick;
        world.res_mut::<GameLog>().push(tick, LogKind::Info, format!("The weather turns: {}.", defs.weather[next].label));
    }
}

// --------------------------------------------------------------------- fields

/// Outdoor ambient value of each field right now (climate + weather).
pub fn ambient_values(defs: &Defs, clock: &Clock, weather: &WeatherState) -> Vec<f32> {
    defs.env_fields
        .iter()
        .map(|(id, fd)| {
            let mut v = match fd.ambient {
                Ambient::Constant(c) => c,
                Ambient::Climate(mean, season, day) => {
                    let yp = clock.year_phase();
                    let h = clock.hour_f();
                    mean + season * (TAU * (yp - 0.125)).sin() + day * (TAU * (h - 9.0) / 24.0).sin()
                }
            };
            if let Some(w) = weather.current {
                for (f, off) in &defs.weather[w].ambient {
                    if *f == id {
                        v += off;
                    }
                }
            }
            v
        })
        .collect()
}

fn ground_value(fd: &EnvFieldDef) -> Option<f32> {
    fd.ground.or(match fd.ambient {
        Ambient::Climate(mean, _, _) => Some(mean),
        Ambient::Constant(_) => None,
    })
}

/// (cell, emit) pairs for every emitting thing on the map.
fn collect_emitters(world: &World, defs: &Defs) -> Vec<(P3, EnvEmit)> {
    let pos = world.read::<Position>();
    let mut out = Vec::new();
    // Buildings that cast light also feed the gameplay "light" field.
    let light_field = defs.env_fields.find("light");
    for (e, b) in world.read::<Building>().iter() {
        if let Some(p) = pos.get(e) {
            let bd = &defs.buildings[b.def];
            out.extend(bd.env.iter().map(|em| (p.0, em.clone())));
            if let (Some(f), Some(l)) = (light_field, &bd.light) {
                let strength = (l.color.0 + l.color.1 + l.color.2) / 3.0;
                out.push((p.0, EnvEmit { field: f, amount: strength.min(1.2), radius: l.radius * 0.8, target: None }));
            }
        }
    }
    for (e, it) in world.read::<Item>().iter() {
        if let Some(p) = pos.get(e) {
            out.extend(defs.items[it.def].env.iter().map(|em| (p.0, em.clone())));
        }
    }
    for (e, pl) in world.read::<Plant>().iter() {
        if let Some(p) = pos.get(e) {
            out.extend(defs.plants[pl.def].env.iter().map(|em| (p.0, em.clone())));
        }
    }
    out
}

/// Visits open cells within `radius` of `p` on the same level with falloff 0..1.
fn for_each_in_radius(map: &Map, p: P3, radius: f32, mut f: impl FnMut(usize, f32)) {
    let r = radius.max(0.0).ceil() as i32;
    for dy in -r..=r {
        for dx in -r..=r {
            let q = p.offset(dx, dy, 0);
            if !map.in_bounds(q) || map.is_solid(q) {
                continue;
            }
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            if d > radius + 0.01 {
                continue;
            }
            f(map.idx(q), 1.0 - d / (radius + 1.0));
        }
    }
}

/// One environment update: dynamic fields (emit, diffuse, relax), static
/// fields (inputs + emitters), then room aggregates, roles and impressiveness.
pub fn update_env(world: &World) {
    let defs = world.res::<Rc<Defs>>().clone();
    let map = world.res::<Map>();
    let clock = world.res::<Clock>().clone();
    let weather = world.res::<WeatherState>().clone();
    let ambient = ambient_values(&defs, &clock, &weather);
    let emitters = collect_emitters(world, &defs);
    let mut env = world.res_mut::<Env>();
    let mut rooms = world.res_mut::<Rooms>();
    env.ensure(&defs, map.len(), &ambient);
    env.apply_pending();
    let n = map.len();
    // Per-cell structure, computed once per update.
    let open: Vec<bool> = map.cells().iter().map(|c| c.solid.is_none()).collect();
    let roofed: Vec<bool> = (0..n).map(|i| open[i] && map.is_roofed(map.pos_of(i))).collect();
    let daylight = clock.daylight() * weather.light(&defs);

    for (fid, fd) in defs.env_fields.iter() {
        let amb = ambient[fid.index()];
        match fd.kind {
            FieldKind::Dynamic => {
                let grid = &mut env.grids[fid.index()];
                for (p, em) in emitters.iter().filter(|(_, em)| em.field == fid) {
                    for_each_in_radius(&map, *p, em.radius, |i, k| match em.target {
                        Some(t) => grid[i] += (t - grid[i]) * (em.amount * k).min(1.0),
                        None => grid[i] += em.amount * k,
                    });
                }
                if fd.diffusion > 0.0 {
                    let ground = ground_value(fd);
                    let src = grid.clone();
                    for i in 0..n {
                        if !open[i] {
                            continue;
                        }
                        let p = map.pos_of(i);
                        let wall_here = map.is_room_boundary(p);
                        let mut delta = 0.0;
                        for (dx, dy) in NEIGHBORS4 {
                            let q = p.offset(dx, dy, 0);
                            if !map.in_bounds(q) {
                                continue;
                            }
                            let qi = map.idx(q);
                            if !open[qi] {
                                if let Some(g) = ground {
                                    delta += fd.diffusion * 0.5 * (g - src[i]);
                                }
                                continue;
                            }
                            // Walls and doors insulate (doors leak a little).
                            let c = map.cell(q);
                            let rate = match c.building_kind {
                                Some(BuildingKind::Wall) => 0.0,
                                Some(BuildingKind::Door) => 0.25,
                                _ if wall_here => 0.0,
                                _ => 1.0,
                            };
                            delta += fd.diffusion * rate * (src[qi] - src[i]);
                        }
                        grid[i] = src[i] + delta;
                    }
                }
                for i in 0..n {
                    if !open[i] {
                        continue;
                    }
                    let pull = if roofed[i] { fd.indoor_pull } else { fd.outdoor_pull };
                    grid[i] = fd.clamp(grid[i] + (amb - grid[i]) * pull);
                }
            }
            FieldKind::Static => {
                let mut grid = vec![fd.default; n];
                for i in 0..n {
                    if !open[i] {
                        continue;
                    }
                    for (input, k) in &fd.inputs {
                        grid[i] += k * match input {
                            FieldInput::Field(f) => env.get(*f, i),
                            FieldInput::Daylight => {
                                if roofed[i] {
                                    0.0
                                } else {
                                    daylight
                                }
                            }
                        };
                    }
                }
                for (p, em) in emitters.iter().filter(|(_, em)| em.field == fid) {
                    let src_room = rooms.cell_room.get(map.idx(*p)).copied().unwrap_or(NO_ROOM);
                    for_each_in_radius(&map, *p, em.radius, |i, k| {
                        if fd.same_room_only && rooms.cell_room.get(i).copied().unwrap_or(NO_ROOM) != src_room {
                            return;
                        }
                        grid[i] += em.amount * k;
                    });
                }
                for v in &mut grid {
                    *v = fd.clamp(*v);
                }
                env.grids[fid.index()] = grid;
            }
        }
    }

    aggregate_rooms(&defs, &env, &mut rooms);
}

/// Room field aggregates and impressiveness (pure function of the grids).
pub fn aggregate_rooms(defs: &Defs, env: &Env, rooms: &mut Rooms) {
    let nf = defs.env_fields.len();
    for room in rooms.rooms.iter_mut() {
        room.fields = vec![0.0; nf];
        if !room.indoor {
            continue;
        }
        for (fid, fd) in defs.env_fields.iter() {
            let vals = room.cells.iter().map(|i| env.get(fid, *i as usize));
            room.fields[fid.index()] = match fd.room_agg {
                RoomAgg::Average => vals.sum::<f32>() / room.cells.len().max(1) as f32,
                RoomAgg::Sum => vals.sum(),
                RoomAgg::Min => vals.fold(f32::INFINITY, f32::min),
                RoomAgg::Max => vals.fold(f32::NEG_INFINITY, f32::max),
            };
        }
        let size = room.cells.len() as f32;
        room.impressiveness = defs.env_fields.iter().map(|(id, fd)| fd.impressiveness * room.fields[id.index()]).sum::<f32>()
            + 2.5 * size.min(48.0).sqrt()
            - 8.0;
    }
}

/// Recomputes rooms (on structural change) and their building-based roles.
pub fn refresh_rooms(world: &World) {
    let map = world.res::<Map>();
    let stale = world.res::<Rooms>().struct_rev != map.struct_rev;
    if !stale {
        return;
    }
    let defs = world.res::<Rc<Defs>>().clone();
    let mut rooms = compute_rooms(&map);
    let pos = world.read::<Position>();
    for (e, b) in world.read::<Building>().iter() {
        let Some(p) = pos.get(e).map(|p| p.0) else { continue };
        let r = rooms.cell_room.get(map.idx(p)).copied().unwrap_or(NO_ROOM);
        // Buildings standing in a doorway or wall cell don't count.
        if r == NO_ROOM {
            continue;
        }
        let room = &mut rooms.rooms[r as usize];
        match room.buildings.iter_mut().find(|(d, _)| *d == b.def) {
            Some((_, c)) => *c += 1,
            None => room.buildings.push((b.def, 1)),
        }
    }
    for room in rooms.rooms.iter_mut().filter(|r| r.indoor) {
        let mut best: Option<(f32, RoomRoleId)> = None;
        for (rid, rd) in defs.room_roles.iter() {
            let count: u32 = room.buildings.iter().filter(|(d, _)| rd.buildings.contains(d)).map(|(_, c)| *c).sum();
            if count >= rd.min_count && rd.max_count.is_none_or(|m| count <= m) && best.is_none_or(|(p, _)| rd.priority > p) {
                best = Some((rd.priority, rid));
            }
        }
        room.role = best.map(|(_, r)| r);
    }
    aggregate_rooms(&defs, &world.res::<Env>(), &mut rooms);
    drop((map, pos));
    *world.res_mut::<Rooms>() = rooms;
}

pub fn sys_env(world: &mut World) {
    refresh_rooms(world);
    let tick = world.res::<Clock>().tick;
    if tick.is_multiple_of(ENV_INTERVAL) {
        update_env(world);
    }
}

/// Field value at a cell (convenience for UI and tools).
pub fn field_at(world: &World, f: EnvFieldId, p: P3) -> f32 {
    let map = world.res::<Map>();
    if !map.in_bounds(p) {
        return 0.0;
    }
    world.res::<Env>().get(f, map.idx(p))
}
