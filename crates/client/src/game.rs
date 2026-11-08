//! Game state, input handling, player tools and the HUD.

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Instant, SystemTime};

use ecs::Entity;
use sim::components::*;
use sim::defs::{BuildingId, BuildingKind, Defs};
use sim::jobs::PawnDebug;
use sim::map::{Designation, Map};
use sim::orders::{self, DesignateTool};
use sim::path::Pathfinder;
use sim::resources::{Clock, GameLog, LogKind, SimMeta, TICKS_PER_SECOND, Zones};
use sim::defs::WeatherVisual;
use sim::env::{Env, Rooms, WeatherState};
use sim::stats::StatCache;
use sim::{P3, Sim};
use winit::keyboard::KeyCode;

use crate::atlas::Atlas;
use crate::fx::Fx;
use crate::gfx::{Lighting, Quad, Xf};
use crate::iso::{Camera, Proj};
use crate::scene::{self, FrameParams, Overlay, Preview, Scene};
use crate::ui::{Input, Ui, theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Designate(DesignateTool),
    Stockpile,
    GrowZone,
    ClearZone,
    Build(BuildingId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    None,
    Pawn(Entity),
    Thing(Entity),
    /// A stockpile or growing zone (by id).
    Zone(u16),
}

#[derive(Default)]
pub struct FrameInput {
    pub ui: Input,
    pub released: bool,
    pub keys_pressed: Vec<KeyCode>,
    pub keys_held: HashSet<KeyCode>,
    pub middle_drag: (f32, f32),
}

pub struct Game {
    pub sim: Sim,
    pub cam: Camera,
    scene: Scene,
    fx: Fx,
    pub speed: u8,
    pub paused: bool,
    tick_acc: f64,
    pub tool: Tool,
    build_rot: u8,
    drag_start: Option<P3>,
    pub selection: Selection,
    hover: Option<P3>,
    pub show_work: bool,
    pub show_build: bool,
    pub show_debug: bool,
    pub overlay: Overlay,
    pub walls_down: bool,
    status: Option<(String, f32)>,
    time: f32,
    // perf
    last_tick_ms: f32,
    last_ticks: u32,
    fps: f32,
    // hot reload
    pub defs_dir: PathBuf,
    defs_stamp: Option<SystemTime>,
    reload_timer: f32,
    pub save_path: PathBuf,
    pub ui_scale: f32,
}

fn dir_stamp(dir: &std::path::Path) -> Option<SystemTime> {
    let mut latest: Option<SystemTime> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                latest = Some(latest.map_or(m, |l: SystemTime| l.max(m)));
            }
        }
    }
    latest
}

const SPEED_MULT: [f64; 5] = [0.0, 1.0, 2.0, 4.0, 8.0];

pub struct FrameOut {
    pub world: Vec<Quad>,
    pub world_xf: Xf,
    pub ui: Vec<Quad>,
    pub ui_xf: Xf,
    pub lighting: Lighting,
}

/// Matrix taking map-space vectors (x east, y south, z up) to camera space
/// for camera angle `a` (must match the bake camera: elevation 30 deg).
pub fn map_to_cam(angle: u8) -> [[f32; 3]; 3] {
    let (c, s) = (30f32.to_radians().cos(), 30f32.to_radians().sin());
    let r2 = std::f32::consts::FRAC_1_SQRT_2;
    let conv = |v: [f32; 3]| -> [f32; 3] {
        let (dx, dy) = (v[0], v[1]);
        let (rx, ry) = match angle % 4 {
            0 => (dx, dy),
            1 => (-dy, dx),
            2 => (-dx, -dy),
            _ => (dy, -dx),
        };
        let h = (rx + ry) * r2;
        [(rx - ry) * r2, v[2] * c - h * s, v[2] * s + h * c]
    };
    let cols = [conv([1.0, 0.0, 0.0]), conv([0.0, 1.0, 0.0]), conv([0.0, 0.0, 1.0])];
    [[cols[0][0], cols[1][0], cols[2][0]], [cols[0][1], cols[1][1], cols[2][1]], [cols[0][2], cols[1][2], cols[2][2]]]
}

/// Sun, sky and point lights for the current time of day and camera angle.
pub fn lighting(clock: &Clock, angle: u8, lights: Vec<([f32; 3], f32, [f32; 3])>, weather_light: f32, visual: WeatherVisual) -> Lighting {
    let m = map_to_cam(angle);
    let apply = |v: [f32; 3]| {
        let r = [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2]);
        let l = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt().max(1e-5);
        [r[0] / l, r[1] / l, r[2] / l]
    };
    let day = clock.daylight();
    let h = clock.hour_f();
    // The sun rises in the east (+x), passes south (+y) and sets in the west.
    let th = ((h - 6.0) / 12.0 * std::f32::consts::PI).clamp(0.0, std::f32::consts::PI);
    let sun = [th.cos(), 0.45, th.sin().max(0.2)];
    let moon = [-0.3, 0.3, 0.9];
    let dusk = (1.0 - (day - 0.35).abs() / 0.35).clamp(0.0, 1.0) * (day > 0.0) as u8 as f32;
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
    // Weather dims direct sun more than the sky.
    let wl = weather_light.clamp(0.0, 1.0);
    let sun_col = if day > 0.02 {
        let c = lerp([1.0, 0.97, 0.9], [1.0, 0.7, 0.45], dusk * 0.7);
        let k = 0.62 * day * wl * wl;
        [c[0] * k, c[1] * k, c[2] * k]
    } else {
        [0.12, 0.15, 0.24]
    };
    let mut sky = lerp([0.2, 0.22, 0.36], [0.56, 0.56, 0.58], day);
    let sk = 0.6 + 0.4 * wl;
    sky = [sky[0] * sk, sky[1] * sk, sky[2] * sk];
    if visual == WeatherVisual::AetherStorm {
        sky = [sky[0] * 1.05 + 0.06, sky[1] * 0.85, sky[2] * 1.15 + 0.08];
    }
    Lighting {
        sun_dir: apply(if day > 0.02 { sun } else { moon }),
        sun_col,
        sky,
        cave: [0.17, 0.16, 0.2],
        map_to_cam: m,
        lights,
    }
}

impl Game {
    pub fn new(sim: Sim, defs_dir: PathBuf) -> Game {
        let (start, w, h, d) = {
            let m = sim.world.res::<Map>();
            (sim.world.res::<SimMeta>().start, m.w, m.h, m.d)
        };
        let proj = Proj { angle: 0, w: w as f32, h: h as f32 };
        let center = proj.cell_center(start.x, start.y, start.z);
        Game {
            sim,
            cam: Camera { center, anim: (0.0, 0.0), zoom: 2, angle: 0, view_z: d - 1 },
            scene: Scene::new(),
            fx: Fx::new(),
            speed: 1,
            paused: false,
            tick_acc: 0.0,
            tool: Tool::Select,
            build_rot: 0,
            drag_start: None,
            selection: Selection::None,
            hover: None,
            show_work: false,
            show_build: false,
            show_debug: false,
            overlay: Overlay::None,
            walls_down: false,
            status: None,
            time: 0.0,
            last_tick_ms: 0.0,
            last_ticks: 0,
            fps: 0.0,
            defs_stamp: dir_stamp(&defs_dir),
            defs_dir,
            reload_timer: 0.0,
            save_path: PathBuf::from("saves/quick.sav"),
            ui_scale: 2.0,
        }
    }

    fn defs(&self) -> Rc<Defs> {
        self.sim.defs()
    }

    fn proj(&self) -> Proj {
        let m = self.sim.world.res::<Map>();
        Proj { angle: self.cam.angle, w: m.w as f32, h: m.h as f32 }
    }

