//! Isometric projection with four camera angles.
//!
//! Map coordinates are continuous (cell (x, y) spans [x, x+1)); "world pixels"
//! are the un-zoomed iso pixel space sprites are authored in.
//! Must match tools/bake/bake.py: 48x24 diamonds, 30 px per level, angle `a`
//! puts the camera at azimuth 45 + 90a deg (a = 0: south-east).

pub const HALF_W: f32 = 24.0;
pub const HALF_H: f32 = 12.0;
pub const LEVEL_H: f32 = 30.0;

#[derive(Clone, Copy, Debug)]
pub struct Proj {
    pub angle: u8,
    pub w: f32,
    pub h: f32,
}

impl Proj {
    /// Map -> rotated coordinates (the frame where +x/+y go down-screen).
    pub fn rot(&self, x: f32, y: f32) -> (f32, f32) {
        match self.angle % 4 {
            0 => (x, y),
            1 => (self.h - y, x),
            2 => (self.w - x, self.h - y),
            _ => (y, self.w - x),
        }
    }

    pub fn unrot(&self, rx: f32, ry: f32) -> (f32, f32) {
        match self.angle % 4 {
            0 => (rx, ry),
            1 => (ry, self.h - rx),
            2 => (self.w - rx, self.h - ry),
            _ => (self.w - ry, rx),
        }
    }

    /// Continuous map point at height z (in levels) -> world pixels.
    pub fn project(&self, x: f32, y: f32, z: f32) -> (f32, f32) {
        let (rx, ry) = self.rot(x, y);
        ((rx - ry) * HALF_W, (rx + ry) * HALF_H - z * LEVEL_H)
    }

    /// Ground centre of a cell -> world pixels.
    pub fn cell_center(&self, x: i32, y: i32, z: i32) -> (f32, f32) {
        self.project(x as f32 + 0.5, y as f32 + 0.5, z as f32)
    }

    /// Draw-order depth of a continuous map point (larger = nearer).
    pub fn depth(&self, x: f32, y: f32) -> f32 {
        let (rx, ry) = self.rot(x, y);
        rx + ry
    }

    /// World pixels on the plane at height z -> continuous map point.
    pub fn unproject(&self, sx: f32, sy: f32, z: f32) -> (f32, f32) {
        let u = sx / HALF_W;
        let v = (sy + z * LEVEL_H) / HALF_H;
        let rx = (u + v) / 2.0;
        let ry = (v - u) / 2.0;
        self.unrot(rx, ry)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    /// World pixel at the screen centre.
    pub center: (f32, f32),
    /// Transient offset (world px) that decays to zero, for smooth transitions.
    pub anim: (f32, f32),
    pub zoom: i32,
    pub angle: u8,
    pub view_z: i32,
}

impl Camera {
    /// Integer screen offset so sprites land on whole pixels.
    pub fn offset(&self, screen: (u32, u32)) -> (f32, f32) {
        let z = self.zoom as f32;
        let (cx, cy) = (self.center.0 + self.anim.0, self.center.1 + self.anim.1);
        ((screen.0 as f32 / 2.0 - cx * z).round(), (screen.1 as f32 / 2.0 - cy * z).round())
    }

    pub fn screen_to_world(&self, screen: (u32, u32), p: (f32, f32)) -> (f32, f32) {
        let (ox, oy) = self.offset(screen);
        let z = self.zoom as f32;
        ((p.0 - ox) / z, (p.1 - oy) / z)
    }
}
