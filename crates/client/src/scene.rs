//! Builds the sorted list of world sprites from the simulation state.

use std::rc::Rc;

use ecs::Entity;
use sim::components::*;
use sim::defs::Defs;
use sim::jobs::{JobKind, Toil};
use sim::map::{Designation, Map};
use sim::rng::hash3;
use sim::{P3, Sim};

use crate::atlas::{Atlas, Rect, SpriteSet};
use crate::gfx::Quad;
use crate::iso::{Camera, LEVEL_H, Proj};

#[derive(Clone, Copy)]
struct Spr {
    key: u64,
    quad: Quad,
}

fn key(depth: f32, z: i32, layer: u8) -> u64 {
    let d = ((depth + 64.0) * 256.0).max(0.0) as u64;
    (d << 24) | (((z + 16) as u64 & 0xff) << 16) | ((layer as u64) << 8)
}

/// Overlay colours.
pub const C_MINE: [f32; 4] = [1.0, 0.6, 0.2, 0.55];
pub const C_CHOP: [f32; 4] = [0.95, 0.85, 0.3, 0.9];
pub const C_HARVEST: [f32; 4] = [0.75, 0.45, 0.95, 0.9];
pub const C_STAIRS: [f32; 4] = [0.3, 0.85, 0.95, 0.6];
pub const C_ZONE: [f32; 4] = [0.95, 0.82, 0.35, 0.22];
pub const C_GROW: [f32; 4] = [0.45, 0.9, 0.35, 0.2];
pub const C_SELECT: [f32; 4] = [0.95, 0.75, 0.3, 1.0];
pub const C_ROOF: [f32; 4] = [0.55, 0.75, 1.0, 0.8];
pub const C_NOROOF: [f32; 4] = [1.0, 0.45, 0.4, 0.8];

/// Environment overlay shown over the floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    None,
    Field(sim::defs::EnvFieldId),
    Rooms,
}

/// Blue → cyan → green → yellow → red.
fn heat(t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0) * 4.0;
    let stops = [[0.2, 0.35, 1.0], [0.2, 0.85, 0.95], [0.3, 0.9, 0.35], [0.95, 0.9, 0.25], [0.95, 0.25, 0.2]];
    let i = (t.floor() as usize).min(3);
    let f = t - i as f32;
    let (a, b) = (stops[i], stops[i + 1]);
    [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preview {
    Valid,
    Invalid,
}

/// Where a colonist is drawn this frame (for bubbles, particles, text).
#[derive(Clone, Copy, Debug)]
pub struct PawnAnchor {
    pub entity: Entity,
    /// World px of the feet.
    pub feet: (f32, f32),
    /// Sprite height above the feet, in world px.
    pub height: f32,
    /// World px of the current work target, if working.
    pub work_at: Option<(f32, f32)>,
    pub job: Option<JobKind>,
}

pub struct Scene {
    terrain_key: Option<(u64, u8, i32)>,
    terrain: Vec<Spr>,
    /// Screen-space rects of pawns this frame, for picking.
    pub pawn_rects: Vec<(Entity, [f32; 4])>,
    pub anchors: Vec<PawnAnchor>,
    /// Point lights this frame: (map pos, radius, colour).
    pub lights: Vec<([f32; 3], f32, [f32; 3])>,
    pub sprites_drawn: usize,
}

pub struct FrameParams<'a> {
    pub cam: Camera,
    pub screen: (u32, u32),
    pub selected: Option<Entity>,
    pub hover: Option<P3>,
    pub preview: &'a [(P3, Preview)],
    pub preview_sprite: Option<(&'a str, u8)>,
    pub time: f32,
    pub overlay: Overlay,
    /// Draw all walls cut down (otherwise only walls on the view level).
    pub walls_down: bool,
}

fn ramp_sprite(defs: &Defs, map: &Map, p: P3) -> String {
    let below = map.get(p.down()).and_then(|c| c.solid).map(|t| defs.terrain[t].sprite.as_str()).unwrap_or("block_soil");
    below.replacen("block_", "ramp_", 1)
}

fn variant_of(p: P3) -> u32 {
    (hash3(0x5eed, p.x, p.y, p.z) >> 33) as u32
}

fn rgb(c: [u8; 3]) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0]
}

