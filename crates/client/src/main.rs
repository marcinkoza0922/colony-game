//! Colony client: window, event loop and the debug screenshot mode.
//!
//! Usage:
//!   colony [--seed N] [--load FILE]
//! Debug/testing:
//!   colony --shot out.png [--size 1600x900] [--ticks N] [--orders] [--angle A]
//!          [--zoom Z] [--viewz Z] [--select I] [--work] [--build] [--debug] [--focus X,Y]
//!          [--frames N] [--mouse X,Y] [--weather NAME] [--overlay FIELD|rooms] [--zone]

mod atlas;
mod font;
mod fx;
mod game;
mod gfx;
mod iso;
mod scene;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use sim::{Sim, scenario, worldgen::WorldGenParams};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Window, WindowId};

use atlas::Atlas;
use game::{FrameInput, Game, Selection};
use gfx::Gpu;

struct Args(Vec<String>);

impl Args {
    fn get(&self, k: &str) -> Option<String> {
        self.0.iter().position(|a| a == k).and_then(|i| self.0.get(i + 1)).cloned()
    }
    fn has(&self, k: &str) -> bool {
        self.0.iter().any(|a| a == k)
    }
    fn num<T: std::str::FromStr>(&self, k: &str) -> Option<T> {
        self.get(k).and_then(|s| s.parse().ok())
    }
}

