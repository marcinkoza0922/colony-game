//! Client-side visual effects (not simulated, not saved): work particles,
//! floating text for colonist events, and thought bubbles.

use sim::Sim;
use sim::components::{CurrentJob, MentalState, Mood, Needs, Pawn};
use sim::defs::{NeedRole, WeatherVisual};
use sim::jobs::JobKind;
use sim::resources::{GameLog, LogKind};

use crate::atlas::{Atlas, Icon, Rect};
use crate::font;
use crate::gfx::Quad;
use crate::scene::PawnAnchor;

struct Particle {
    pos: (f32, f32),
    vel: (f32, f32),
    life: f32,
    max: f32,
    color: [f32; 4],
}

struct FloatText {
    text: String,
    pos: (f32, f32),
    age: f32,
    color: [f32; 4],
}

struct Drop {
    pos: (f32, f32),
    vel: (f32, f32),
    len: f32,
    life: f32,
    color: [f32; 4],
}

pub struct Fx {
    particles: Vec<Particle>,
    texts: Vec<FloatText>,
    drops: Vec<Drop>,
    last_log_tick: Option<u64>,
    seed: u32,
    spawn_acc: f32,
    weather_acc: f32,
    flash: f32,
    visual: WeatherVisual,
    /// Visible world rect (x0, y0, x1, y1).
    view: (f32, f32, f32, f32),
}

fn job_colors(kind: JobKind) -> &'static [[f32; 4]] {
    match kind {
        JobKind::Mine | JobKind::DigStairs => &[[0.55, 0.52, 0.48, 1.0], [0.4, 0.38, 0.35, 1.0], [0.7, 0.66, 0.6, 1.0]],
        JobKind::Chop => &[[0.55, 0.4, 0.25, 1.0], [0.75, 0.58, 0.38, 1.0], [0.3, 0.45, 0.3, 1.0]],
        JobKind::Build => &[[0.7, 0.55, 0.35, 1.0], [0.55, 0.5, 0.45, 1.0]],
        JobKind::Craft => &[[1.0, 0.85, 0.35, 1.0], [1.0, 0.6, 0.2, 1.0], [0.9, 0.9, 0.8, 1.0]],
        JobKind::Harvest => &[[0.4, 0.6, 0.3, 1.0], [0.55, 0.3, 0.6, 1.0]],
        JobKind::Cut => &[[0.4, 0.6, 0.3, 1.0], [0.6, 0.55, 0.3, 1.0]],
        JobKind::Sow => &[[0.45, 0.35, 0.22, 1.0], [0.6, 0.48, 0.3, 1.0]],
        _ => &[[0.8, 0.8, 0.8, 1.0]],
    }
}

impl Fx {
    pub fn new() -> Self {
        Fx {
            particles: Vec::new(),
            texts: Vec::new(),
            drops: Vec::new(),
            last_log_tick: None,
            seed: 0x1234_5678,
            spawn_acc: 0.0,
            weather_acc: 0.0,
            flash: 0.0,
            visual: WeatherVisual::Clear,
            view: (0.0, 0.0, 0.0, 0.0),
        }
    }