/// Lighting flags for a sprite.
#[derive(Clone, Copy)]
struct Lit {
    outdoor: bool,
    wpos: [f32; 3],
}

impl Lit {
    fn at(map: &Map, p: P3, dz: f32) -> Lit {
        Lit { outdoor: !map.is_roofed(p), wpos: [p.x as f32 + 0.5, p.y as f32 + 0.5, p.z as f32 + dz] }
    }
}

fn make_quad(atlas: &Atlas, r: Rect, at: (f32, f32), color: [f32; 4], lit: Option<Lit>) -> Quad {
    let (light, wpos) = match lit {
        Some(l) => ([1.0, if l.outdoor { 1.0 } else { 0.0 }, 0.0, 0.0], [l.wpos[0], l.wpos[1], l.wpos[2], 0.0]),
        None => ([0.0; 4], [0.0; 4]),
    };
    Quad {
        pos: [at.0.round() - r.ax as f32, at.1.round() - r.ay as f32],
        size: [r.w as f32, r.h as f32],
        uv: atlas.uv(r),
        color,
        light,
        wpos,
    }
}

/// Which tool a colonist holds for a job.
fn tool_for(kind: JobKind) -> Option<&'static str> {
    match kind {
        JobKind::Mine | JobKind::DigStairs => Some("pickaxe"),
        JobKind::Chop => Some("axe"),
        JobKind::Build | JobKind::Craft => Some("hammer"),
        JobKind::Harvest | JobKind::Cut => Some("sickle"),
        _ => None,
    }
}

/// Union of two anchored rects (only w/h/ax/ay are meaningful).
fn union(a: Rect, b: Rect) -> Rect {
    let (x0, y0) = ((-a.ax).min(-b.ax), (-a.ay).min(-b.ay));
    let (x1, y1) = ((a.w as i32 - a.ax).max(b.w as i32 - b.ax), (a.h as i32 - a.ay).max(b.h as i32 - b.ay));
    Rect { x: 0, y: 0, w: (x1 - x0) as u32, h: (y1 - y0) as u32, ax: -x0, ay: -y0 }
}

impl Scene {
    pub fn new() -> Self {
        Scene {
            terrain_key: None,
            terrain: Vec::new(),
            pawn_rects: Vec::new(),
            anchors: Vec::new(),
            lights: Vec::new(),
            sprites_drawn: 0,
        }
    }

    pub fn invalidate(&mut self) {
        self.terrain_key = None;
    }

    fn dim(view_z: i32, floor: i32) -> f32 {
        1.0 - 0.14 * ((view_z - 1 - floor).clamp(0, 4) as f32)
    }

    #[allow(clippy::too_many_arguments)]
    fn put(out: &mut Vec<Spr>, atlas: &Atlas, set: Option<&SpriteSet>, var: u32, dir: u32, frame: u32, at: (f32, f32), color: [f32; 4], k: u64, lit: Option<Lit>) {
        match set {
            Some(s) => out.push(Spr { key: k, quad: make_quad(atlas, s.rect(var, dir, frame), at, color, lit) }),
            // Missing sprite: magenta diamond placeholder.
            None => out.push(Spr { key: k, quad: make_quad(atlas, atlas.diamond, at, [1.0, 0.0, 1.0, 0.8], None) }),
        }
    }

    fn put_rect(out: &mut Vec<Spr>, atlas: &Atlas, r: Rect, at: (f32, f32), color: [f32; 4], k: u64) {
        out.push(Spr { key: k, quad: make_quad(atlas, r, at, color, None) });
    }

