//! Colonist inspector tabs: Health (capacities, conditions, body diagram) and
//! Social (relationships and opinions).

use ecs::{Entity, World};
use sim::defs::{Defs, HediffKind};
use sim::health::{self, Health};
use sim::social::{self, Social};

use crate::ui::{Ui, theme};

/// Pixel scale of the body diagram.
const DIAGRAM_SCALE: f32 = 2.0;

fn health_color(eff: f32, missing: bool) -> [f32; 4] {
    if missing {
        return [0.12, 0.12, 0.13, 1.0];
    }
    let t = eff.clamp(0.0, 1.0);
    // Red (ruined) -> amber -> healthy grey-green.
    if t < 0.5 {
        let k = t / 0.5;
        [0.75, 0.15 + 0.4 * k, 0.12, 1.0]
    } else {
        let k = (t - 0.5) / 0.5;
        [0.75 - 0.35 * k, 0.55 + 0.1 * k, 0.12 + 0.3 * k, 1.0]
    }
}

/// `area` = (x, y, width, bottom) of the tab content.
pub fn health_tab(ui: &mut Ui, w: &World, defs: &Defs, e: Entity, area: (f32, f32, f32, f32)) {
    let (x, mut y, pw, bottom) = area;
    let (Some(h), Some(body)) = (w.get_cloned::<Health>(e), health::body_of(defs, w, e)) else { return };
    let summary = health::summary(defs, &h);
    let status = if h.downed {
        ("Downed", theme::BAD)
    } else if summary < 0.6 {
        ("Badly hurt", theme::BAD)
    } else if summary < 0.95 || !h.hediffs.is_empty() {
        ("Hurt", theme::ACCENT)
    } else {
        ("Healthy", theme::GOOD)
    };
    ui.text(x + 5.0, y, status.0, status.1);
    if h.pain > 0.01 {
        ui.text(x + 80.0, y, &format!("Pain {:.0}%", h.pain * 100.0), if h.pain > 0.4 { theme::BAD } else { theme::TEXT });
    }
    y += 10.0;
    if h.bleeding > 0.0 {
        ui.text(x + 5.0, y, &format!("Bleeding {:.0}%/day", h.bleeding * 100.0), theme::BAD);
        y += 10.0;
    }
    y += 2.0;

    // Body diagram (left) and capacities (right).
    let (dw, dh) = (body.diagram_size.0 as f32 * DIAGRAM_SCALE, body.diagram_size.1 as f32 * DIAGRAM_SCALE);
    let (bx, by) = (x + 6.0, y);
    ui.rect(bx - 2.0, by - 2.0, dw + 4.0, dh + 4.0, [0.06, 0.06, 0.07, 1.0]);
    let mut hovered: Option<usize> = None;
    for (i, p) in body.parts.iter().enumerate() {
        let Some((px, py, pw2, ph2)) = p.diagram else { continue };
        let missing = health::is_missing(defs, body, &h, i);
        let eff = health::part_efficiency(defs, body, &h, i);
        let (rx, ry, rw, rh) = (
            bx + px as f32 * DIAGRAM_SCALE,
            by + py as f32 * DIAGRAM_SCALE,
            pw2 as f32 * DIAGRAM_SCALE,
            ph2 as f32 * DIAGRAM_SCALE,
        );
        let mut c = health_color(eff, missing);
        if p.inside && !missing && eff >= 0.999 {
            c = [c[0] * 0.8, c[1] * 0.8, c[2] * 0.8, 1.0];
        }
        ui.rect(rx, ry, rw, rh, c);
        if ui.hover(rx, ry, rw, rh) {
            hovered = Some(i);
        }
    }
    if let Some(i) = hovered {
        let p = &body.parts[i];
        let hp = health::part_hp(defs, body, &h, i);
        let mut tip = format!("{}: {:.0}/{:.0} HP", p.label, hp, p.hp);
        if health::is_missing(defs, body, &h, i) {
            tip = format!("{}: missing", p.label);
        }
        for x in h.hediffs.iter().filter(|x| x.part() == Some(i)) {
            tip.push('\n');
            tip.push_str(&health::describe(defs, x));
        }
        ui.set_tooltip(tip);
    }
    let cx = bx + dw + 8.0;
    let mut cy = by;
    for (cid, cd) in defs.capacities.iter() {
        let v = h.capacity(cid);
        ui.text_clip(cx, cy, &cd.label, 72.0, theme::TEXT);
        let col = if v < 0.5 { theme::BAD } else if v < 0.95 { theme::ACCENT } else { theme::DIM };
        ui.text(cx + 76.0, cy, &format!("{:.0}%", v * 100.0), col);
        cy += 9.0;
    }
    y = (by + dh + 6.0).max(cy + 4.0);

    // Conditions grouped by part.
    ui.text(x + 5.0, y, "Conditions", theme::ACCENT);
    y += 10.0;
    let groups = health::grouped(defs, body, &h);
    if groups.is_empty() {
        ui.text(x + 7.0, y, "(none)", theme::DIM);
    }
    for (part, list) in groups {
        if y > bottom - 10.0 {
            break;
        }
        ui.text_clip(x + 7.0, y, &part, pw - 14.0, theme::DIM);
        y += 9.0;
        for hd in list {
            if y > bottom - 10.0 {
                break;
            }
            let d = &defs.hediffs[hd.def];
            let bad = hd.tended.is_none() && d.tendable || d.kind == HediffKind::MissingPart;
            ui.text_clip(x + 12.0, y, &health::describe(defs, hd), pw - 18.0, if bad { theme::BAD } else { theme::TEXT });
            y += 9.0;
        }
    }
}

