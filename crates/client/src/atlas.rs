//! Sprite atlas: the baked sheet (tools/bake) plus procedurally generated UI /
//! overlay sprites (font glyphs, diamonds, white pixel), composed on the CPU.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::font;

#[derive(Clone, Copy, Debug, Default)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Anchor (projected ground centre of the cell) relative to the rect's top-left.
    pub ax: i32,
    pub ay: i32,
}

#[derive(Clone, Debug)]
pub struct SpriteSet {
    pub variants: u32,
    pub dirs: u32,
    pub frames: u32,
    pub rects: Vec<Rect>,
}

impl SpriteSet {
    pub fn rect(&self, variant: u32, dir: u32, frame: u32) -> Rect {
        let v = variant % self.variants.max(1);
        let d = dir % self.dirs.max(1);
        let f = frame % self.frames.max(1);
        self.rects[((v * self.dirs + d) * self.frames + f) as usize]
    }
}

fn one() -> u32 {
    1
}

#[derive(Deserialize)]
struct ManSprite {
    #[serde(default = "one")]
    variants: u32,
    dirs: u32,
    frames: u32,
    rects: Vec<(u32, u32, u32, u32, i32, i32)>,
}

#[derive(Deserialize)]
struct Manifest {
    atlas: String,
    #[serde(default)]
    normals: String,
    #[allow(dead_code)]
    width: u32,
    #[allow(dead_code)]
    height: u32,
    sprites: HashMap<String, ManSprite>,
}

/// Small icons for colonist thought bubbles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Food,
    Cloud,
}

#[rustfmt::skip]
const ICON_FOOD: [&str; 7] = [
    "...#...",
    "..#....",
    ".#####.",
    "#######",
    "#######",
    ".#####.",
    "..###..",
];
#[rustfmt::skip]
const ICON_CLOUD: [&str; 7] = [
    ".......",
    "..###..",
    ".#####.",
    "#######",
    ".......",
    ".#.#.#.",
    "#.#.#..",
];

pub struct Atlas {
    pub w: u32,
    pub h: u32,
    pub pixels: Vec<u8>,
    /// Normal map, same layout as `pixels` (rgb = n * 0.5 + 0.5).
    pub normals: Vec<u8>,
    pub sprites: HashMap<String, SpriteSet>,
    pub white: Rect,
    /// Tile-sized filled diamond, anchor at its centre.
    pub diamond: Rect,
    /// Tile-sized diamond outline.
    pub diamond_line: Rect,
    /// Soft ellipse shadow.
    pub shadow: Rect,
    /// Thought bubble background (anchor at the tail tip).
    pub bubble: Rect,
    pub icons: Vec<(Icon, Rect)>,
    /// Floor dirt decals (variants), white; tinted when drawn.
    pub filth: Vec<Rect>,
    pub glyphs: Vec<Rect>,
    pub warnings: Vec<String>,
}

const EXTRA_ROWS: u32 = 64;
const DW: u32 = 48;
const DH: u32 = 24;

fn in_diamond(x: u32, y: u32) -> bool {
    let fx = (x as f32 + 0.5 - DW as f32 / 2.0).abs() / (DW as f32 / 2.0);
    let fy = (y as f32 + 0.5 - DH as f32 / 2.0).abs() / (DH as f32 / 2.0);
    fx + fy <= 1.0
}