    fn rebuild_terrain(&mut self, sim: &Sim, atlas: &Atlas, proj: &Proj, view_z: i32) {
        let defs = sim.defs();
        let map = sim.world.res::<Map>();
        let zones = sim.world.res::<sim::resources::Zones>();
        let mut out = Vec::with_capacity(40_000);
        let maxz = view_z.min(map.d - 1);
        for z in 0..=maxz {
            for y in 0..map.h {
                for x in 0..map.w {
                    let p = P3::new(x, y, z);
                    let c = map.cell(p);
                    let depth = proj.depth(x as f32 + 0.5, y as f32 + 0.5);
                    if let Some(t) = c.solid {
                        let cut = z == maxz && maxz < map.d - 1;
                        let exposed = cut
                            || !map.is_solid(p.up())
                            || [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| !map.is_solid(p.offset(*dx, *dy, 0)));
                        if !exposed {
                            continue;
                        }
                        let d = Self::dim(view_z, z + 1);
                        let at = proj.cell_center(x, y, z);
                        let lit = Lit::at(&map, p, 0.9);
                        Self::put(&mut out, atlas, atlas.get(&defs.terrain[t].sprite), variant_of(p), 0, 0, at, [d, d, d, 1.0], key(depth, z, 0), Some(lit));
                        let top = proj.cell_center(x, y, z + 1);
                        if cut {
                            Self::put_rect(&mut out, atlas, atlas.diamond, top, [0.05, 0.05, 0.07, 0.55], key(depth, z, 1));
                        }
                        if c.designation == Designation::Mine {
                            Self::put_rect(&mut out, atlas, atlas.diamond, top, C_MINE, key(depth, z, 2));
                        }
                    } else {
                        let at = proj.cell_center(x, y, z);
                        if let Some(dir) = c.ramp {
                            let d = Self::dim(view_z, z);
                            let name = ramp_sprite(&defs, &map, p);
                            let rd = (dir.index() as u32 + proj.angle as u32) % 4;
                            Self::put(&mut out, atlas, atlas.get(&name), 0, rd, 0, at, [d, d, d, 1.0], key(depth, z, 0), Some(Lit::at(&map, p, 0.4)));
                        }
                        if let Some(zid) = c.zone {
                            let col = if zones.grow(zid).is_some() { C_GROW } else { C_ZONE };
                            Self::put_rect(&mut out, atlas, atlas.diamond, at, col, key(depth, z, 1));
                        }
                        let dc = match c.designation {
                            Designation::Chop => Some(C_CHOP),
                            Designation::Harvest => Some(C_HARVEST),
                            Designation::DigStairs => Some(C_STAIRS),
                            Designation::BuildRoof => Some(C_ROOF),
                            Designation::RemoveRoof => Some(C_NOROOF),
                            _ => None,
                        };
                        if let Some(col) = dc {
                            Self::put_rect(&mut out, atlas, atlas.diamond_line, at, col, key(depth, z, 6));
                        }
                    }
                }
            }
        }
        self.terrain = out;
    }

