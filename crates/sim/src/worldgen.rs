//! Colony map generation: heightmap terrain with a few z-levels, ore and
//! salvage veins, ruined old-world structures, ramps and wild plants.

use crate::defs::{Defs, ItemId, PlantId, TerrainId};
use crate::geom::{Dir4, P3};
use crate::map::Map;
use crate::rng::{SimRng, hash3};

#[derive(Clone, Debug)]
pub struct WorldGenParams {
    pub width: i32,
    pub height: i32,
    pub levels: i32,
}

impl Default for WorldGenParams {
    fn default() -> Self {
        Self { width: 128, height: 128, levels: 5 }
    }
}

pub struct WorldGenResult {
    pub map: Map,
    pub plants: Vec<(P3, PlantId, f32)>,
    pub items: Vec<(P3, ItemId, u32)>,
    pub start: P3,
}

/// Smooth value noise in [0, 1].
pub fn value_noise(seed: u64, x: f32, y: f32) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let fx = x - xi as f32;
    let fy = y - yi as f32;
    let r = |ix: i32, iy: i32| (hash3(seed, ix, iy, 0) >> 40) as f32 / (1u64 << 24) as f32;
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (s(fx), s(fy));
    let a = r(xi, yi) + (r(xi + 1, yi) - r(xi, yi)) * sx;
    let b = r(xi, yi + 1) + (r(xi + 1, yi + 1) - r(xi, yi + 1)) * sx;
    a + (b - a) * sy
}