    fn rand(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Weather precipitation and flashes over the visible world rect.
    pub fn update_weather(&mut self, dt: f32, visual: WeatherVisual, view: (f32, f32, f32, f32), running: bool) {
        self.visual = visual;
        self.view = view;
        let (x0, y0, x1, y1) = view;
        let area = ((x1 - x0) * (y1 - y0)).max(1.0);
        let (rate, kind) = match visual {
            WeatherVisual::Rain => (area / 900.0, 0),
            WeatherVisual::AetherStorm => (area / 6000.0, 1),
            _ => (0.0, 0),
        };
        self.weather_acc += rate * dt;
        while self.weather_acc >= 1.0 {
            self.weather_acc -= 1.0;
            let x = x0 + self.rand() * (x1 - x0 + 80.0);
            let y = y0 - 20.0 + self.rand() * (y1 - y0) * 0.6;
            let life = 0.35 + self.rand() * 0.5;
            let d = if kind == 0 {
                Drop { pos: (x, y), vel: (-60.0, 420.0), len: 6.0, life, color: [0.75, 0.82, 0.95, 0.45] }
            } else {
                let c = if self.rand() < 0.5 { [0.75, 0.4, 1.0, 0.8] } else { [0.4, 0.95, 0.9, 0.7] };
                Drop { pos: (x, y), vel: ((self.rand() - 0.5) * 40.0, 60.0 + self.rand() * 60.0), len: 2.0, life: life * 3.0, color: c }
            };
            self.drops.push(d);
        }
        if running && visual == WeatherVisual::AetherStorm && self.rand() < dt * 0.25 {
            self.flash = 1.0;
        }
        self.flash = (self.flash - dt * 3.0).max(0.0);
        for d in &mut self.drops {
            d.pos.0 += d.vel.0 * dt;
            d.pos.1 += d.vel.1 * dt;
            d.life -= dt;
        }
        self.drops.retain(|d| d.life > 0.0);
        if self.drops.len() > 3000 {
            let n = self.drops.len() - 3000;
            self.drops.drain(0..n);
        }
    }

    pub fn update(&mut self, dt: f32, sim: &Sim, anchors: &[PawnAnchor], running: bool) {
        // Particles from working colonists.
        if running {
            self.spawn_acc += dt * 16.0;
            while self.spawn_acc >= 1.0 {
                self.spawn_acc -= 1.0;
                for a in anchors {
                    let (Some(at), Some(kind)) = (a.work_at, a.job) else { continue };
                    let cols = job_colors(kind);
                    let col = cols[(self.rand() * cols.len() as f32) as usize % cols.len()];
                    let vx = (self.rand() - 0.5) * 70.0;
                    let vy = -40.0 - self.rand() * 50.0;
                    let life = 0.4 + self.rand() * 0.4;
                    let jitter = (self.rand() - 0.5) * 8.0;
                    self.particles.push(Particle { pos: (at.0 + jitter, at.1), vel: (vx, vy), life, max: life, color: col });
                }
            }
        }
        for p in &mut self.particles {
            p.vel.1 += 160.0 * dt;
            p.pos.0 += p.vel.0 * dt;
            p.pos.1 += p.vel.1 * dt;
            p.life -= dt;
        }
        self.particles.retain(|p| p.life > 0.0);
        if self.particles.len() > 600 {
            let n = self.particles.len() - 600;
            self.particles.drain(0..n);
        }

        // Floating text for new log entries about colonists.
        let log = sim.world.res::<GameLog>();
        let newest = log.entries.last().map(|e| e.tick).unwrap_or(0);
        let since = *self.last_log_tick.get_or_insert(newest);
        for e in log.entries.iter().filter(|e| e.tick > since) {
            let Some(subject) = e.subject else { continue };
            let Some(a) = anchors.iter().find(|a| a.entity == subject) else { continue };
            let color = match e.kind {
                LogKind::Good => [0.6, 0.95, 0.5, 1.0],
                LogKind::Bad => [1.0, 0.45, 0.4, 1.0],
                LogKind::Info => [0.95, 0.92, 0.85, 1.0],
            };
            // Drop the colonist's name for brevity: "Ada is now level 5..." -> "level 5...".
            let name = sim.world.get::<Pawn>(subject).map(|p| p.name.clone()).unwrap_or_default();
            let text = e.text.strip_prefix(&name).map(|t| t.trim_start().to_string()).unwrap_or_else(|| e.text.clone());
            let text = text.trim_start_matches("is now ").trim_start_matches("has ").trim_end_matches('.').to_string();
            self.texts.push(FloatText { text, pos: (a.feet.0, a.feet.1 - a.height - 6.0), age: 0.0, color });
        }
        self.last_log_tick = Some(newest.max(since));
        drop(log);
        for t in &mut self.texts {
            t.age += dt;
            t.pos.1 -= 8.0 * dt;
        }
        self.texts.retain(|t| t.age < 3.0);
    }

    fn rect_quad(atlas: &Atlas, r: Rect, x: f32, y: f32, color: [f32; 4]) -> Quad {
        Quad::unlit([x.round(), y.round()], [r.w as f32, r.h as f32], atlas.uv(r), color)
    }

    fn text(out: &mut Vec<Quad>, atlas: &Atlas, s: &str, cx: f32, y: f32, color: [f32; 4]) {
        let w = s.chars().count() as f32 * font::ADVANCE;
        let mut x = (cx - w / 2.0).round();
        for c in s.chars() {
            if c != ' ' {
                let g = atlas.glyph(c);
                out.push(Self::rect_quad(atlas, g, x + 1.0, y + 1.0, [0.0, 0.0, 0.0, color[3] * 0.7]));
                out.push(Self::rect_quad(atlas, g, x, y, color));
            }
            x += font::ADVANCE;
        }
    }

    /// World-space quads drawn on top of the scene.
    pub fn quads(&self, atlas: &Atlas, sim: &Sim, anchors: &[PawnAnchor], time: f32) -> Vec<Quad> {
        let mut out = Vec::with_capacity(self.particles.len() + 256);
        for p in &self.particles {
            let a = (p.life / p.max).clamp(0.0, 1.0);
            let s = if a > 0.6 { 3.0 } else if a > 0.25 { 2.0 } else { 1.0 };
            out.push(Quad::unlit([p.pos.0.round(), p.pos.1.round()], [s, s], atlas.uv(atlas.white), [p.color[0], p.color[1], p.color[2], a]));
        }
        // Thought bubbles.
        let defs = sim.defs();
        let w = &sim.world;
        let role_need = |role: NeedRole| defs.needs.iter().find(|(_, n)| n.role == Some(role)).map(|(id, _)| id);
        let (food, rest) = (role_need(NeedRole::Food), role_need(NeedRole::Rest));
        for a in anchors {
            let e = a.entity;
            let needs = w.get::<Needs>(e);
            let need = |id: Option<sim::defs::NeedId>| id.and_then(|i| needs.as_ref().and_then(|n| n.values.get(i.index()).copied())).unwrap_or(1.0);
            let mood = w.get::<Mood>(e).map(|m| m.value).unwrap_or(50.0);
            let sleeping = w.get::<CurrentJob>(e).is_some_and(|j| j.is_sleeping());
            enum Show {
                Glyph(char, [f32; 4]),
                Icon(Icon, [f32; 4]),
            }
            let bob = ((time * 2.5 + e.index() as f32).sin() * 1.5).round();
            // Speech bubble from a recent social interaction takes priority.
            if let Some(i) = w.get::<sim::social::Social>(e).and_then(|s| s.bubble.map(|(i, _)| i)) {
                let id = &defs.interactions[i];
                let txt = if id.icon.is_empty() { "..." } else { id.icon.as_str() };
                let tw = txt.chars().count() as f32 * font::ADVANCE;
                let (bw, bh) = (tw + 5.0, 11.0);
                let bx = (a.feet.0 + 4.0).round();
                let by = (a.feet.1 - a.height - 6.0 + bob).round();
                let white = atlas.white;
                out.push(Quad::unlit([bx - 1.0, by - 1.0], [bw + 2.0, bh + 2.0], atlas.uv(white), [0.1, 0.1, 0.12, 0.9]));
                out.push(Quad::unlit([bx, by], [bw, bh], atlas.uv(white), [0.95, 0.93, 0.88, 1.0]));
                out.push(Quad::unlit([bx + 2.0, by + bh], [3.0, 2.0], atlas.uv(white), [0.95, 0.93, 0.88, 1.0]));
                let col = if id.insult { [0.75, 0.15, 0.1, 1.0] } else if id.romance || id.name == "lovin" { [0.8, 0.25, 0.45, 1.0] } else { [0.2, 0.2, 0.25, 1.0] };
                let mut x = bx + 3.0;
                for c in txt.chars() {
                    if c != ' ' {
                        out.push(Self::rect_quad(atlas, atlas.glyph(c), x, by + 2.0, col));
                    }
                    x += font::ADVANCE;
                }
                continue;
            }
            let hurt = w.get::<sim::health::Health>(e);
            let show = if hurt.as_ref().is_some_and(|h| h.downed || h.bleeding > 0.0) {
                Some(Show::Glyph('+', [0.85, 0.12, 0.1, 1.0]))
            } else if w.has::<MentalState>(e) {
                Some(Show::Glyph('!', [0.85, 0.15, 0.1, 1.0]))
            } else if sleeping {
                Some(Show::Glyph('Z', [0.3, 0.45, 0.85, 1.0]))
            } else if need(food) < 0.2 {
                Some(Show::Icon(Icon::Food, [0.85, 0.35, 0.15, 1.0]))
            } else if need(rest) < 0.15 {
                Some(Show::Glyph('z', [0.4, 0.4, 0.5, 1.0]))
            } else if mood < 30.0 {
                Some(Show::Icon(Icon::Cloud, [0.35, 0.4, 0.55, 1.0]))
            } else {
                None
            };
            let Some(show) = show else { continue };
            let b = atlas.bubble;
            let bx = a.feet.0 + 7.0 - b.ax as f32;
            let by = a.feet.1 - a.height - 2.0 + bob - b.ay as f32;
            out.push(Self::rect_quad(atlas, b, bx, by, [1.0; 4]));
            match show {
                Show::Glyph(c, col) => out.push(Self::rect_quad(atlas, atlas.glyph(c), bx + 4.0, by + 2.0, col)),
                Show::Icon(i, col) => out.push(Self::rect_quad(atlas, atlas.icon(i), bx + 3.0, by + 2.0, col)),
            }
        }
        for t in &self.texts {
            let alpha = (1.0 - (t.age - 2.0).max(0.0)).clamp(0.0, 1.0);
            Self::text(&mut out, atlas, &t.text, t.pos.0, t.pos.1, [t.color[0], t.color[1], t.color[2], alpha]);
        }
        // Weather on top of everything in the world.
        let white = atlas.uv(atlas.white);
        for d in &self.drops {
            out.push(Quad::unlit([d.pos.0.round(), d.pos.1.round()], [1.0, d.len], white, d.color));
        }
        let (x0, y0, x1, y1) = self.view;
        let veil = match self.visual {
            WeatherVisual::Fog => Some([0.72, 0.75, 0.8, 0.3]),
            WeatherVisual::Rain => Some([0.3, 0.35, 0.45, 0.1]),
            WeatherVisual::AetherStorm => Some([0.45, 0.2, 0.65, 0.12]),
            WeatherVisual::Clear => None,
        };
        if let Some(c) = veil {
            out.push(Quad::unlit([x0, y0], [x1 - x0, y1 - y0], white, c));
        }
        if self.flash > 0.0 {
            out.push(Quad::unlit([x0, y0], [x1 - x0, y1 - y0], white, [0.85, 0.75, 1.0, self.flash * 0.35]));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim::worldgen::WorldGenParams;

    #[test]
    fn colonist_log_entries_become_floating_text() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let defs = sim::Defs::load_dir(&assets.join("defs"), None).unwrap();
        let mut s = Sim::new(defs, 1, &WorldGenParams { width: 48, height: 48, levels: 5 });
        let pawn = s.world.read::<Pawn>().entities()[0];
        let anchors = [PawnAnchor { entity: pawn, feet: (10.0, 20.0), height: 30.0, work_at: None, job: None }];
        let mut fx = Fx::new();
        fx.update(0.016, &s, &anchors, false); // establishes the baseline
        s.tick();
        let name = s.world.get::<Pawn>(pawn).unwrap().name.clone();
        sim::jobs::log_about(&s.world, LogKind::Good, pawn, format!("{name} is now level 5 in Mining."));
        fx.update(0.016, &s, &anchors, false);
        assert_eq!(fx.texts.len(), 1);
        assert_eq!(fx.texts[0].text, "level 5 in Mining");
        // Old entries are not replayed.
        fx.update(0.016, &s, &anchors, false);
        assert_eq!(fx.texts.len(), 1);
    }
}