    pub fn build(&mut self, sim: &Sim, atlas: &Atlas, fp: &FrameParams) -> Vec<Quad> {
        let map_dims = {
            let m = sim.world.res::<Map>();
            (m.w, m.h, m.revision)
        };
        let proj = Proj { angle: fp.cam.angle, w: map_dims.0 as f32, h: map_dims.1 as f32 };
        let view_z = fp.cam.view_z;
        let tkey = (map_dims.2, fp.cam.angle, view_z);
        if self.terrain_key != Some(tkey) {
            self.rebuild_terrain(sim, atlas, &proj, view_z);
            self.terrain_key = Some(tkey);
        }
        let defs: Rc<Defs> = sim.defs();
        let w = &sim.world;
        let map = w.res::<Map>();
        let mut dynm: Vec<Spr> = Vec::with_capacity(4096);
        let pos = w.read::<Position>();
        let depth_of = |p: P3| proj.depth(p.x as f32 + 0.5, p.y as f32 + 0.5);

        // Plants.
        for (e, pl) in w.read::<Plant>().iter() {
            let Some(p) = pos.get(e).map(|p| p.0) else { continue };
            if p.z > view_z {
                continue;
            }
            let pd = &defs.plants[pl.def];
            let ripe = pd.harvest.as_ref().is_some_and(|h| pl.growth >= h.min_growth);
            let ripe_set = atlas.get(&format!("{}_ripe", pd.sprite));
            let young_set = atlas.get(&format!("{}_young", pd.sprite));
            let set = match (ripe, pl.growth < 0.5) {
                (true, _) if ripe_set.is_some() => ripe_set,
                (_, true) if young_set.is_some() => young_set,
                _ => atlas.get(&pd.sprite),
            };
            let d = Self::dim(view_z, p.z);
            Self::put(&mut dynm, atlas, set, variant_of(p), 0, 0, proj.cell_center(p.x, p.y, p.z), [d, d, d, 1.0], key(depth_of(p), p.z, 3), Some(Lit::at(&map, p, 0.6)));
        }
        // Items.
        for (e, it) in w.read::<Item>().iter() {
            let Some(p) = pos.get(e).map(|p| p.0) else { continue };
            if p.z > view_z {
                continue;
            }
            let d = Self::dim(view_z, p.z);
            Self::put(&mut dynm, atlas, atlas.get(&defs.items[it.def].sprite), 0, 0, 0, proj.cell_center(p.x, p.y, p.z), [d, d, d, 1.0], key(depth_of(p), p.z, 2), Some(Lit::at(&map, p, 0.2)));
        }
        // Buildings and blueprints; collect lights.
        self.lights.clear();
        let blds = w.read::<Building>();
        let bps = w.read::<Blueprint>();
        let all_blds = blds.iter().map(|(e, b)| (e, b.def, b.rot, false)).chain(bps.iter().map(|(e, b)| (e, b.def, b.rot, true)));
        for (e, def, rot, blue) in all_blds {
            let Some(p) = pos.get(e).map(|p| p.0) else { continue };
            if p.z > view_z {
                continue;
            }
            let bd = &defs.buildings[def];
            let d = Self::dim(view_z, p.z);
            let color = if blue { [0.45 * d, 0.7 * d, 1.0 * d, 0.55] } else { [d, d, d, 1.0] };
            let dir = (rot as u32 + proj.angle as u32) % 4;
            let lit = if blue { None } else { Some(Lit::at(&map, p, 0.5)) };
            // Walls and doors on the viewed level are drawn cut down so rooms are visible.
            let cut = matches!(bd.kind, sim::defs::BuildingKind::Wall | sim::defs::BuildingKind::Door) && (p.z == view_z || fp.walls_down);
            let low = cut.then(|| atlas.get(&format!("{}_low", bd.sprite))).flatten();
            let filled = w.get::<sim::death::Grave>(e).is_some_and(|g| g.occupant.is_some());
            let filled = filled.then(|| atlas.get(&format!("{}_filled", bd.sprite))).flatten();
            let set = low.or(filled).or_else(|| atlas.get(&bd.sprite));
            Self::put(&mut dynm, atlas, set, variant_of(p), dir, 0, proj.cell_center(p.x, p.y, p.z), color, key(depth_of(p), p.z, 3), lit);
            if !blue && let Some(l) = &bd.light {
                let wobble = 0.5 + 0.5 * (fp.time * 9.0 + p.x as f32 * 1.7).sin() * (fp.time * 5.3 + p.y as f32).cos();
                let flick = 1.0 - l.flicker * wobble;
                self.lights.push((
                    [p.x as f32 + 0.5, p.y as f32 + 0.5, p.z as f32 + l.height],
                    l.radius,
                    [l.color.0 * flick, l.color.1 * flick, l.color.2 * flick],
                ));
            }
        }

        // Colonists: layered, tinted sprites plus held tools.
        self.pawn_rects.clear();
        self.anchors.clear();
        let (ox, oy) = fp.cam.offset(fp.screen);
        let zoom = fp.cam.zoom as f32;
        let pawns = w.read::<Pawn>();
        let movers = w.read::<Mover>();
        let facings = w.read::<Facing>();
        let invs = w.read::<Inventory>();
        let items = w.read::<Item>();
        let mental = w.read::<MentalState>();
        let jobs = w.read::<CurrentJob>();
        let looks = w.read::<Appearance>();
        for (e, pawn) in pawns.iter() {
            let Some(p) = pos.get(e).map(|p| p.0) else { continue };
            let (fx, fy, fz, moving, zs) = match movers.get(e).and_then(|m| m.current_step().map(|n| (m, n))) {
                Some((m, n)) => {
                    let t = m.frac();
                    let a = m.from;
                    (a.x as f32 + (n.x - a.x) as f32 * t, a.y as f32 + (n.y - a.y) as f32 * t, a.z as f32 + (n.z - a.z) as f32 * t, true, a.z.max(n.z))
                }
                None => (p.x as f32, p.y as f32, p.z as f32, false, p.z),
            };
            if zs > view_z {
                continue;
            }
            let at = proj.project(fx + 0.5, fy + 0.5, fz);
            // Carried colonists ride on their carrier's shoulders.
            let carried = sim::medical::carrier_of(w, e).is_some();
            let at = if carried { (at.0, at.1 - 14.0) } else { at };
            let depth = proj.depth(fx + 0.5, fy + 0.5);
            let facing = facings.get(e).map(|f| f.0).unwrap_or(4) as u32;
            let dir = (facing + 2 * proj.angle as u32) % 8;
            let job = jobs.get(e).map(|j| &j.0);
            // Lying down: asleep, resting in bed, downed or being carried.
            let sleeping = jobs.get(e).is_some_and(|j| j.is_sleeping() || matches!(j.0.kind, JobKind::Rest | JobKind::Downed))
                || carried
                || sim::health::is_downed(w, e);
            let work_target = job.and_then(|j| match j.toils.get(j.step) {
                Some(Toil::Work(t)) => Some(*t),
                _ => None,
            });
            let frame = if moving {
                1 + ((fp.time * 8.0) as u32 % 4)
            } else if sleeping {
                5
            } else if work_target.is_some() {
                6 + ((fp.time * 3.0 + e.index() as f32 * 0.37) as u32 % 2)
            } else {
                0
            };
            let race = &defs.races[pawn.race];
            let look = looks.get(e).cloned().unwrap_or_default();
            let d = Self::dim(view_z, p.z);
            let red = if mental.contains(e) { [1.0, 0.75, 0.75] } else { [1.0; 3] };
            let shade = |c: [f32; 4]| [c[0] * d * red[0], c[1] * d * red[1], c[2] * d * red[2], 1.0];
            let lit = Some(Lit { outdoor: !map.is_roofed(p), wpos: [fx + 0.5, fy + 0.5, fz + 0.6] });
            if fp.selected == Some(e) {
                Self::put_rect(&mut dynm, atlas, atlas.diamond_line, at, C_SELECT, key(depth, zs, 4));
            }
            if !sleeping {
                Self::put_rect(&mut dynm, atlas, atlas.shadow, at, [1.0; 4], key(depth, zs, 4) + 1);
            }
            let base = format!("pawn_{}_s{}", race.sprite, look.style);
            let layers: [(&str, [f32; 4]); 5] = [
                ("bottom", rgb(look.bottom)),
                ("top", rgb(look.top)),
                ("skin", rgb(look.skin)),
                ("hair", rgb(look.hair)),
                ("detail", [1.0; 4]),
            ];
            let mut bounds: Option<Rect> = None;
            for (i, (layer, col)) in layers.iter().enumerate() {
                let Some(s) = atlas.get(&format!("{base}_{layer}")) else { continue };
                let r = s.rect(0, dir, frame);
                if r.w > 1 {
                    bounds = Some(bounds.map_or(r, |b| union(b, r)));
                }
                dynm.push(Spr { key: key(depth, zs, 4) + 2 + i as u64, quad: make_quad(atlas, r, at, shade(*col), lit) });
            }
            if bounds.is_none() {
                // Atlas without layered colonists: placeholder.
                Self::put(&mut dynm, atlas, None, 0, 0, 0, at, [1.0; 4], key(depth, zs, 4) + 2, None);
            }
            if frame >= 6
                && let Some(tool) = job.and_then(|j| tool_for(j.kind))
                && let Some(s) = atlas.get(&format!("tool_{}_{}", race.sprite, tool))
            {
                dynm.push(Spr { key: key(depth, zs, 4) + 8, quad: make_quad(atlas, s.rect(0, dir, frame - 6), at, shade([1.0; 4]), lit) });
            }
            if let Some(it) = invs.get(e).and_then(|i| i.carrying).and_then(|c| items.get(c)) {
                Self::put(&mut dynm, atlas, atlas.get(&defs.items[it.def].sprite), 0, 0, 0, (at.0, at.1 - 16.0), shade([1.0; 4]), key(depth, zs, 4) + 9, lit);
            }
            let b = bounds.unwrap_or(Rect { x: 0, y: 0, w: 24, h: 40, ax: 12, ay: 38 });
            let x0 = at.0.round() - b.ax as f32;
            let y0 = at.1.round() - b.ay as f32;
            self.pawn_rects.push((e, [x0 * zoom + ox, y0 * zoom + oy, b.w as f32 * zoom, b.h as f32 * zoom]));
            let work_at = work_target.and_then(|t| {
                use sim::jobs::WorkTarget::*;
                let cell = match t {
                    Mine(c) | DigStairs(c) | Roof(c, _) | Clean(c) | Sow(c, _) => Some(c),
                    Chop(x) | Harvest(x) | Cut(x) | Build(x) | Craft { bench: x, .. } | Tend(x) | Bury(x) => {
                        pos.get(x).map(|p| p.0)
                    }
                };
                cell.map(|c| {
                    let (cx, cy) = proj.cell_center(c.x, c.y, c.z);
                    (cx, cy - LEVEL_H * 0.5)
                })
            });
            self.anchors.push(PawnAnchor { entity: e, feet: at, height: b.ay as f32, work_at, job: job.map(|j| j.kind) });
        }
        drop((pawns, movers, facings, invs, items, mental, jobs, looks, blds, bps, pos));

        // Hover, previews.
        if let Some(h) = fp.hover {
            dynm.push(Spr {
                key: key(depth_of(h), h.z, 7),
                quad: make_quad(atlas, atlas.diamond_line, proj.cell_center(h.x, h.y, h.z), [1.0, 1.0, 1.0, 0.7], None),
            });
        }
        for (p, kind) in fp.preview {
            let col = match kind {
                Preview::Valid => [0.4, 0.95, 0.5, 0.45],
                Preview::Invalid => [0.95, 0.3, 0.3, 0.35],
            };
            let ground_z = if map.is_solid(*p) { p.z + 1 } else { p.z };
            dynm.push(Spr { key: key(depth_of(*p), p.z, 7), quad: make_quad(atlas, atlas.diamond, proj.cell_center(p.x, p.y, ground_z), col, None) });
            if let (Some((sprite, rot)), Preview::Valid) = (fp.preview_sprite, kind)
                && let Some(set) = atlas.get(sprite)
            {
                let dir = (rot as u32 + proj.angle as u32) % 4;
                Self::put(&mut dynm, atlas, Some(set), 0, dir, 0, proj.cell_center(p.x, p.y, p.z), [0.6, 1.0, 0.7, 0.6], key(depth_of(*p), p.z, 3) + 5, None);
            }
        }
        self.overlay_and_decals(sim, atlas, &proj, fp, &map, &mut dynm);
        drop(map);

        // Cull + sort.
        let (sw, sh) = (fp.screen.0 as f32, fp.screen.1 as f32);
        let visible = |q: &Quad| {
            let x0 = q.pos[0] * zoom + ox;
            let y0 = q.pos[1] * zoom + oy;
            x0 < sw && y0 < sh && x0 + q.size[0] * zoom > 0.0 && y0 + q.size[1] * zoom > 0.0
        };
        let mut all: Vec<Spr> = self.terrain.iter().copied().filter(|s| visible(&s.quad)).collect();
        all.extend(dynm.into_iter().filter(|s| visible(&s.quad)));
        all.sort_by_key(|s| s.key);
        self.sprites_drawn = all.len();
        all.into_iter().map(|s| s.quad).collect()
    }
}