    pub fn flash(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), 3.0));
    }

    /// Rotates the camera keeping the map point under the screen centre fixed.
    pub fn rotate(&mut self, delta: i32) {
        let proj = self.proj();
        let z = self.cam.view_z as f32;
        let (x, y) = proj.unproject(self.cam.center.0, self.cam.center.1, z);
        self.cam.angle = ((self.cam.angle as i32 + delta).rem_euclid(4)) as u8;
        let proj = self.proj();
        self.cam.center = proj.project(x, y, z);
    }

    pub fn focus(&mut self, p: P3) {
        let proj = self.proj();
        self.cam.center = proj.cell_center(p.x, p.y, p.z);
    }

    fn set_view_z(&mut self, z: i32) {
        let d = self.sim.world.res::<Map>().d;
        let nz = z.clamp(0, d - 1);
        // Keep the screen centre over the same map point.
        let proj = self.proj();
        let (x, y) = proj.unproject(self.cam.center.0, self.cam.center.1, self.cam.view_z as f32);
        self.cam.view_z = nz;
        let old = self.cam.center;
        self.cam.center = proj.project(x, y, nz as f32);
        // Slide smoothly to the new level instead of jumping.
        self.cam.anim.0 += old.0 - self.cam.center.0;
        self.cam.anim.1 += old.1 - self.cam.center.1;
    }

    pub fn save(&mut self) {
        match self.sim.save_bytes() {
            Ok(b) => {
                if let Some(dir) = self.save_path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                match std::fs::write(&self.save_path, b) {
                    Ok(_) => self.flash(format!("Saved to {}", self.save_path.display())),
                    Err(e) => self.flash(format!("Save failed: {e}")),
                }
            }
            Err(e) => self.flash(format!("Save failed: {e}")),
        }
    }

    pub fn load(&mut self) {
        match std::fs::read(&self.save_path) {
            Ok(b) => match self.sim.load_bytes(&b) {
                Ok(skipped) => {
                    self.selection = Selection::None;
                    self.scene.invalidate();
                    let msg = if skipped.is_empty() { "Loaded.".to_string() } else { format!("Loaded (skipped: {})", skipped.join(", ")) };
                    self.flash(msg);
                }
                Err(e) => self.flash(format!("Load failed: {e}")),
            },
            Err(e) => self.flash(format!("Load failed: {e}")),
        }
    }

    fn check_hot_reload(&mut self, dt: f32) {
        self.reload_timer += dt;
        if self.reload_timer < 1.0 {
            return;
        }
        self.reload_timer = 0.0;
        let stamp = dir_stamp(&self.defs_dir);
        if stamp != self.defs_stamp {
            self.defs_stamp = stamp;
            let old = self.defs();
            match Defs::load_dir(&self.defs_dir, Some(&old)) {
                Ok(d) => {
                    self.sim.set_defs(d);
                    self.scene.invalidate();
                    self.flash("Defs reloaded.");
                }
                Err(e) => self.flash(format!("Def error: {e}")),
            }
        }
    }

    fn handle_keys(&mut self, input: &FrameInput, dt: f32) {
        let pan = 500.0 * dt / self.cam.zoom as f32;
        let held = |k: KeyCode| input.keys_held.contains(&k);
        if held(KeyCode::KeyW) || held(KeyCode::ArrowUp) {
            self.cam.center.1 -= pan;
        }
        if held(KeyCode::KeyS) || held(KeyCode::ArrowDown) {
            self.cam.center.1 += pan;
        }
        if held(KeyCode::KeyA) || held(KeyCode::ArrowLeft) {
            self.cam.center.0 -= pan;
        }
        if held(KeyCode::KeyD) || held(KeyCode::ArrowRight) {
            self.cam.center.0 += pan;
        }
        self.cam.center.0 -= input.middle_drag.0 / self.cam.zoom as f32;
        self.cam.center.1 -= input.middle_drag.1 / self.cam.zoom as f32;
        for k in &input.keys_pressed {
            match k {
                KeyCode::KeyQ => self.rotate(-1),
                KeyCode::KeyE => self.rotate(1),
                KeyCode::PageUp | KeyCode::Period => self.set_view_z(self.cam.view_z + 1),
                KeyCode::PageDown | KeyCode::Comma => self.set_view_z(self.cam.view_z - 1),
                KeyCode::Space => self.paused = !self.paused,
                KeyCode::Digit1 => (self.speed, self.paused) = (1, false),
                KeyCode::Digit2 => (self.speed, self.paused) = (2, false),
                KeyCode::Digit3 => (self.speed, self.paused) = (3, false),
                KeyCode::Digit4 => (self.speed, self.paused) = (4, false),
                KeyCode::F3 => self.show_debug = !self.show_debug,
                KeyCode::F5 => self.save(),
                KeyCode::F9 => self.load(),
                KeyCode::KeyR => self.build_rot = (self.build_rot + 1) % 4,
                KeyCode::KeyM => self.tool = Tool::Designate(DesignateTool::Mine),
                KeyCode::KeyC => self.tool = Tool::Designate(DesignateTool::Chop),
                KeyCode::KeyH => self.tool = Tool::Designate(DesignateTool::Harvest),
                KeyCode::KeyX => self.tool = Tool::Designate(DesignateTool::Cancel),
                KeyCode::KeyZ => self.tool = Tool::Stockpile,
                KeyCode::KeyG => self.tool = Tool::GrowZone,
                KeyCode::KeyB => self.show_build = !self.show_build,
                KeyCode::Tab => self.show_work = !self.show_work,
                KeyCode::KeyO => self.cycle_overlay(),
                KeyCode::KeyV => {
                    self.walls_down = !self.walls_down;
                    self.flash(if self.walls_down { "Walls cut down" } else { "Walls up" });
                }
                KeyCode::Equal | KeyCode::NumpadAdd => self.cam.zoom = (self.cam.zoom + 1).min(6),
                KeyCode::Minus | KeyCode::NumpadSubtract => self.cam.zoom = (self.cam.zoom - 1).max(1),
                KeyCode::Escape => {
                    if self.tool != Tool::Select {
                        self.tool = Tool::Select;
                    } else {
                        self.selection = Selection::None;
                        self.show_work = false;
                        self.show_build = false;
                    }
                    self.drag_start = None;
                }
                _ => {}
            }
        }
    }

    fn advance_sim(&mut self, dt: f32) {
        if self.paused || self.speed == 0 {
            self.last_ticks = 0;
            return;
        }
        self.tick_acc += dt as f64 * TICKS_PER_SECOND as f64 * SPEED_MULT[self.speed as usize];
        self.tick_acc = self.tick_acc.min(TICKS_PER_SECOND as f64 * 8.0);
        let start = Instant::now();
        let mut n = 0;
        while self.tick_acc >= 1.0 {
            self.sim.tick();
            self.tick_acc -= 1.0;
            n += 1;
            if start.elapsed().as_secs_f32() > 0.025 {
                self.tick_acc = 0.0;
                break;
            }
        }
        self.last_ticks = n;
        if n > 0 {
            self.last_tick_ms = start.elapsed().as_secs_f32() * 1000.0 / n as f32;
        }
    }

    /// Cells covered by a drag from `a` to `b` on a's level.
    fn rect_cells(a: P3, b: P3) -> Vec<P3> {
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        let mut v = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                v.push(P3::new(x, y, a.z));
            }
        }
        v
    }

    fn tool_cells(&self) -> Vec<P3> {
        let Some(h) = self.hover else { return Vec::new() };
        match (self.tool, self.drag_start) {
            (Tool::Select, _) => Vec::new(),
            (Tool::Build(def), Some(s)) => {
                let wall = self.defs().buildings[def].kind == BuildingKind::Wall;
                let cells = Self::rect_cells(s, P3::new(h.x, h.y, s.z));
                if wall {
                    let (x0, x1) = (s.x.min(h.x), s.x.max(h.x));
                    let (y0, y1) = (s.y.min(h.y), s.y.max(h.y));
                    cells.into_iter().filter(|p| p.x == x0 || p.x == x1 || p.y == y0 || p.y == y1).collect()
                } else {
                    vec![P3::new(h.x, h.y, s.z)]
                }
            }
            (_, Some(s)) => Self::rect_cells(s, P3::new(h.x, h.y, s.z)),
            (_, None) => vec![h],
        }
    }

    fn cell_valid(&self, p: P3) -> bool {
        let w = &self.sim.world;
        match self.tool {
            Tool::Designate(t) => orders::designation_valid(w, p, t),
            Tool::Stockpile => orders::stockpile_cell_valid(w, p),
            Tool::GrowZone => orders::grow_zone_cell_valid(w, p),
            Tool::ClearZone => w.res::<Map>().get(p).is_some_and(|c| c.zone.is_some()),
            Tool::Build(def) => orders::can_place(w, p, def),
            Tool::Select => false,
        }
    }

    fn apply_tool(&mut self, cells: &[P3]) {
        let w = &mut self.sim.world;
        let n = match self.tool {
            Tool::Designate(t) => orders::designate(w, cells, t),
            Tool::Stockpile => orders::create_stockpile(w, cells).map(|_| cells.len()).unwrap_or(0),
            Tool::GrowZone => match orders::create_grow_zone(w, cells) {
                Some(id) => {
                    // Show the crop picker right away.
                    self.selection = Selection::Zone(id);
                    cells.len()
                }
                None => 0,
            },
            Tool::ClearZone => {
                orders::remove_zone_cells(w, cells);
                cells.len()
            }
            Tool::Build(def) => cells.iter().filter(|p| orders::place_blueprint(w, **p, def, self.build_rot).is_some()).count(),
            Tool::Select => 0,
        };
        if n == 0 {
            self.flash("Nothing to do there.");
        }
    }

    fn world_input(&mut self, input: &FrameInput, screen: (u32, u32)) {
        let world = self.cam.screen_to_world(screen, input.ui.mouse);
        let proj = self.proj();
        self.hover = {
            let map = self.sim.world.res::<Map>();
            scene::pick_cell(&map, &proj, world, self.cam.view_z)
        };
        if input.ui.right_pressed {
            if self.tool != Tool::Select {
                self.tool = Tool::Select;
                self.drag_start = None;
            } else {
                self.selection = Selection::None;
            }
            return;
        }
        match self.tool {
            Tool::Select => {
                if input.ui.pressed {
                    let (mx, my) = input.ui.mouse;
                    let pawn = self
                        .scene
                        .pawn_rects
                        .iter()
                        .rev()
                        .find(|(_, r)| mx >= r[0] && my >= r[1] && mx < r[0] + r[2] && my < r[1] + r[3])
                        .map(|(e, _)| *e);
                    self.selection = match pawn {
                        Some(e) => Selection::Pawn(e),
                        None => match self.hover {
                            Some(h) => {
                                let map = self.sim.world.res::<Map>();
                                let c = map.cell(h);
                                c.building
                                    .or(c.item)
                                    .or(c.plant)
                                    .map(Selection::Thing)
                                    .or(c.zone.map(Selection::Zone))
                                    .unwrap_or(Selection::None)
                            }
                            None => Selection::None,
                        },
                    };
                }
            }
            _ => {
                if input.ui.pressed {
                    self.drag_start = self.hover;
                }
                if input.released
                    && self.drag_start.is_some()
                {
                    let cells = self.tool_cells();
                    self.apply_tool(&cells);
                    self.drag_start = None;
                }
            }
        }
    }

    /// Runs one frame: input, simulation, UI, world. Returns render batches.
    pub fn frame(&mut self, atlas: &Atlas, dt: f32, input: &FrameInput, screen: (u32, u32)) -> FrameOut {
        self.time += dt;
        let decay = (-dt * 12.0).exp();
        self.cam.anim = (self.cam.anim.0 * decay, self.cam.anim.1 * decay);
        if self.cam.anim.0.abs() < 0.3 && self.cam.anim.1.abs() < 0.3 {
            self.cam.anim = (0.0, 0.0);
        }
        self.fps = self.fps * 0.95 + (1.0 / dt.max(1e-4)) * 0.05;
        if let Some((_, t)) = &mut self.status {
            *t -= dt;
            if *t <= 0.0 {
                self.status = None;
            }
        }
        self.handle_keys(input, dt);
        if input.ui.wheel != 0.0 {
            self.cam.zoom = (self.cam.zoom + input.ui.wheel.signum() as i32).clamp(1, 6);
        }
        self.advance_sim(dt);
        self.check_hot_reload(dt);
        if let Selection::Pawn(e) | Selection::Thing(e) = self.selection
            && !self.sim.world.is_alive(e)
        {
            self.selection = Selection::None;
        }
        if let Selection::Zone(z) = self.selection {
            let zones = self.sim.world.res::<Zones>();
            if zones.get(z).is_none() && zones.grow(z).is_none() {
                drop(zones);
                self.selection = Selection::None;
            }
        }

        self.ui_scale = if screen.1 >= 1300 { 3.0 } else { 2.0 };
        let us = self.ui_scale;
        let mut ui_input = input.ui;
        ui_input.mouse = (input.ui.mouse.0 / us, input.ui.mouse.1 / us);
        let mut ui = Ui::new(atlas, (screen.0 as f32 / us, screen.1 as f32 / us), ui_input);
        self.hud(&mut ui);
        let over_ui = ui.wants_mouse || ui.consumed_click;
        if !over_ui || input.released {
            let mut wi = FrameInput { ui: input.ui, released: input.released, ..Default::default() };
            if over_ui {
                wi.ui.pressed = false;
                wi.ui.right_pressed = false;
            }
            self.world_input(&wi, screen);
        }
        if over_ui && !input.released {
            self.hover = None;
        }
        // World tooltip.
        if !over_ui && let Some(h) = self.hover {
            let tip = self.cell_tooltip(h);
            if !tip.is_empty() && self.tool == Tool::Select {
                ui.set_tooltip(tip);
            }
        }
        let ui_quads = ui.finish();

        let cells = self.tool_cells();
        let preview: Vec<(P3, Preview)> = cells
            .iter()
            .map(|p| (*p, if self.cell_valid(*p) { Preview::Valid } else { Preview::Invalid }))
            .collect();
        let defs = self.defs();
        let preview_sprite = match self.tool {
            Tool::Build(def) => Some((defs.buildings[def].sprite.as_str(), self.build_rot)),
            _ => None,
        };
        let selected = match self.selection {
            Selection::Pawn(e) => Some(e),
            _ => None,
        };
        let fp = FrameParams {
            cam: self.cam,
            screen,
            selected,
            hover: if self.tool == Tool::Select { self.hover } else { None },
            preview: &preview,
            preview_sprite,
            time: self.time,
            overlay: self.overlay,
            walls_down: self.walls_down,
        };
        let mut world = self.scene.build(&self.sim, atlas, &fp);
        let running = !self.paused && self.last_ticks > 0;
        self.fx.update(dt, &self.sim, &self.scene.anchors, running);
        {
            let (ox, oy) = self.cam.offset(screen);
            let z = self.cam.zoom as f32;
            let view = (-ox / z, -oy / z, (screen.0 as f32 - ox) / z, (screen.1 as f32 - oy) / z);
            let visual = self.sim.world.res::<WeatherState>().current.map(|w| self.defs().weather[w].visual).unwrap_or_default();
            self.fx.update_weather(dt, visual, view, !self.paused);
        }
        world.extend(self.fx.quads(atlas, &self.sim, &self.scene.anchors, self.time));
        let (ox, oy) = self.cam.offset(screen);
        let clock = self.sim.world.res::<Clock>().clone();
        let weather = self.sim.world.res::<WeatherState>().clone();
        let wlight = weather.light(&self.defs());
        let visual = weather.current.map(|w| self.defs().weather[w].visual).unwrap_or_default();
        let lighting = lighting(&clock, self.cam.angle, self.scene.lights.clone(), wlight, visual);
        FrameOut {
            world,
            world_xf: Xf { scale: self.cam.zoom as f32, offset: [ox, oy] },
            ui: ui_quads,
            ui_xf: Xf { scale: us, offset: [0.0, 0.0] },
            lighting,
        }
    }

    fn overlays(&self) -> Vec<Overlay> {
        let defs = self.defs();
        let mut v = vec![Overlay::None];
        v.extend(defs.env_fields.iter().filter(|(_, f)| f.overlay.is_some()).map(|(id, _)| Overlay::Field(id)));
        v.push(Overlay::Rooms);
        v
    }

    pub fn cycle_overlay(&mut self) {
        let all = self.overlays();
        let i = all.iter().position(|o| *o == self.overlay).unwrap_or(0);
        self.overlay = all[(i + 1) % all.len()];
    }

    fn overlay_name(&self) -> String {
        match self.overlay {
            Overlay::None => "off".into(),
            Overlay::Field(f) => self.defs().env_fields[f].label.clone(),
            Overlay::Rooms => "Rooms".into(),
        }
    }

    fn cell_tooltip(&self, p: P3) -> String {
        let defs = self.defs();
        let w = &self.sim.world;
        let map = w.res::<Map>();
        let Some(c) = map.get(p) else { return String::new() };
        let mut lines = Vec::new();
        if let Some(t) = c.solid {
            lines.push(defs.terrain[t].label.clone());
        } else if let Some(t) = map.get(p.down()).and_then(|c| c.solid) {
            lines.push(format!("{} floor", defs.terrain[t].label));
        }
        if let Some(e) = c.item
            && let Some(it) = w.get::<Item>(e)
        {
            lines.push(format!("{} x{}", defs.items[it.def].label, it.count));
        }
        if let Some(e) = c.plant
            && let Some(pl) = w.get::<Plant>(e)
        {
            let hp = if pl.health < 0.995 { format!(", health {:.0}%", pl.health * 100.0) } else { String::new() };
            lines.push(format!("{} ({:.0}% grown{hp})", defs.plants[pl.def].label, pl.growth * 100.0));
        }
        if let Some(z) = c.zone {
            let zones = w.res::<Zones>();
            if let Some(g) = zones.grow(z) {
                lines.push(format!("{}: {}", g.name, defs.plants[g.crop].label));
            } else if let Some(sp) = zones.get(z) {
                lines.push(sp.name.clone());
            }
        }
        if let Some(e) = c.building {
            if let Some(b) = w.get::<Building>(e) {
                lines.push(defs.buildings[b.def].label.clone());
            } else if let Some(b) = w.get::<Blueprint>(e) {
                lines.push(format!("{} (blueprint)", defs.buildings[b.def].label));
            }
        }
        if c.designation != Designation::None {
            lines.push(format!("Designated: {:?}", c.designation));
        }
        if c.roof {
            lines.push("Roofed".into());
        }
        // Environment at the cell (fields with an overlay range are the interesting ones).
        if map.in_bounds(p) && c.solid.is_none() {
            let env = w.res::<Env>();
            let parts: Vec<String> = defs
                .env_fields
                .iter()
                .filter(|(_, f)| f.overlay.is_some())
                .map(|(id, f)| format!("{} {}", f.label, f.format(env.get(id, map.idx(p)))))
                .collect();
            lines.push(parts.join(", "));
            if let Some(r) = w.res::<Rooms>().indoor_room_at(&map, p) {
                let role = r.role.map(|x| defs.room_roles[x].label.clone()).unwrap_or_else(|| "Room".into());
                lines.push(format!("{role}: {} cells, impressiveness {:.0}", r.cells.len(), r.impressiveness));
            } else {
                lines.push("Outdoors".into());
            }
        }
        if self.show_debug {
            lines.push(format!("{p} walk={} roof={}", map.is_walkable(p), map.is_roofed(p)));
        }
        lines.join("\n")
    }

    // ------------------------------------------------------------------ HUD

    fn hud(&mut self, ui: &mut Ui) {
        let (sw, sh) = ui.size;
        self.top_bar(ui, sw);
        self.colonist_list(ui);
        self.toolbar(ui, sw, sh);
        if self.show_build {
            self.build_menu(ui, sh);
        }
        match self.selection {
            Selection::Pawn(e) => self.pawn_panel(ui, e, sw, sh),
            Selection::Thing(e) => self.thing_panel(ui, e, sw),
            Selection::Zone(z) => self.zone_panel(ui, z, sw),
            Selection::None => {}
        }
        if self.show_work {
            self.work_grid(ui, sw, sh);
        }
        self.message_log(ui, sh);
        if self.show_debug {
            self.debug_panel(ui, sw);
        }
        if let Some((s, t)) = &self.status {
            let a = t.min(1.0);
            let w = Ui::text_w(s) + 10.0;
            let x = ((sw - w) / 2.0).floor();
            ui.rect(x, 22.0, w, 13.0, [0.0, 0.0, 0.0, 0.7 * a]);
            ui.text(x + 5.0, 25.0, s, [1.0, 0.9, 0.6, a]);
        }
    }

    fn top_bar(&mut self, ui: &mut Ui, sw: f32) {
        ui.panel(0.0, 0.0, sw, 17.0);
        let clock = self.sim.clock();
        ui.text(5.0, 5.0, &format!("{}, {}", clock.season_name(), clock.time_string()), theme::TEXT);
        let mut x = 160.0;
        let labels = ["||", ">", ">>", ">>>", "!!!"];
        for (i, l) in labels.iter().enumerate() {
            let active = if i == 0 { self.paused } else { !self.paused && self.speed as usize == i };
            if ui.button(x, 2.0, 22.0, 13.0, l, active) {
                if i == 0 {
                    self.paused = !self.paused;
                } else {
                    self.speed = i as u8;
                    self.paused = false;
                }
            }
            x += 24.0;
        }
        x += 10.0;
        ui.text(x, 5.0, &format!("Level {}", self.cam.view_z), theme::TEXT);
        x += 48.0;
        if ui.button(x, 2.0, 13.0, 13.0, "-", false) {
            self.set_view_z(self.cam.view_z - 1);
        }
        if ui.button(x + 15.0, 2.0, 13.0, 13.0, "+", false) {
            self.set_view_z(self.cam.view_z + 1);
        }
        x += 40.0;
        let dirs = ["SE", "NE", "NW", "SW"];
        if ui.button(x, 2.0, 13.0, 13.0, "<", false) {
            self.rotate(-1);
        }
        ui.text(x + 17.0, 5.0, &format!("View {}", dirs[self.cam.angle as usize]), theme::TEXT);
        if ui.button(x + 64.0, 2.0, 13.0, 13.0, ">", false) {
            self.rotate(1);
        }
        // Weather and outdoor temperature.
        {
            let defs = self.defs();
            let ws = self.sim.world.res::<WeatherState>().clone();
            let amb = sim::env::ambient_values(&defs, &clock, &ws);
            let weather = ws.current.map(|w| defs.weather[w].label.clone()).unwrap_or_else(|| "Clear".into());
            let temp = defs.core.field_temperature.map(|t| defs.env_fields[t].format(amb[t.index()])).unwrap_or_default();
            let s = format!("{weather}  {temp}");
            ui.text(sw - 150.0 - Ui::text_w(&s), 5.0, &s, theme::TEXT);
        }
        let mut rx = sw - 4.0;
        for (label, id) in [("Debug", 2), ("Load", 1), ("Save", 0)] {
            let w = Ui::text_w(label) + 8.0;
            rx -= w + 2.0;
            let active = id == 2 && self.show_debug;
            if ui.button(rx, 2.0, w, 13.0, label, active) {
                match id {
                    0 => self.save(),
                    1 => self.load(),
                    _ => self.show_debug = !self.show_debug,
                }
            }
        }
    }

    fn colonist_list(&mut self, ui: &mut Ui) {
        let defs = self.defs();
        let pawns: Vec<Entity> = self.sim.world.read::<Pawn>().entities().to_vec();
        let h = 6.0 + pawns.len() as f32 * 24.0;
        ui.panel(2.0, 20.0, 150.0, h);
        let mut y = 23.0;
        let mut clicked = None;
        for e in pawns {
            let w = &self.sim.world;
            let Some(p) = w.get_cloned::<Pawn>(e) else { continue };
            let mood = w.get::<Mood>(e).map(|m| m.value).unwrap_or(50.0);
            let job = w.get::<CurrentJob>(e).map(|j| j.0.kind.label()).unwrap_or("Idle");
            let brk = w.get::<MentalState>(e).map(|m| m.kind.label());
            let sel = self.selection == Selection::Pawn(e);
            if sel {
                ui.rect(3.0, y - 1.0, 148.0, 23.0, [0.3, 0.25, 0.12, 0.6]);
            }
            if ui.hover(3.0, y - 1.0, 148.0, 23.0) && ui.input.pressed {
                clicked = Some(e);
            }
            ui.text_clip(6.0, y, &p.name, 100.0, theme::TEXT);
            ui.text(108.0, y, &defs.races[p.race].label, theme::DIM);
            let mc = if mood < 35.0 { theme::BAD } else if mood > 65.0 { theme::GOOD } else { theme::ACCENT };
            ui.bar(6.0, y + 9.0, 40.0, 5.0, mood / 100.0, mc);
            match brk {
                Some(b) => ui.text_clip(50.0, y + 9.0, b, 98.0, theme::BAD),
                None => ui.text_clip(50.0, y + 9.0, job, 98.0, theme::DIM),
            };
            y += 24.0;
        }
        if let Some(e) = clicked {
            ui.consumed_click = true;
            if self.selection == Selection::Pawn(e)
                && let Some(p) = self.sim.world.get::<Position>(e).map(|p| p.0) {
                    self.focus(p);
                }
            self.selection = Selection::Pawn(e);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, sw: f32, sh: f32) {
        let y = sh - 18.0;
        ui.panel(0.0, y - 1.0, sw, 19.0);
        let mut x = 4.0;
        let tools: [(&str, Tool, &str); 12] = [
            ("Select", Tool::Select, "Select things (Esc)"),
            ("Mine", Tool::Designate(DesignateTool::Mine), "Mine rock at the drag level (M)"),
            ("Dig stairs", Tool::Designate(DesignateTool::DigStairs), "Dig a staircase down"),
            ("Chop", Tool::Designate(DesignateTool::Chop), "Chop trees (C)"),
            ("Harvest", Tool::Designate(DesignateTool::Harvest), "Harvest ripe plants (H)"),
            ("Cancel", Tool::Designate(DesignateTool::Cancel), "Cancel designations and blueprints (X)"),
            ("Roof", Tool::Designate(DesignateTool::BuildRoof), "Build roofs over an area"),
            ("No roof", Tool::Designate(DesignateTool::RemoveRoof), "Remove roofs from an area"),
            ("Stockpile", Tool::Stockpile, "Create a stockpile zone (Z)"),
            ("Grow zone", Tool::GrowZone, "Create a growing zone on soil (G)"),
            ("Clear zone", Tool::ClearZone, "Remove zone cells"),
            ("Build", Tool::Select, "Open the build menu (B)"),
        ];
        for (i, (label, tool, tip)) in tools.iter().enumerate() {
            let is_build = i == 11;
            let active = if is_build { self.show_build || matches!(self.tool, Tool::Build(_)) } else { self.tool == *tool };
            let w = Ui::text_w(label) + 10.0;
            if ui.hover(x, y + 1.0, w, 14.0) {
                ui.set_tooltip(*tip);
            }
            if ui.button(x, y + 1.0, w, 14.0, label, active) {
                if is_build {
                    self.show_build = !self.show_build;
                } else {
                    self.tool = *tool;
                    self.show_build = false;
                }
                self.drag_start = None;
            }
            x += w + 2.0;
            if i == 5 || i == 7 || i == 10 {
                x += 8.0;
            }
        }
        x += 8.0;
        let w = Ui::text_w("Work") + 10.0;
        if ui.button(x, y + 1.0, w, 14.0, "Work", self.show_work) {
            self.show_work = !self.show_work;
        }
        x += w + 2.0;
        let label = format!("Overlay: {}", self.overlay_name());
        let w = Ui::text_w(&label) + 10.0;
        if ui.hover(x, y + 1.0, w, 14.0) {
            ui.set_tooltip("Cycle environment overlays (O)");
        }
        if ui.button(x, y + 1.0, w, 14.0, &label, self.overlay != Overlay::None) {
            self.cycle_overlay();
        }
        if let Tool::Build(def) = self.tool {
            let defs = self.defs();
            let s = format!("Placing {} (R: rotate, right-click: stop)", defs.buildings[def].label);
            ui.text(sw - Ui::text_w(&s) - 6.0, y + 4.0, &s, theme::ACCENT);
        }
    }

    fn build_menu(&mut self, ui: &mut Ui, sh: f32) {
        let defs = self.defs();
        let items: Vec<(BuildingId, String, String)> = defs
            .buildings
            .iter()
            .filter(|(_, b)| b.buildable)
            .map(|(id, b)| {
                let cost: Vec<String> = b.cost.iter().map(|c| format!("{} {}", c.count, defs.items[c.item].label)).collect();
                (id, b.label.clone(), cost.join(", "))
            })
            .collect();
        let h = items.len() as f32 * 15.0 + 18.0;
        let x = 300.0;
        let y = sh - 20.0 - h;
        let mut cy = ui.titled_panel(x, y, 250.0, h, "Build");
        for (id, label, cost) in items {
            let active = self.tool == Tool::Build(id);
            if ui.button(x + 3.0, cy, 244.0, 13.0, "", active) {
                self.tool = Tool::Build(id);
                self.show_build = false;
            }
            ui.text(x + 7.0, cy + 3.0, &label, if active { theme::ACCENT } else { theme::TEXT });
            ui.text_clip(x + 98.0, cy + 3.0, &cost, 146.0, theme::DIM);
            cy += 15.0;
        }
    }

    fn pawn_panel(&mut self, ui: &mut Ui, e: Entity, sw: f32, sh: f32) {
        let defs = self.defs();
        let w = &self.sim.world;
        let Some(pawn) = w.get_cloned::<Pawn>(e) else { return };
        let pw = 210.0;
        let x = sw - pw - 2.0;
        let y0 = 20.0;
        let ph = sh - 42.0;
        let mut y = ui.titled_panel(x, y0, pw, ph, &pawn.name);
        let race = &defs.races[pawn.race];
        ui.text(x + 5.0, y, &format!("{}, age {}", race.label, pawn.age), theme::DIM);
        if ui.hover(x + 5.0, y, 100.0, 8.0) {
            ui.set_tooltip(race.description.clone());
        }
        y += 11.0;
        let job = w.get::<CurrentJob>(e).map(|j| j.0.clone());
        let jl = job.as_ref().map(|j| j.kind.label()).unwrap_or("Idle");
        ui.text(x + 5.0, y, &format!("Doing: {jl}"), theme::TEXT);
        if let Some(f) = job.as_ref().and_then(|j| j.work_fraction()) {
            ui.bar(x + 140.0, y, 60.0, 7.0, f, theme::ACCENT);
        }
        y += 11.0;
        if let Some(pp) = w.get::<Position>(e).map(|p| p.0) {
            let map = w.res::<Map>();
            let env = w.res::<Env>();
            let temp = defs.core.field_temperature.map(|t| defs.env_fields[t].format(env.get(t, map.idx(pp)))).unwrap_or_default();
            let place = match w.res::<Rooms>().indoor_room_at(&map, pp) {
                Some(r) => r.role.map(|x| defs.room_roles[x].label.clone()).unwrap_or_else(|| "Indoors".into()),
                None => "Outdoors".into(),
            };
            ui.text_clip(x + 5.0, y, &format!("{place}, {temp}"), pw - 10.0, theme::DIM);
            y += 11.0;
        }
        if let Some(ms) = w.get::<MentalState>(e) {
            ui.text(x + 5.0, y, &format!("BREAK: {}", ms.kind.label()), theme::BAD);
            y += 11.0;
        }
        // Mood.
        let mood = w.get_cloned::<Mood>(e).unwrap_or_default();
        let thr = sim::stats::stat(w, e, defs.core.stat_break_threshold);
        ui.text(x + 5.0, y, "Mood", theme::ACCENT);
        let mc = if mood.value < thr { theme::BAD } else if mood.value > 65.0 { theme::GOOD } else { theme::ACCENT };
        ui.bar(x + 45.0, y, 120.0, 8.0, mood.value / 100.0, mc);
        // Break threshold marker.
        ui.rect(x + 45.0 + 120.0 * thr / 100.0, y - 1.0, 1.0, 10.0, theme::BAD);
        ui.text(x + 170.0, y, &format!("{:.0}", mood.value), theme::TEXT);
        y += 12.0;
        // Needs.
        let needs = w.get_cloned::<Needs>(e).unwrap_or_default();
        for rn in &race.needs {
            let v = needs.values.get(rn.need.index()).copied().unwrap_or(1.0);
            ui.text(x + 5.0, y, &defs.needs[rn.need].label, theme::TEXT);
            let c = if v < 0.25 { theme::BAD } else { [0.45, 0.65, 0.85, 1.0] };
            ui.bar(x + 75.0, y, 90.0, 7.0, v, c);
            ui.text(x + 170.0, y, &format!("{:.0}%", v * 100.0), theme::DIM);
            y += 10.0;
        }
        y += 3.0;
        // Quirks.
        ui.text(x + 5.0, y, "Quirks", theme::ACCENT);
        y += 10.0;
        let quirks = w.get_cloned::<Quirks>(e).unwrap_or_default();
        let mut qx = x + 5.0;
        for q in &quirks.0 {
            let qd = &defs.quirks[*q];
            let tw = Ui::text_w(&qd.label) + 6.0;
            if qx + tw > x + pw - 4.0 {
                qx = x + 5.0;
                y += 12.0;
            }
            ui.rect(qx, y - 2.0, tw, 11.0, [0.22, 0.2, 0.26, 1.0]);
            ui.text(qx + 3.0, y, &qd.label, [0.85, 0.78, 0.95, 1.0]);
            if ui.hover(qx, y - 2.0, tw, 11.0) {
                ui.set_tooltip(qd.description.clone());
            }
            qx += tw + 3.0;
        }
        y += 14.0;
        // Thoughts.
        ui.text(x + 5.0, y, "Thoughts", theme::ACCENT);
        y += 10.0;
        let mut thoughts: Vec<(String, f32)> = mood.situational.iter().map(|t| (defs.thoughts[*t].label.clone(), defs.thoughts[*t].mood)).collect();
        let mut mem: Vec<(String, f32)> = Vec::new();
        for m in &mood.memories {
            let td = &defs.thoughts[m.thought];
            match mem.iter_mut().find(|(l, _)| *l == td.label) {
                Some(x) => x.1 += td.mood,
                None => mem.push((td.label.clone(), td.mood)),
            }
        }
        thoughts.extend(mem);
        for (label, v) in thoughts.iter().take(9) {
            ui.text_clip(x + 7.0, y, label, 160.0, theme::TEXT);
            ui.text(x + 175.0, y, &format!("{v:+.0}"), if *v >= 0.0 { theme::GOOD } else { theme::BAD });
            y += 9.0;
        }
        if thoughts.is_empty() {
            ui.text(x + 7.0, y, "(nothing in particular)", theme::DIM);
            y += 9.0;
        }
        y += 4.0;
        // Skills.
        ui.text(x + 5.0, y, "Skills", theme::ACCENT);
        y += 10.0;
        let skills = w.get_cloned::<Skills>(e).unwrap_or_default();
        for (sid, sd) in defs.skills.iter() {
            if y > y0 + ph - 24.0 {
                break;
            }
            let l = skills.levels.get(sid.index()).copied().unwrap_or_default();
            let apt = sim::stats::aptitude(&defs, pawn.race, &quirks.0, sid);
            ui.text(x + 7.0, y, &sd.label, theme::TEXT);
            ui.bar(x + 85.0, y + 1.0, 80.0, 6.0, l.level as f32 / 20.0, [0.7, 0.6, 0.35, 1.0]);
            ui.rect(x + 86.0, y + 6.0, 78.0 * (l.xp / sim::components::xp_to_next(l.level)).min(1.0), 1.0, [1.0, 1.0, 1.0, 0.5]);
            ui.text(x + 170.0, y, &format!("{}", l.level), theme::TEXT);
            if apt > 1.01 {
                ui.text(x + 186.0, y, "+", theme::GOOD);
            } else if apt < 0.99 {
                ui.text(x + 186.0, y, "-", theme::BAD);
            }
            y += 9.0;
        }
        // Debug: stats and job internals.
        if self.show_debug {
            let sc = w.get_cloned::<StatCache>(e).unwrap_or_default();
            let c = &defs.core;
            ui.text_clip(
                x + 5.0,
                y0 + ph - 22.0,
                &format!("mv {:.2} wk {:.2} mn {:.2}", sc.get(&defs, c.stat_move_speed), sc.get(&defs, c.stat_work_speed), sc.get(&defs, c.stat_mining_speed)),
                pw - 8.0,
                theme::DIM,
            );
            if let Some(d) = w.get::<PawnDebug>(e) {
                ui.text_clip(x + 5.0, y0 + ph - 12.0, &format!("fail: {}", d.last_fail.as_deref().unwrap_or("-")), pw - 8.0, theme::DIM);
            }
        } else if ui.button(x + pw - 60.0, y0 + ph - 16.0, 56.0, 13.0, "Center", false)
            && let Some(p) = w.get::<Position>(e).map(|p| p.0)
        {
            self.focus(p);
        }
    }

    fn thing_panel(&mut self, ui: &mut Ui, e: Entity, sw: f32) {
        let defs = self.defs();
        let pw = 210.0;
        let x = sw - pw - 2.0;
        let w = &self.sim.world;
        let title;
        let mut lines: Vec<String> = Vec::new();
        let mut bench = false;
        let mut blueprint = false;
        if let Some(b) = w.get::<Building>(e) {
            let bd = &defs.buildings[b.def];
            title = bd.label.clone();
            if let Some(bed) = w.get::<Bed>(e) {
                lines.push(format!("Owner: {}", bed.owner.map(|o| sim::jobs::pawn_name(w, o)).unwrap_or_else(|| "nobody".into())));
            }
            bench = w.has::<Workbench>(e);
        } else if let Some(b) = w.get::<Blueprint>(e) {
            let bd = &defs.buildings[b.def];
            title = format!("{} (blueprint)", bd.label);
            for (c, d) in bd.cost.iter().zip(&b.delivered) {
                lines.push(format!("{}: {}/{}", defs.items[c.item].label, d, c.count));
            }
            blueprint = true;
        } else if let Some(it) = w.get::<Item>(e) {
            title = defs.items[it.def].label.clone();
            lines.push(format!("Count: {}", it.count));
            if defs.items[it.def].nutrition > 0.0 {
                lines.push(format!("Nutrition: {:.2} each", defs.items[it.def].nutrition));
            }
            lines.push(if sim::items::is_stored(w, e) { "Stored".into() } else { "Not in a stockpile".into() });
        } else if let Some(pl) = w.get::<Plant>(e) {
            let pd = &defs.plants[pl.def];
            title = pd.label.clone();
            lines.extend(self.plant_lines(e));
            if let Some(h) = &pd.harvest {
                lines.push(if pl.growth >= h.min_growth { "Ready to harvest".into() } else { "Not ripe".into() });
            }
        } else {
            return;
        }
        let bills: Vec<Bill> = if bench { w.get::<Workbench>(e).map(|wb| wb.bills.clone()).unwrap_or_default() } else { Vec::new() };
        let stored: Vec<(String, u32)> = if bench {
            w.get::<Workbench>(e).map(|wb| wb.stored.iter().map(|(d, n)| (defs.items[*d].label.clone(), *n)).collect()).unwrap_or_default()
        } else {
            Vec::new()
        };
        let recipes: Vec<_> = if bench {
            let bdef = w.get::<Building>(e).unwrap().def;
            defs.recipes.iter().filter(|(_, r)| r.bench == bdef).map(|(id, r)| (id, r.label.clone())).collect()
        } else {
            Vec::new()
        };
        let h = 30.0 + lines.len() as f32 * 10.0 + if bench { 30.0 + bills.len() as f32 * 15.0 + recipes.len() as f32 * 15.0 + stored.len() as f32 * 10.0 } else { 0.0 } + if blueprint { 16.0 } else { 0.0 };
        let mut y = ui.titled_panel(x, 20.0, pw, h, &title);
        for l in &lines {
            ui.text(x + 5.0, y, l, theme::TEXT);
            y += 10.0;
        }
        if blueprint {
            if ui.button(x + 5.0, y, 60.0, 13.0, "Cancel", false)
                && let Some(p) = w.get::<Position>(e).map(|p| p.0)
            {
                orders::designate(&mut self.sim.world, &[p], DesignateTool::Cancel);
            }
            return;
        }
        if !bench {
            return;
        }
        y += 4.0;
        ui.text(x + 5.0, y, "Bills", theme::ACCENT);
        y += 11.0;
        let mut action: Option<(u32, Option<BillMode>)> = None;
        for b in &bills {
            let r = &defs.recipes[b.recipe];
            ui.text_clip(x + 5.0, y + 3.0, &r.label, 100.0, theme::TEXT);
            let mode = match b.mode {
                BillMode::Count(n) => format!("{}/{}", b.done, n),
                BillMode::Forever => "Forever".into(),
                BillMode::UntilHave(n) => format!("Until {n}"),
            };
            if ui.button(x + 108.0, y, 70.0, 13.0, &mode, false) {
                let next = match b.mode {
                    BillMode::Count(n) if n < 10 => BillMode::Count(n + 5),
                    BillMode::Count(_) => BillMode::Forever,
                    BillMode::Forever => BillMode::UntilHave(10),
                    BillMode::UntilHave(n) if n < 30 => BillMode::UntilHave(n + 10),
                    BillMode::UntilHave(_) => BillMode::Count(5),
                };
                action = Some((b.id, Some(next)));
            }
            if ui.hover(x + 108.0, y, 70.0, 13.0) {
                ui.set_tooltip("Click to cycle: count / forever / until you have N");
            }
            if ui.button(x + 182.0, y, 22.0, 13.0, "x", false) {
                action = Some((b.id, None));
            }
            y += 15.0;
        }
        if !stored.is_empty() {
            for (l, n) in &stored {
                ui.text(x + 7.0, y, &format!("holding {n} {l}"), theme::DIM);
                y += 10.0;
            }
        }
        ui.text(x + 5.0, y + 2.0, "Add bill:", theme::DIM);
        y += 13.0;
        let mut add = None;
        for (rid, label) in &recipes {
            if ui.button(x + 5.0, y, pw - 10.0, 13.0, label, false) {
                add = Some(*rid);
            }
            y += 15.0;
        }
        let world = &mut self.sim.world;
        if let Some((id, mode)) = action {
            match mode {
                Some(m) => orders::set_bill(world, e, id, m, false),
                None => orders::remove_bill(world, e, id),
            }
        }
        if let Some(r) = add {
            orders::add_bill(world, e, r, BillMode::Count(5));
        }
    }

    /// Growth, health and what limits growth, for the plant inspector.
    fn plant_lines(&self, e: Entity) -> Vec<String> {
        let defs = self.defs();
        let w = &self.sim.world;
        let (Some(pl), Some(p)) = (w.get::<Plant>(e), w.get::<Position>(e).map(|p| p.0)) else { return Vec::new() };
        let pd = &defs.plants[pl.def];
        let map = w.res::<Map>();
        let env = w.res::<Env>();
        let info = sim::farm::growth_info(&defs, &map, &env, pd, p);
        let mut lines = vec![format!("Growth: {:.0}%", pl.growth * 100.0), format!("Health: {:.0}%", pl.health * 100.0)];
        if pl.growth < 1.0 {
            let mut s = format!("Growth rate: {:.0}%", info.rate * 100.0);
            if let Some((lim, _)) = &info.limit {
                let what = match lim {
                    sim::farm::Limit::Field(f) => defs.env_fields[*f].label.to_lowercase(),
                    sim::farm::Limit::Fertility => "soil".into(),
                };
                s.push_str(&format!(" ({what})"));
            }
            lines.push(s);
        }
        for h in sim::farm::active_harm(&map, &env, pd, p) {
            let l = if h.label.is_empty() { defs.env_fields[h.field].label.to_lowercase() } else { h.label.clone() };
            lines.push(format!("Suffering from {l}!"));
        }
        lines
    }

    fn zone_panel(&mut self, ui: &mut Ui, z: u16, sw: f32) {
        let defs = self.defs();
        let pw = 210.0;
        let x = sw - pw - 2.0;
        let (grow, stock) = {
            let zones = self.sim.world.res::<Zones>();
            (zones.grow(z).cloned(), zones.get(z).cloned())
        };
        if let Some(sp) = stock {
            use sim::defs::ItemCategory as C;
            let cats = [
                (C::Resource, "Resources", sp.allow_resources),
                (C::RawFood, "Raw food", sp.allow_raw_food),
                (C::Meal, "Meals", sp.allow_meals),
                (C::Manufactured, "Manufactured", sp.allow_manufactured),
                (C::Seed, "Seeds", sp.allow_seeds),
            ];
            let h = 30.0 + 11.0 + cats.len() as f32 * 15.0 + 18.0;
            let mut y = ui.titled_panel(x, 20.0, pw, h, &sp.name);
            ui.text(x + 5.0, y, &format!("{} cells. Accepts:", sp.cells.len()), theme::DIM);
            y += 11.0;
            let mut toggle = None;
            for (cat, label, on) in cats {
                if ui.button(x + 5.0, y, pw - 10.0, 13.0, &format!("[{}] {label}", if on { "x" } else { " " }), on) {
                    toggle = Some((cat, !on));
                }
                y += 15.0;
            }
            if ui.button(x + 5.0, y + 2.0, 90.0, 13.0, "Remove zone", false) {
                orders::remove_zone(&mut self.sim.world, z);
            }
            if let Some((cat, on)) = toggle {
                orders::set_stockpile_filter(&mut self.sim.world, z, cat, on);
            }
            return;
        }
        let Some(g) = grow else { return };
        let w = &self.sim.world;
        let (mut planted, mut ripe) = (0, 0);
        {
            let map = w.res::<Map>();
            let plants = w.read::<Plant>();
            for p in &g.cells {
                if let Some(pl) = map.cell(*p).plant.and_then(|e| plants.get(e))
                    && pl.def == g.crop
                {
                    planted += 1;
                    if sim::farm::is_ripe(&defs, pl) {
                        ripe += 1;
                    }
                }
            }
        }
        let crops: Vec<(sim::defs::PlantId, String, u32, String)> = defs
            .plants
            .iter()
            .filter_map(|(id, pd)| {
                let c = pd.crop.as_ref()?;
                let seeds = sim::items::colony_count(w, c.seed);
                let days = pd.grow_days;
                Some((id, pd.label.clone(), seeds, format!("{days:.1} growing days, needs soil {:.1}", c.min_fertility)))
            })
            .collect();
        let h = 30.0 + 33.0 + crops.len() as f32 * 15.0 + 12.0;
        let mut y = ui.titled_panel(x, 20.0, pw, h, &g.name);
        ui.text(x + 5.0, y, &format!("{} cells, {planted} planted, {ripe} ripe", g.cells.len()), theme::TEXT);
        y += 11.0;
        let mut seeds_now = 0;
        if let Some((_, _, n, _)) = crops.iter().find(|(id, ..)| *id == g.crop) {
            seeds_now = *n;
        }
        ui.text(x + 5.0, y, &format!("Seeds in colony: {seeds_now}"), if seeds_now == 0 { theme::BAD } else { theme::DIM });
        y += 11.0;
        ui.text(x + 5.0, y, "Crop:", theme::ACCENT);
        y += 11.0;
        let mut pick = None;
        for (id, label, seeds, tip) in &crops {
            let active = *id == g.crop;
            if ui.hover(x + 5.0, y, pw - 10.0, 13.0) {
                ui.set_tooltip(tip.clone());
            }
            if ui.button(x + 5.0, y, pw - 10.0, 13.0, "", active) {
                pick = Some(*id);
            }
            ui.text(x + 9.0, y + 3.0, label, if active { theme::ACCENT } else { theme::TEXT });
            let s = format!("{seeds} seeds");
            ui.text(x + pw - 9.0 - Ui::text_w(&s), y + 3.0, &s, if *seeds == 0 { theme::BAD } else { theme::DIM });
            y += 15.0;
        }
        y += 3.0;
        let sow_label = if g.sow { "Sowing: on" } else { "Sowing: off" };
        let toggle_sow = ui.button(x + 5.0, y, 90.0, 13.0, sow_label, g.sow);
        let remove = ui.button(x + 100.0, y, 90.0, 13.0, "Remove zone", false);
        let world = &mut self.sim.world;
        if let Some(c) = pick {
            orders::set_grow_zone(world, z, c, g.sow);
        }
        if toggle_sow {
            orders::set_grow_zone(world, z, g.crop, !g.sow);
        }
        if remove {
            orders::remove_zone(world, z);
        }
    }

    fn work_grid(&mut self, ui: &mut Ui, sw: f32, sh: f32) {
        let defs = self.defs();
        let pawns: Vec<Entity> = self.sim.world.read::<Pawn>().entities().to_vec();
        let col_w = 42.0;
        let name_w = 110.0;
        let wts: Vec<_> = defs.work_types.iter().map(|(id, w)| (id, w.label.clone(), w.skill)).collect();
        let pw = name_w + col_w * wts.len() as f32 + 10.0;
        let ph = 40.0 + pawns.len() as f32 * 16.0 + 14.0;
        let x = ((sw - pw) / 2.0).floor();
        let y0 = ((sh - ph) / 2.0).floor();
        let mut y = ui.titled_panel(x, y0, pw, ph, "Work priorities (1 = highest, click to cycle)");
        for (i, (_, label, _)) in wts.iter().enumerate() {
            let cx = x + name_w + i as f32 * col_w;
            ui.text_clip(cx + 2.0, y, label, col_w - 2.0, theme::ACCENT);
        }
        y += 12.0;
        let mut change = None;
        for e in pawns {
            let w = &self.sim.world;
            let name = w.get::<Pawn>(e).map(|p| p.name.clone()).unwrap_or_default();
            let prios = w.get_cloned::<WorkPriorities>(e).unwrap_or_default();
            let skills = w.get_cloned::<Skills>(e).unwrap_or_default();
            ui.text_clip(x + 5.0, y + 3.0, &name, name_w - 8.0, theme::TEXT);
            for (i, (wt, _, skill)) in wts.iter().enumerate() {
                let cx = x + name_w + i as f32 * col_w;
                let p = prios.0.get(wt.index()).copied().unwrap_or(0);
                let label = if p == 0 { "-".to_string() } else { p.to_string() };
                let hov = ui.hover(cx, y, col_w - 4.0, 13.0);
                let bg = if hov { theme::BTN_HOVER } else { theme::BTN };
                ui.rect(cx, y, col_w - 4.0, 13.0, bg);
                let col = match p {
                    1 => [0.95, 0.55, 0.35, 1.0],
                    2 => theme::ACCENT,
                    3 => theme::TEXT,
                    4 => theme::DIM,
                    _ => [0.4, 0.4, 0.4, 1.0],
                };
                ui.text(cx + 4.0, y + 3.0, &label, col);
                if let Some(s) = skill {
                    let lvl = skills.levels.get(s.index()).map(|l| l.level).unwrap_or(0);
                    ui.text(cx + 18.0, y + 3.0, &format!("{lvl}"), [0.6, 0.6, 0.6, 0.8]);
                }
                if hov && ui.input.pressed {
                    change = Some((e, *wt, if p == 0 { 1 } else if p >= 4 { 0 } else { p + 1 }));
                    ui.consumed_click = true;
                }
                if hov && ui.input.right_pressed {
                    change = Some((e, *wt, if p == 0 { 4 } else { p - 1 }));
                    ui.consumed_click = true;
                }
            }
            y += 16.0;
        }
        ui.text(x + 5.0, y + 2.0, "Small numbers: skill level. Right-click lowers.", theme::DIM);
        if let Some((e, wt, p)) = change {
            orders::set_priority(&mut self.sim.world, e, wt, p);
        }
    }

    fn message_log(&mut self, ui: &mut Ui, sh: f32) {
        let tick = self.sim.clock().tick;
        let entries: Vec<(u64, LogKind, String)> = {
            let log = self.sim.world.res::<GameLog>();
            log.entries.iter().rev().take(6).map(|e| (e.tick, e.kind, e.text.clone())).collect()
        };
        let mut y = sh - 32.0;
        for (t, kind, text) in entries {
            let age = (tick.saturating_sub(t)) as f32 / TICKS_PER_SECOND as f32;
            let a = (1.0 - (age - 20.0) / 10.0).clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            let c = match kind {
                LogKind::Info => theme::TEXT,
                LogKind::Good => theme::GOOD,
                LogKind::Bad => theme::BAD,
            };
            ui.text(6.0, y, &text, [c[0], c[1], c[2], a]);
            y -= 10.0;
        }
    }

    fn debug_panel(&mut self, ui: &mut Ui, sw: f32) {
        let w = &self.sim.world;
        let pf = w.res::<Pathfinder>();
        let zones = w.res::<Zones>().stockpiles.len();
        let lines = [
            format!("fps {:.0}  sprites {}", self.fps, self.scene.sprites_drawn),
            format!("tick {:.3} ms x{}  speed {}", self.last_tick_ms, self.last_ticks, self.speed),
            format!("entities {}  zones {}", w.entity_count(), zones),
            format!("path searches {}  nodes {}", pf.searches, pf.nodes_expanded),
            format!("cam {:.0},{:.0} z{} zoom{} angle{}", self.cam.center.0, self.cam.center.1, self.cam.view_z, self.cam.zoom, self.cam.angle),
            format!("hover {}", self.hover.map(|h| h.to_string()).unwrap_or_else(|| "-".into())),
        ];
        let x = sw - 440.0;
        ui.panel(x, 20.0, 222.0, 8.0 + lines.len() as f32 * 10.0);
        for (i, l) in lines.iter().enumerate() {
            ui.text_clip(x + 4.0, 24.0 + i as f32 * 10.0, l, 216.0, theme::GOOD);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iso::Proj;
    use sim::worldgen::WorldGenParams;

    const SCREEN: (u32, u32) = (1600, 900);

    fn setup() -> (Game, Atlas) {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let defs = Defs::load_dir(&assets.join("defs"), None).unwrap();
        let sim = Sim::new(defs, 5, &WorldGenParams { width: 64, height: 64, levels: 5 });
        let mut g = Game::new(sim, assets.join("defs"));
        g.paused = true;
        (g, Atlas::load(&assets.join("sprites")))
    }

    /// Screen position of a cell's ground centre.
    fn screen_of(g: &Game, p: P3) -> (f32, f32) {
        let proj = g.proj();
        let (wx, wy) = proj.cell_center(p.x, p.y, p.z);
        let (ox, oy) = g.cam.offset(SCREEN);
        (wx * g.cam.zoom as f32 + ox, wy * g.cam.zoom as f32 + oy)
    }

    fn frame(g: &mut Game, a: &Atlas, mouse: (f32, f32), pressed: bool, released: bool) {
        let mut inp = FrameInput::default();
        inp.ui.mouse = mouse;
        inp.ui.pressed = pressed;
        inp.ui.down = pressed;
        inp.released = released;
        g.frame(a, 1.0 / 60.0, &inp, SCREEN);
    }

    fn start(g: &Game) -> P3 {
        g.sim.world.res::<SimMeta>().start
    }

    #[test]
    fn picking_hits_the_cell_under_the_cursor() {
        let (mut g, a) = setup();
        let s = start(&g).offset(2, 1, 0);
        let sp = screen_of(&g, s);
        frame(&mut g, &a, sp, false, false);
        assert_eq!(g.hover, Some(s));
    }

    #[test]
    fn drag_creates_stockpile() {
        let (mut g, a) = setup();
        let s = start(&g);
        g.tool = Tool::Stockpile;
        let (p0, p1) = (s.offset(-1, -1, 0), s.offset(1, 1, 0));
        let sp = screen_of(&g, p0);
        frame(&mut g, &a, sp, true, false);
        let sp = screen_of(&g, p1);
        frame(&mut g, &a, sp, false, false);
        let sp = screen_of(&g, p1);
        frame(&mut g, &a, sp, false, true);
        let zones = g.sim.world.res::<Zones>();
        assert_eq!(zones.stockpiles.len(), 1);
        assert!(zones.stockpiles.values().next().unwrap().cells.len() >= 5);
    }

    #[test]
    fn grow_zone_drag_selects_zone_and_plants_are_inspectable() {
        let (mut g, a) = setup();
        let s = start(&g);
        // Find a 3x3 patch of zoneable cells near the start.
        let o = (-10..=10)
            .flat_map(|dy| (-10..=10).map(move |dx| s.offset(dx, dy, 0)))
            .find(|o| (0..3).all(|y| (0..3).all(|x| orders::grow_zone_cell_valid(&g.sim.world, o.offset(x, y, 0)))))
            .expect("flat ground");
        g.focus(o);
        g.tool = Tool::GrowZone;
        let (p0, p1) = (o, o.offset(2, 2, 0));
        let sp = screen_of(&g, p0);
        frame(&mut g, &a, sp, true, false);
        let sp = screen_of(&g, p1);
        frame(&mut g, &a, sp, false, false);
        let sp = screen_of(&g, p1);
        frame(&mut g, &a, sp, false, true);
        let id = {
            let zones = g.sim.world.res::<Zones>();
            assert_eq!(zones.growing.len(), 1);
            *zones.growing.keys().next().unwrap()
        };
        assert_eq!(g.selection, Selection::Zone(id));
        // The zone panel draws; clicking an empty zone cell selects the zone.
        frame(&mut g, &a, (0.0, 0.0), false, false);
        g.tool = Tool::Select;
        g.selection = Selection::None;
        // Clear the cell so the click hits the zone, not a wild plant.
        let cell = o.offset(1, 1, 0);
        let old = g.sim.world.res::<Map>().cell(cell).plant;
        if let Some(e) = old {
            sim::jobs::remove_plant(&mut g.sim.world, e);
        }
        let sp = screen_of(&g, cell);
        frame(&mut g, &a, sp, true, false);
        assert_eq!(g.selection, Selection::Zone(id));
        // A crop on the cell is selected instead and its panel draws.
        let crop = g.defs().plants.expect("ashroot");
        let e = g.sim.world.spawn((Plant::new(crop, 0.4), Position(cell)));
        g.sim.world.res_mut::<Map>().cell_mut(cell).plant = Some(e);
        let sp = screen_of(&g, cell);
        frame(&mut g, &a, sp, true, false);
        assert_eq!(g.selection, Selection::Thing(e));
        assert!(g.plant_lines(e).iter().any(|l| l.starts_with("Health")));
        frame(&mut g, &a, (0.0, 0.0), false, false);
    }

    #[test]
    fn click_places_blueprint_in_every_camera_angle() {
        let (mut g, a) = setup();
        let bed = g.defs().buildings.expect("bed");
        for angle in 0..4 {
            g.rotate(if angle == 0 { 0 } else { 1 });
            g.tool = Tool::Build(bed);
            let p = start(&g).offset(angle - 2, 3, 0);
            g.focus(p);
            let sp = screen_of(&g, p);
        frame(&mut g, &a, sp, true, false);
            let sp = screen_of(&g, p);
        frame(&mut g, &a, sp, false, true);
            let has = g.sim.world.res::<Map>().cell(p).building.is_some();
            assert!(has, "no blueprint at {p} with angle {}", g.cam.angle);
        }
    }

    #[test]
    fn clicking_a_pawn_selects_it() {
        let (mut g, a) = setup();
        frame(&mut g, &a, (0.0, 0.0), false, false);
        let (e, r) = g.scene.pawn_rects[0];
        let c = (r[0] + r[2] / 2.0, r[1] + r[3] * 0.6);
        frame(&mut g, &a, c, true, false);
        assert_eq!(g.selection, Selection::Pawn(e));
    }

    #[test]
    fn rotation_keeps_centre_point() {
        let (mut g, _a) = setup();
        let z = g.cam.view_z as f32;
        let before = g.proj().unproject(g.cam.center.0, g.cam.center.1, z);
        g.rotate(1);
        g.rotate(1);
        let after = g.proj().unproject(g.cam.center.0, g.cam.center.1, z);
        assert!((before.0 - after.0).abs() < 1e-3 && (before.1 - after.1).abs() < 1e-3);
        let p = Proj { angle: 3, w: 64.0, h: 64.0 };
        let (x, y) = p.unrot(p.rot(10.25, 40.5).0, p.rot(10.25, 40.5).1);
        assert!((x - 10.25).abs() < 1e-4 && (y - 40.5).abs() < 1e-4);
    }
}