fn assets_dir() -> PathBuf {
    let local = PathBuf::from("assets");
    if local.join("defs").is_dir() {
        return local;
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

const CLEAR: [f64; 3] = [0.07, 0.08, 0.1];

fn make_game(args: &Args) -> (Game, Atlas) {
    let assets = assets_dir();
    let defs_dir = assets.join("defs");
    let defs = Sim::load_defs(&defs_dir).unwrap_or_else(|e| {
        eprintln!("Failed to load defs: {e}");
        std::process::exit(1);
    });
    let seed = args.num("--seed").unwrap_or(42u64);
    let mut sim = Sim::new(defs, seed, &WorldGenParams::default());
    if let Some(f) = args.get("--load") {
        let bytes = std::fs::read(&f).expect("read save");
        sim.load_bytes(&bytes).expect("load save");
    }
    let atlas = Atlas::load(&assets.join("sprites"));
    for w in &atlas.warnings {
        eprintln!("atlas: {w}");
    }
    (Game::new(sim, defs_dir), atlas)
}

fn apply_debug_args(game: &mut Game, args: &Args) {
    if args.has("--orders") {
        scenario::basic_orders(&mut game.sim);
    }
    if let Some(n) = args.num::<u64>("--ticks") {
        for i in 0..n {
            game.sim.tick();
            if args.has("--orders") && i % 600 == 0 {
                scenario::ensure_bills(&mut game.sim);
                scenario::periodic_orders(&mut game.sim);
            }
        }
    }
    if let Some(a) = args.num::<i32>("--angle") {
        game.rotate(a);
    }
    if let Some(z) = args.num::<i32>("--zoom") {
        game.cam.zoom = z.clamp(1, 6);
    }
    if let Some(z) = args.num::<i32>("--viewz") {
        game.cam.view_z = z;
    }
    if let Some(i) = args.num::<usize>("--select") {
        let e = game.sim.world.read::<sim::components::Pawn>().entities().get(i).copied();
        if let Some(e) = e {
            game.selection = Selection::Pawn(e);
            if let Some(p) = game.sim.world.get::<sim::components::Position>(e).map(|p| p.0) {
                game.focus(p);
            }
        }
    }
    if args.has("--zone") {
        let z = game.sim.world.res::<sim::resources::Zones>().growing.values().next().map(|g| (g.id, g.cells[0]));
        if let Some((id, p)) = z {
            game.selection = Selection::Zone(id);
            game.focus(p);
        }
    }
    if let Some(f) = args.get("--focus") {
        let v: Vec<i32> = f.split(',').filter_map(|s| s.parse().ok()).collect();
        if v.len() >= 2 {
            let z = game.sim.world.res::<sim::map::Map>().surface_z(v[0], v[1], game.cam.view_z).unwrap_or(2);
            game.focus(sim::P3::new(v[0], v[1], z));
        }
    }
    if let Some(name) = args.get("--weather") {
        let defs = game.sim.defs();
        match defs.weather.find(&name) {
            Some(w) => {
                let mut ws = game.sim.world.res_mut::<sim::env::WeatherState>();
                ws.current = Some(w);
                ws.ticks_left = 1_000_000;
            }
            None => eprintln!("unknown weather {name}"),
        }
    }
    if let Some(name) = args.get("--overlay") {
        let defs = game.sim.defs();
        game.overlay = match (name.as_str(), defs.env_fields.find(&name)) {
            ("rooms", _) => scene::Overlay::Rooms,
            (_, Some(f)) => scene::Overlay::Field(f),
            _ => scene::Overlay::None,
        };
    }
    game.walls_down = args.has("--walls-down");
    game.show_work = args.has("--work");
    game.show_build = args.has("--build");
    game.show_debug = args.has("--debug");
}

fn screenshot(args: &Args, path: &str) {
    let (w, h) = args
        .get("--size")
        .and_then(|s| s.split_once('x').map(|(a, b)| (a.parse().unwrap_or(1600), b.parse().unwrap_or(900))))
        .unwrap_or((1600u32, 900u32));
    let (mut game, atlas) = make_game(args);
    apply_debug_args(&mut game, args);
    let mut gpu = Gpu::new(None, w, h);
    gpu.upload_atlas(atlas.w, atlas.h, &atlas.pixels, &atlas.normals);
    let mut input = FrameInput::default();
    if let Some(m) = args.get("--mouse") {
        let v: Vec<f32> = m.split(',').filter_map(|s| s.parse().ok()).collect();
        if v.len() == 2 {
            input.ui.mouse = (v[0], v[1]);
        }
    }
    // Optionally run live frames first (particles, animation), then two
    // paused frames so picking rects / hover state are populated.
    let live = args.num::<u32>("--frames").unwrap_or(0);
    game.paused = live == 0;
    for _ in 0..live {
        let _ = game.frame(&atlas, 1.0 / 60.0, &input, (w, h));
    }
    game.paused = true;
    let _ = game.frame(&atlas, 1.0 / 60.0, &input, (w, h));
    let f = game.frame(&atlas, 1.0 / 60.0, &input, (w, h));
    let px = gpu.render_offscreen(w, h, &[(&f.world, f.world_xf), (&f.ui, f.ui_xf)], &f.lighting, CLEAR);
    image::save_buffer(path, &px, w, h, image::ExtendedColorType::Rgba8).expect("write screenshot");
    println!("wrote {path} ({w}x{h}, {} world sprites, {} lights)", f.world.len(), f.lighting.lights.len());
}

struct App {
    args: Args,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    game: Option<Game>,
    atlas: Option<Atlas>,
    input: FrameInput,
    mouse: (f32, f32),
    middle_down: bool,
    last: Instant,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Colony")
            .with_inner_size(winit::dpi::LogicalSize::new(1600.0, 900.0));
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        let size = window.inner_size();
        let mut gpu = Gpu::new(Some(window.clone()), size.width, size.height);
        let (mut game, atlas) = make_game(&self.args);
        apply_debug_args(&mut game, &self.args);
        gpu.upload_atlas(atlas.w, atlas.h, &atlas.pixels, &atlas.normals);
        self.window = Some(window);
        self.gpu = Some(gpu);
        self.game = Some(game);
        self.atlas = Some(atlas);
        self.last = Instant::now();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(s) => {
                if let Some(g) = &mut self.gpu {
                    g.resize(s.width, s.height);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let p = (position.x as f32, position.y as f32);
                if self.middle_down {
                    self.input.middle_drag.0 += p.0 - self.mouse.0;
                    self.input.middle_drag.1 += p.1 - self.mouse.1;
                }
                self.mouse = p;
            }
            WindowEvent::MouseInput { state, button, .. } => match (button, state) {
                (MouseButton::Left, ElementState::Pressed) => {
                    self.input.ui.pressed = true;
                    self.input.ui.down = true;
                }
                (MouseButton::Left, ElementState::Released) => {
                    self.input.released = true;
                    self.input.ui.down = false;
                }
                (MouseButton::Right, ElementState::Pressed) => self.input.ui.right_pressed = true,
                (MouseButton::Middle, s) => self.middle_down = s == ElementState::Pressed,
                _ => {}
            },
            WindowEvent::MouseWheel { delta, .. } => {
                self.input.ui.wheel += match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 40.0) as f32,
                };
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => {
                            if !event.repeat {
                                self.input.keys_pressed.push(code);
                            }
                            self.input.keys_held.insert(code);
                        }
                        ElementState::Released => {
                            self.input.keys_held.remove(&code);
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

impl App {
    fn redraw(&mut self) {
        let (Some(gpu), Some(game), Some(atlas)) = (&mut self.gpu, &mut self.game, &self.atlas) else { return };
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        self.input.ui.mouse = self.mouse;
        let screen = (gpu.config.width, gpu.config.height);
        let f = game.frame(atlas, dt, &self.input, screen);
        gpu.render(&[(&f.world, f.world_xf), (&f.ui, f.ui_xf)], &f.lighting, CLEAR);
        // Reset per-frame input.
        self.input.ui.pressed = false;
        self.input.ui.right_pressed = false;
        self.input.released = false;
        self.input.ui.wheel = 0.0;
        self.input.keys_pressed.clear();
        self.input.middle_drag = (0.0, 0.0);
    }
}

fn main() {
    let args = Args(std::env::args().collect());
    if let Some(path) = args.get("--shot") {
        screenshot(&args, &path);
        return;
    }
    let el = EventLoop::new().expect("event loop");
    el.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        args,
        window: None,
        gpu: None,
        game: None,
        atlas: None,
        input: FrameInput::default(),
        mouse: (0.0, 0.0),
        middle_down: false,
        last: Instant::now(),
    };
    el.run_app(&mut app).expect("run app");
}