impl Scene {
    /// Filth decals on dirty floors and the optional environment overlay.
    fn overlay_and_decals(&self, sim: &Sim, atlas: &Atlas, proj: &Proj, fp: &FrameParams, map: &Map, out: &mut Vec<Spr>) {
        let defs = sim.defs();
        let env = sim.world.res::<sim::env::Env>();
        let rooms = sim.world.res::<sim::env::Rooms>();
        let filth = defs.core.field_filth;
        let view_z = fp.cam.view_z.min(map.d - 1);
        let (ox, oy) = fp.cam.offset(fp.screen);
        let zoom = fp.cam.zoom as f32;
        let on_screen = |at: (f32, f32)| {
            let sx = at.0 * zoom + ox;
            let sy = at.1 * zoom + oy;
            sx > -64.0 && sy > -64.0 && sx < fp.screen.0 as f32 + 64.0 && sy < fp.screen.1 as f32 + 64.0
        };
        for y in 0..map.h {
            for x in 0..map.w {
                // Top floor cell of this column at or below the view level.
                let Some(z) = (0..=view_z).rev().find(|&z| {
                    let p = P3::new(x, y, z);
                    !map.is_solid(p) && map.is_supported(p)
                }) else {
                    continue;
                };
                let p = P3::new(x, y, z);
                let at = proj.cell_center(x, y, z);
                if !on_screen(at) {
                    continue;
                }
                let i = map.idx(p);
                let depth = proj.depth(x as f32 + 0.5, y as f32 + 0.5);
                if let Some(f) = filth {
                    let v = env.get(f, i);
                    if v > 0.3 && !atlas.filth.is_empty() {
                        let r = atlas.filth[(variant_of(p) as usize) % atlas.filth.len()];
                        let a = (v / 1.5).min(0.85);
                        out.push(Spr { key: key(depth, z, 1) + 1, quad: make_quad(atlas, r, at, [0.22, 0.17, 0.12, a], Some(Lit::at(map, p, 0.0))) });
                    }
                }
                let col = match fp.overlay {
                    Overlay::None => continue,
                    Overlay::Field(f) => {
                        let fd = &defs.env_fields[f];
                        let (lo, hi) = fd.overlay.unwrap_or((0.0, 1.0));
                        let t = (env.get(f, i) - lo) / (hi - lo).max(1e-3);
                        let c = heat(t);
                        [c[0], c[1], c[2], 0.42]
                    }
                    Overlay::Rooms => match rooms.indoor_room_at(map, p) {
                        Some(_) => {
                            let id = rooms.cell_room[i];
                            let h = hash3(7, id as i32, 0, 0);
                            let c = heat(((h >> 40) & 0xff) as f32 / 255.0);
                            [c[0], c[1], c[2], 0.45]
                        }
                        None if map.cell(p).roof => [0.6, 0.7, 0.9, 0.3],
                        None => continue,
                    },
                };
                out.push(Spr { key: key(depth, z, 7), quad: make_quad(atlas, atlas.diamond, at, col, None) });
            }
        }
    }
}

/// Picks the cell under a world-pixel position, scanning down from `view_z`.
/// Returns the solid cell at the view level (cut face) or the open floor cell
/// standing on top of a lower block.
pub fn pick_cell(map: &Map, proj: &Proj, world: (f32, f32), view_z: i32) -> Option<P3> {
    let top = view_z.min(map.d - 1);
    for z in (0..=top).rev() {
        let (x, y) = proj.unproject(world.0, world.1, (z + 1) as f32);
        let (cx, cy) = (x.floor() as i32, y.floor() as i32);
        let p = P3::new(cx, cy, z);
        if !map.in_bounds(p) {
            continue;
        }
        if map.is_solid(p) {
            return Some(if z == top { p } else { p.up() });
        }
    }
    None
}