fn opinion_color(v: f32) -> [f32; 4] {
    if v >= 15.0 {
        theme::GOOD
    } else if v <= -15.0 {
        theme::BAD
    } else {
        theme::TEXT
    }
}

pub fn social_tab(ui: &mut Ui, w: &World, e: Entity, area: (f32, f32, f32, f32)) {
    let (x, mut y, pw, bottom) = area;
    let Some(s) = w.get_cloned::<Social>(e) else { return };
    let others: Vec<Entity> = w.read::<sim::components::Pawn>().entities().iter().copied().filter(|o| *o != e).collect();
    ui.text(x + 5.0, y, "Colonist", theme::ACCENT);
    ui.text(x + 100.0, y, "Relation", theme::ACCENT);
    ui.text(x + 165.0, y, "Opin.", theme::ACCENT);
    y += 11.0;
    let mut rows: Vec<(Entity, String, f32)> = others
        .iter()
        .map(|o| {
            let rels: Vec<&str> = s.relation(*o).iter().map(|k| k.label()).collect();
            (*o, rels.join(", "), social::opinion_of(w, e, *o))
        })
        .collect();
    rows.sort_by(|a, b| (!a.1.is_empty()).cmp(&!b.1.is_empty()).reverse().then(b.2.total_cmp(&a.2)));
    for (o, rel, op) in rows {
        if y > bottom - 30.0 {
            break;
        }
        let name = sim::jobs::pawn_name(w, o);
        ui.text_clip(x + 5.0, y, &name, 92.0, theme::TEXT);
        ui.text_clip(x + 100.0, y, if rel.is_empty() { "-" } else { &rel }, 62.0, theme::DIM);
        ui.text(x + 165.0, y, &format!("{:+.0}", op.round() + 0.0), opinion_color(op));
        if ui.hover(x + 3.0, y - 1.0, pw - 6.0, 9.0) {
            let theirs = social::opinion_of(w, o, e);
            let mut tip = format!("Opinion of {name}: {op:+.0}");
            for (l, v) in social::opinion_breakdown(w, e, o) {
                tip.push_str(&format!("\n  {l}: {v:+.0}"));
            }
            tip.push_str(&format!("\n{name}'s opinion of them: {theirs:+.0}"));
            ui.set_tooltip(tip);
        }
        y += 10.0;
    }
    let _ = pw;
}
