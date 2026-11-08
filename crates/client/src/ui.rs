//! Bespoke immediate-mode pixel UI. Coordinates are in UI pixels (screen / scale).

use crate::atlas::{Atlas, Rect};
use crate::font;
use crate::gfx::Quad;

pub type Color = [f32; 4];

pub mod theme {
    use super::Color;
    pub const PANEL: Color = [0.11, 0.12, 0.14, 0.94];
    pub const PANEL_LIGHT: Color = [0.16, 0.17, 0.2, 0.96];
    pub const BORDER: Color = [0.32, 0.34, 0.38, 1.0];
    pub const ACCENT: Color = [0.86, 0.66, 0.32, 1.0];
    pub const TEXT: Color = [0.88, 0.86, 0.8, 1.0];
    pub const DIM: Color = [0.58, 0.58, 0.56, 1.0];
    pub const GOOD: Color = [0.52, 0.8, 0.45, 1.0];
    pub const BAD: Color = [0.92, 0.4, 0.35, 1.0];
    pub const BTN: Color = [0.2, 0.22, 0.26, 1.0];
    pub const BTN_HOVER: Color = [0.28, 0.31, 0.36, 1.0];
    pub const BTN_ACTIVE: Color = [0.42, 0.36, 0.2, 1.0];
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    pub mouse: (f32, f32),
    pub pressed: bool,
    pub right_pressed: bool,
    pub down: bool,
    pub wheel: f32,
}

pub struct Ui<'a> {
    pub atlas: &'a Atlas,
    pub quads: Vec<Quad>,
    pub size: (f32, f32),
    pub input: Input,
    /// Set when the mouse is over a panel or a UI element consumed a click.
    pub wants_mouse: bool,
    pub consumed_click: bool,
    tooltip: Option<String>,
}

impl<'a> Ui<'a> {
    pub fn new(atlas: &'a Atlas, size: (f32, f32), input: Input) -> Self {
        Ui { atlas, quads: Vec::with_capacity(4096), size, input, wants_mouse: false, consumed_click: false, tooltip: None }
    }

    pub fn hover(&self, x: f32, y: f32, w: f32, h: f32) -> bool {
        let (mx, my) = self.input.mouse;
        mx >= x && my >= y && mx < x + w && my < y + h
    }

    fn sprite(&mut self, r: Rect, x: f32, y: f32, w: f32, h: f32, color: Color) {
        self.quads.push(Quad::unlit([x, y], [w, h], self.atlas.uv(r), color));
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        let r = self.atlas.white;
        self.sprite(r, x, y, w, h, color);
    }

    pub fn outline(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        self.rect(x, y, w, 1.0, color);
        self.rect(x, y + h - 1.0, w, 1.0, color);
        self.rect(x, y, 1.0, h, color);
        self.rect(x + w - 1.0, y, 1.0, h, color);
    }

    /// A panel blocks world input under it.
    pub fn panel(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.rect(x, y, w, h, theme::PANEL);
        self.outline(x, y, w, h, theme::BORDER);
        if self.hover(x, y, w, h) {
            self.wants_mouse = true;
            if self.input.pressed || self.input.right_pressed {
                self.consumed_click = true;
            }
        }
    }

    pub fn titled_panel(&mut self, x: f32, y: f32, w: f32, h: f32, title: &str) -> f32 {
        self.panel(x, y, w, h);
        self.rect(x + 1.0, y + 1.0, w - 2.0, 11.0, theme::PANEL_LIGHT);
        self.text(x + 4.0, y + 3.0, title, theme::ACCENT);
        y + 15.0
    }

    pub fn text_w(s: &str) -> f32 {
        s.chars().count() as f32 * font::ADVANCE
    }

    pub fn text(&mut self, x: f32, y: f32, s: &str, color: Color) -> f32 {
        let mut cx = x;
        for c in s.chars() {
            if c != ' ' {
                let g = self.atlas.glyph(c);
                // 1px drop shadow for legibility over the world.
                self.sprite(g, cx + 1.0, y + 1.0, g.w as f32, g.h as f32, [0.0, 0.0, 0.0, color[3] * 0.6]);
                self.sprite(g, cx, y, g.w as f32, g.h as f32, color);
            }
            cx += font::ADVANCE;
        }
        cx - x
    }

    /// Text truncated to fit `max_w`.
    pub fn text_clip(&mut self, x: f32, y: f32, s: &str, max_w: f32, color: Color) {
        let max = (max_w / font::ADVANCE).floor() as usize;
        if s.chars().count() <= max {
            self.text(x, y, s, color);
        } else {
            let t: String = s.chars().take(max.saturating_sub(1)).collect();
            self.text(x, y, &format!("{t}~"), color);
        }
    }

    pub fn button(&mut self, x: f32, y: f32, w: f32, h: f32, label: &str, active: bool) -> bool {
        let hov = self.hover(x, y, w, h);
        let bg = if active {
            theme::BTN_ACTIVE
        } else if hov {
            theme::BTN_HOVER
        } else {
            theme::BTN
        };
        self.rect(x, y, w, h, bg);
        self.outline(x, y, w, h, if active { theme::ACCENT } else { theme::BORDER });
        let tw = Self::text_w(label);
        self.text((x + (w - tw) / 2.0).floor(), (y + (h - 7.0) / 2.0).floor(), label, if active { theme::ACCENT } else { theme::TEXT });
        if hov {
            self.wants_mouse = true;
        }
        if hov && self.input.pressed {
            self.consumed_click = true;
            return true;
        }
        false
    }

    pub fn bar(&mut self, x: f32, y: f32, w: f32, h: f32, frac: f32, color: Color) {
        self.rect(x, y, w, h, [0.05, 0.05, 0.06, 1.0]);
        let f = frac.clamp(0.0, 1.0);
        self.rect(x + 1.0, y + 1.0, ((w - 2.0) * f).round(), h - 2.0, color);
    }

    pub fn set_tooltip(&mut self, s: impl Into<String>) {
        self.tooltip = Some(s.into());
    }

    pub fn finish(mut self) -> Vec<Quad> {
        if let Some(t) = self.tooltip.take() {
            let lines: Vec<&str> = t.lines().collect();
            let w = lines.iter().map(|l| Self::text_w(l)).fold(0.0, f32::max) + 8.0;
            let h = lines.len() as f32 * font::LINE_H + 6.0;
            let (mut x, mut y) = (self.input.mouse.0 + 10.0, self.input.mouse.1 + 10.0);
            x = x.min(self.size.0 - w - 2.0);
            y = y.min(self.size.1 - h - 2.0);
            self.rect(x, y, w, h, [0.06, 0.06, 0.08, 0.96]);
            self.outline(x, y, w, h, theme::ACCENT);
            for (i, l) in lines.iter().enumerate() {
                self.text(x + 4.0, y + 4.0 + i as f32 * font::LINE_H, l, theme::TEXT);
            }
        }
        self.quads
    }
}