impl Atlas {
    pub fn load(dir: &Path) -> Atlas {
        let mut warnings = Vec::new();
        let mut sprites = HashMap::new();
        let mut base: Option<image::RgbaImage> = None;
        let mut nbase: Option<image::RgbaImage> = None;
        match std::fs::read_to_string(dir.join("manifest.ron")) {
            Ok(text) => match ron::from_str::<Manifest>(&text) {
                Ok(m) => {
                    match image::open(dir.join(&m.atlas)) {
                        Ok(img) => base = Some(img.to_rgba8()),
                        Err(e) => warnings.push(format!("atlas image: {e}")),
                    }
                    if !m.normals.is_empty() {
                        match image::open(dir.join(&m.normals)) {
                            Ok(img) => nbase = Some(img.to_rgba8()),
                            Err(e) => warnings.push(format!("normal atlas: {e}")),
                        }
                    }
                    for (name, s) in m.sprites {
                        let rects = s
                            .rects
                            .iter()
                            .map(|&(x, y, w, h, ax, ay)| Rect { x, y, w, h, ax, ay })
                            .collect();
                        sprites.insert(name, SpriteSet { variants: s.variants, dirs: s.dirs, frames: s.frames, rects });
                    }
                }
                Err(e) => warnings.push(format!("manifest: {e}")),
            },
            Err(e) => warnings.push(format!("no sprite manifest ({e}); run tools/bake/bake.sh")),
        }
        let (bw, bh) = base.as_ref().map(|b| b.dimensions()).unwrap_or((0, 0));
        let w = bw.max(1024);
        let h = bh + EXTRA_ROWS;
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        // Default normal: facing the camera.
        let mut normals: Vec<u8> = [128u8, 128, 255, 255].repeat((w * h) as usize);
        for (img, dst) in [(&base, &mut pixels), (&nbase, &mut normals)] {
            if let Some(b) = img
                && b.dimensions() == (bw, bh)
            {
                for y in 0..bh {
                    let src = &b.as_raw()[(y * bw * 4) as usize..((y + 1) * bw * 4) as usize];
                    let d = (y * w * 4) as usize;
                    dst[d..d + src.len()].copy_from_slice(src);
                }
            }
        }
        let mut put = |x: u32, y: u32, c: [u8; 4]| {
            let i = ((y * w + x) * 4) as usize;
            pixels[i..i + 4].copy_from_slice(&c);
        };
        let oy = bh;
        // White block.
        for y in 0..4 {
            for x in 0..4 {
                put(x, oy + y, [255; 4]);
            }
        }
        let white = Rect { x: 1, y: oy + 1, w: 2, h: 2, ax: 0, ay: 0 };
        // Diamonds.
        let (dx0, lx0) = (8, 8 + DW + 4);
        for y in 0..DH {
            for x in 0..DW {
                if in_diamond(x, y) {
                    put(dx0 + x, oy + y, [255; 4]);
                    let edge = [(0i32, 1i32), (0, -1), (1, 0), (-1, 0)].iter().any(|(dx, dy)| {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        nx < 0 || ny < 0 || nx >= DW as i32 || ny >= DH as i32 || !in_diamond(nx as u32, ny as u32)
                    });
                    if edge {
                        put(lx0 + x, oy + y, [255; 4]);
                    }
                }
            }
        }
        let diamond = Rect { x: dx0, y: oy, w: DW, h: DH, ax: DW as i32 / 2, ay: DH as i32 / 2 };
        let diamond_line = Rect { x: lx0, y: oy, w: DW, h: DH, ax: DW as i32 / 2, ay: DH as i32 / 2 };
        // Shadow ellipse 22x8.
        let sx0 = lx0 + DW + 4;
        for y in 0..8 {
            for x in 0..22 {
                let fx = (x as f32 + 0.5 - 11.0) / 11.0;
                let fy = (y as f32 + 0.5 - 4.0) / 4.0;
                if fx * fx + fy * fy <= 1.0 {
                    put(sx0 + x, oy + y, [0, 0, 0, 90]);
                }
            }
        }
        let shadow = Rect { x: sx0, y: oy, w: 22, h: 8, ax: 11, ay: 4 };
        // Thought bubble 13x11 + 3px tail.
        let bx0 = sx0 + 26;
        for y in 0..14u32 {
            for x in 0..13u32 {
                let body = y < 11 && !((x == 0 || x == 12) && (y == 0 || y == 10));
                let tail = (y == 11 && (5..=7).contains(&x)) || (y == 12 && x == 6);
                if !(body || tail) {
                    continue;
                }
                let edge = (y < 11 && (x == 0 || x == 12 || y == 0 || (y == 10 && !(5..=7).contains(&x))))
                    || (body && ((x == 1 || x == 11) && (y == 1 || y == 9)))
                    || (y == 11 && (x == 5 || x == 7))
                    || y == 12;
                put(bx0 + x, oy + y, if edge { [30, 28, 34, 255] } else { [240, 236, 228, 255] });
            }
        }
        let bubble = Rect { x: bx0, y: oy, w: 13, h: 13, ax: 6, ay: 13 };
        let mut icons = Vec::new();
        for (k, (icon, rows)) in [(Icon::Food, ICON_FOOD), (Icon::Cloud, ICON_CLOUD)].iter().enumerate() {
            let ix = bx0 + 16 + k as u32 * 9;
            for (y, row) in rows.iter().enumerate() {
                for (x, ch) in row.chars().enumerate() {
                    if ch == '#' {
                        put(ix + x as u32, oy + y as u32, [255; 4]);
                    }
                }
            }
            icons.push((*icon, Rect { x: ix, y: oy, w: 7, h: 7, ax: 3, ay: 3 }));
        }
        // Filth decals: scattered specks inside the floor diamond.
        let mut filth = Vec::new();
        let fx0 = bx0 + 40;
        let mut seed = 0x9e37_79b9u32;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for v in 0..3u32 {
            let ox = fx0 + v * (DW + 2);
            for _ in 0..22 {
                let x = rnd() % DW;
                let y = rnd() % DH;
                // Keep specks well inside the diamond.
                let fx = (x as f32 + 0.5 - DW as f32 / 2.0).abs() / (DW as f32 / 2.0);
                let fy = (y as f32 + 0.5 - DH as f32 / 2.0).abs() / (DH as f32 / 2.0);
                if fx + fy > 0.75 {
                    continue;
                }
                put(ox + x, oy + y, [255; 4]);
                if rnd() % 3 == 0 && x + 1 < DW {
                    put(ox + x + 1, oy + y, [255; 4]);
                }
            }
            filth.push(Rect { x: ox, y: oy, w: DW, h: DH, ax: DW as i32 / 2, ay: DH as i32 / 2 });
        }
        // Font glyphs, laid out in rows below.
        let mut glyphs = Vec::with_capacity(95);
        for i in 0..95u32 {
            let c = char::from_u32(32 + i).unwrap();
            let gx = 8 + (i % 80) * (font::GLYPH_W + 1);
            let gy = oy + 30 + (i / 80) * (font::GLYPH_H + 1);
            for (row, bits) in font::glyph(c).iter().enumerate() {
                for col in 0..font::GLYPH_W {
                    if bits & (1 << (font::GLYPH_W - 1 - col)) != 0 {
                        put(gx + col, gy + row as u32, [255; 4]);
                    }
                }
            }
            glyphs.push(Rect { x: gx, y: gy, w: font::GLYPH_W, h: font::GLYPH_H, ax: 0, ay: 0 });
        }
        Atlas { w, h, pixels, normals, sprites, white, diamond, diamond_line, shadow, bubble, icons, filth, glyphs, warnings }
    }

    pub fn get(&self, name: &str) -> Option<&SpriteSet> {
        self.sprites.get(name)
    }

    pub fn uv(&self, r: Rect) -> [f32; 4] {
        let (w, h) = (self.w as f32, self.h as f32);
        [r.x as f32 / w, r.y as f32 / h, (r.x + r.w) as f32 / w, (r.y + r.h) as f32 / h]
    }

    pub fn icon(&self, i: Icon) -> Rect {
        self.icons.iter().find(|(k, _)| *k == i).map(|(_, r)| *r).unwrap_or(self.white)
    }

    pub fn glyph(&self, c: char) -> Rect {
        let i = c as u32;
        if (32..=126).contains(&i) { self.glyphs[(i - 32) as usize] } else { self.glyphs[('?' as u32 - 32) as usize] }
    }
}