/// Fractal value noise in roughly [0, 1].
pub fn fbm(seed: u64, x: f32, y: f32, octaves: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut norm = 0.0;
    for o in 0..octaves {
        sum += amp * value_noise(seed.wrapping_add(o as u64 * 7919), x * freq, y * freq);
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / norm
}

struct Mats {
    soil: TerrainId,
    grass: TerrainId,
    stone: TerrainId,
    iron: TerrainId,
    salvage: TerrainId,
    ruin: TerrainId,
}

pub fn generate(defs: &Defs, seed: u64, p: &WorldGenParams) -> WorldGenResult {
    let mut rng = SimRng::new(seed ^ 0xC0_10_1E);
    let t = &defs.terrain;
    let mats = Mats {
        soil: t.expect("soil"),
        grass: t.expect("grass"),
        stone: t.expect("granite"),
        iron: t.expect("iron_ore"),
        salvage: t.expect("salvage"),
        ruin: t.expect("ruin_wall"),
    };
    let (w, h, d) = (p.width, p.height, p.levels.max(3));
    let mut map = Map::new(w, h, d);
    let ground = 2.min(d - 2); // walkable level of ordinary ground
    let hs = rng.next_u64();
    let fs = rng.next_u64();
    let os = rng.next_u64();
    let ss = rng.next_u64();

    // Height per column: number of solid cells (walkable level = height).
    let mut heights = vec![ground; (w * h) as usize];
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    for y in 0..h {
        for x in 0..w {
            let n = fbm(hs, x as f32 / 28.0, y as f32 / 28.0, 4);
            // Flatter near the centre so the colony has room to start.
            let dist = (((x as f32 - cx) / cx).powi(2) + ((y as f32 - cy) / cy).powi(2)).sqrt();
            let n = n + (dist - 0.45).max(0.0) * 0.35 - 0.06;
            let lift = if n > 0.74 {
                3
            } else if n > 0.66 {
                2
            } else if n > 0.57 {
                1
            } else {
                0
            };
            heights[(y * w + x) as usize] = (ground + lift).min(d);
        }
    }

    for y in 0..h {
        for x in 0..w {
            let ht = heights[(y * w + x) as usize];
            let fert = fbm(fs, x as f32 / 14.0, y as f32 / 14.0, 3);
            for z in 0..ht {
                let top = z == ht - 1;
                let mut mat = if top && ht == ground {
                    if fert > 0.42 { mats.grass } else { mats.soil }
                } else if z >= ground - 1 && ht == ground {
                    mats.soil
                } else {
                    mats.stone
                };
                if mat == mats.stone {
                    let ore = fbm(os, x as f32 / 6.0, y as f32 / 6.0 + z as f32 * 31.0, 2);
                    if ore > 0.72 {
                        mat = mats.iron;
                    }
                }
                let salv = fbm(ss, x as f32 / 9.0 + z as f32 * 17.0, y as f32 / 9.0, 2);
                if z < ht - 1 && salv > 0.74 {
                    mat = mats.salvage;
                }
                map.set_solid(P3::new(x, y, z), Some(mat));
            }
        }
    }

    // Ramps on 1-level steps.
    for y in 0..h {
        for x in 0..w {
            let ht = heights[(y * w + x) as usize];
            if ht >= d {
                continue;
            }
            let p = P3::new(x, y, ht);
            for dir in Dir4::ALL {
                let q = p.step(dir);
                if q.x < 0 || q.y < 0 || q.x >= w || q.y >= h {
                    continue;
                }
                let qh = heights[(q.y * w + q.x) as usize];
                if qh == ht + 1 && qh < d && hash3(seed, x, y, dir as i32) % 100 < 30 {
                    map.cell_mut(p).ramp = Some(dir);
                    break;
                }
            }
        }
    }

    let start = find_start(&map, &heights, ground);

    // Ruins of the old world: broken rectangular walls on flat ground, with salvage inside.
    let mut items = Vec::new();
    let scrap = defs.items.find("scrap");
    let mut ruins = 0;
    for _attempt in 0..60 {
        if ruins >= 5 {
            break;
        }
        let rw = rng.range_i32(5, 10);
        let rh = rng.range_i32(5, 9);
        let rx = rng.range_i32(2, w - rw - 3);
        let ry = rng.range_i32(2, h - rh - 3);
        // Keep the landing site clear.
        if (rx - start.x).abs() < 14 && (ry - start.y).abs() < 14 {
            continue;
        }
        let flat = (ry..ry + rh).all(|yy| (rx..rx + rw).all(|xx| heights[(yy * w + xx) as usize] == ground));
        if !flat {
            continue;
        }
        for yy in ry..ry + rh {
            for xx in rx..rx + rw {
                let edge = xx == rx || yy == ry || xx == rx + rw - 1 || yy == ry + rh - 1;
                let pc = P3::new(xx, yy, ground);
                if edge && !rng.chance(0.3) {
                    map.set_solid(pc, Some(mats.ruin));
                    if map.in_bounds(pc.up()) && rng.chance(0.25) {
                        map.set_solid(pc.up(), Some(mats.ruin));
                    }
                } else if !edge
                    && let Some(scrap) = scrap
                    && rng.chance(0.06)
                {
                    items.push((pc, scrap, rng.range_i32(3, 12) as u32));
                }
            }
        }
        ruins += 1;
    }

    // Plants.
    let mut plants = Vec::new();
    let mut weights: Vec<(PlantId, f32)> = defs.plants.iter().map(|(id, pd)| (id, pd.wild_density)).collect();
    weights.retain(|(_, d)| *d > 0.0);
    for y in 0..h {
        for x in 0..w {
            let ht = heights[(y * w + x) as usize];
            if ht >= d || ht == 0 {
                continue;
            }
            let pc = P3::new(x, y, ht);
            if !map.is_walkable(pc) || map.cell(pc).ramp.is_some() {
                continue;
            }
            if (pc.x - start.x).abs() <= 2 && (pc.y - start.y).abs() <= 2 {
                continue;
            }
            let below = map.cell(pc.down()).solid.unwrap();
            if !defs.terrain[below].fertile {
                continue;
            }
            let fert = fbm(fs, x as f32 / 14.0, y as f32 / 14.0, 3);
            for (pid, dens) in &weights {
                if rng.f32() < dens * fert * 1.4 {
                    plants.push((pc, *pid, rng.range_f32(0.4, 1.0)));
                    break;
                }
            }
        }
    }

    WorldGenResult { map, plants, items, start }
}

fn find_start(map: &Map, heights: &[i32], ground: i32) -> P3 {
    let (w, h) = (map.w, map.h);
    let (cx, cy) = (w / 2, h / 2);
    // Spiral-ish search for a flat 7x7 area of ordinary ground near the centre.
    for r in 0..(w.max(h) / 2) {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dy.abs() != r {
                    continue;
                }
                let (x, y) = (cx + dx, cy + dy);
                if x < 4 || y < 4 || x >= w - 4 || y >= h - 4 {
                    continue;
                }
                let ok = (-3..=3).all(|oy| (-3..=3).all(|ox| heights[((y + oy) * w + x + ox) as usize] == ground));
                if ok {
                    return P3::new(x, y, ground);
                }
            }
        }
    }
    P3::new(cx, cy, map.surface_z(cx, cy, map.d - 1).unwrap_or(ground))
}
