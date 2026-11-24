//! Controls for the top bar and the console bar. Every one is `H` tall with
//! the same corners and text size, so a bar lines up: icon buttons (with an
//! optional label and dropdown chevron), the accent button, badges and
//! dividers. Icons are painted, not emoji, so they look the same on every
//! platform.

use crate::theme::Palette;
use eframe::egui::{self, Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, Ui};

/// Height of every toolbar control.
pub const H: f32 = 28.0;
const RADIUS: u8 = 7;
const PAD: f32 = 10.0;
const ICON: f32 = 14.0;

fn font() -> FontId {
    FontId::proportional(13.0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Filled with the accent color (Update).
    Accent,
    /// No fill until hovered (icon buttons on the right).
    Ghost,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Stop,
    Plan,
    Format,
    Gear,
    Moon,
    Sun,
    Check,
    Undo,
    /// a clock face: query history
    Clock,
    /// a database cylinder: the console's database and schema
    Schema,
}

/// White or near-black text, whichever reads better on `fill`.
fn on(fill: Color32) -> Color32 {
    let l = 0.2126 * fill.r() as f32 + 0.7152 * fill.g() as f32 + 0.0722 * fill.b() as f32;
    if l > 150.0 {
        Color32::from_rgb(12, 18, 14)
    } else {
        Color32::WHITE
    }
}

/// A toolbar button: optional icon, optional label, optional dropdown chevron.
/// With only an icon it's a square.
pub fn button(ui: &mut Ui, p: &Palette, kind: Kind, icon: Option<Icon>, label: &str, chevron: bool, enabled: bool) -> Response {
    let text_w = if label.is_empty() { 0.0 } else { ui.painter().layout_no_wrap(label.to_string(), font(), Color32::WHITE).size().x };
    let square = label.is_empty() && !chevron;
    let w = if square {
        H
    } else {
        let mut w = PAD * 2.0 + text_w;
        if icon.is_some() {
            w += ICON + if label.is_empty() { 0.0 } else { 6.0 };
        }
        if chevron {
            w += 14.0;
        }
        w
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, H), if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && resp.hovered();
    let pressed = enabled && resp.is_pointer_button_down_on();
    let open = ui.memory(|m| m.is_popup_open(resp.id.with("menu")));
    let none = Stroke::NONE;
    let thin = |c: Color32| Stroke::new(1.0_f32, c);
    let (fill, stroke, fg) = match (kind, enabled) {
        (Kind::Ghost, false) => (Color32::TRANSPARENT, none, p.faint),
        (_, false) => (p.raised.gamma_multiply(0.7), thin(p.border.gamma_multiply(0.7)), p.faint),
        (Kind::Ghost, true) => (if pressed { p.selection } else if hovered || open { p.hover } else { Color32::TRANSPARENT }, none, if hovered || open { p.text } else { p.muted }),
        (Kind::Accent, true) => {
            let f = if pressed { p.accent.gamma_multiply(0.8) } else if hovered { p.accent.gamma_multiply(0.9) } else { p.accent };
            (f, none, on(p.accent))
        }
    };
    // a disabled Run still shows its play icon in green, faintly; a ghost Run
    // and Stop keep their colors (green to run, red while something can stop)
    let icon_color = match (icon, kind, enabled) {
        (Some(Icon::Play), Kind::Ghost, true) => p.success,
        (Some(Icon::Play), Kind::Ghost, false) => p.success.gamma_multiply(0.45),
        (Some(Icon::Stop), Kind::Ghost, true) => p.danger,
        _ => fg,
    };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(RADIUS), fill, stroke, egui::StrokeKind::Inside);
    let cy = rect.center().y;
    let mut x = rect.left() + PAD;
    if let Some(icon) = icon {
        let r = if square { Rect::from_center_size(rect.center(), egui::vec2(ICON, ICON)) } else { Rect::from_center_size(egui::pos2(x + ICON / 2.0, cy), egui::vec2(ICON, ICON)) };
        // cut-outs (the moon's crescent) use the color behind the button
        let behind = if fill == Color32::TRANSPARENT { p.canvas } else { fill };
        paint_icon(painter, r, icon, icon_color, behind);
        x += ICON + 6.0;
    }
    if !label.is_empty() {
        painter.text(egui::pos2(x, cy), egui::Align2::LEFT_CENTER, label, font(), fg);
    }
    if chevron {
        let c = egui::pos2(rect.right() - PAD - 3.0, cy + 1.0);
        let pts = vec![c + egui::vec2(-3.5, -2.0), c + egui::vec2(3.5, -2.0), c + egui::vec2(0.0, 2.2)];
        painter.add(egui::Shape::convex_polygon(pts, if enabled { p.muted } else { p.faint }, Stroke::NONE));
    }
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Opens a popup below `resp` when it's clicked (a click outside closes it).
/// Inside, call `close(ui)` to close it after picking something.
pub fn dropdown(ui: &mut Ui, resp: &Response, width: f32, add: impl FnOnce(&mut Ui)) {
    let id = resp.id.with("menu");
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(id));
    }
    egui::popup::popup_below_widget(ui, id, resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.set_min_width(width);
        add(ui);
    });
}

pub fn close(ui: &Ui) {
    ui.memory_mut(|m| m.close_popup());
}

/// A status pill (PRODUCTION, read-only, transaction open).
pub fn badge(ui: &mut Ui, color: Color32, text: &str) -> Response {
    let f = FontId::proportional(11.5);
    let tw = ui.painter().layout_no_wrap(text.to_string(), f.clone(), color).size().x;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(tw + 20.0, H), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(RADIUS), color.gamma_multiply(0.14), Stroke::new(1.0_f32, color.gamma_multiply(0.4)), egui::StrokeKind::Inside);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, f, color);
    resp
}

/// A thin vertical line between groups.
pub fn divider(ui: &mut Ui, p: &Palette) {
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 18.0), Sense::hover());
    ui.painter().line_segment([rect.center_top(), rect.center_bottom()], Stroke::new(1.0_f32, p.border));
    ui.add_space(4.0);
}

pub fn paint_icon(painter: &egui::Painter, r: Rect, icon: Icon, c: Color32, bg: Color32) {
    let o = r.center();
    let at = |x: f32, y: f32| o + egui::vec2(x, y);
    let st = Stroke::new(1.5_f32, c);
    let line = |a: egui::Pos2, b: egui::Pos2| painter.line_segment([a, b], st);
    let arc = |cx: f32, cy: f32, rad: f32, from: f32, to: f32| -> Vec<egui::Pos2> {
        (0..=24).map(|i| {
            let a = (from + (to - from) * i as f32 / 24.0).to_radians();
            at(cx + a.cos() * rad, cy + a.sin() * rad)
        }).collect()
    };
    match icon {
        Icon::Play => {
            painter.add(egui::Shape::convex_polygon(vec![at(-3.5, -5.0), at(5.0, 0.0), at(-3.5, 5.0)], c, Stroke::NONE));
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(o, egui::vec2(9.0, 9.0)), CornerRadius::same(2), c);
        }
        Icon::Plan => {
            // an outline: three rows, each indented under the one above
            for (x, y) in [(-5.0_f32, -4.0_f32), (-2.0, 0.0), (1.0, 4.0)] {
                painter.circle_filled(at(x, y), 1.5, c);
                line(at(x + 3.0, y), at(5.5, y));
            }
        }
        Icon::Format => {
            for (y, w) in [(-4.5_f32, 11.0_f32), (-1.5, 7.0), (1.5, 11.0), (4.5, 7.0)] {
                line(at(-5.5, y), at(-5.5 + w, y));
            }
        }
        Icon::Gear => {
            // outline with eight teeth, and a hole in the middle
            let teeth = 8;
            let mut pts = Vec::new();
            for i in 0..teeth {
                let base = std::f32::consts::TAU * i as f32 / teeth as f32;
                for (da, rad) in [(-0.38_f32, 4.6_f32), (-0.2, 6.3), (0.2, 6.3), (0.38, 4.6)] {
                    let a = base + da;
                    pts.push(at(a.cos() * rad, a.sin() * rad));
                }
            }
            painter.add(egui::Shape::closed_line(pts, Stroke::new(1.3_f32, c)));
            painter.circle_stroke(o, 1.9, Stroke::new(1.3_f32, c));
        }
        Icon::Moon => {
            painter.circle_filled(o, 5.5, c);
            painter.circle_filled(at(2.8, -2.4), 4.6, bg);
        }
        Icon::Sun => {
            painter.circle_filled(o, 3.0, c);
            for i in 0..8 {
                let a = std::f32::consts::TAU * i as f32 / 8.0;
                let d = egui::vec2(a.cos(), a.sin());
                line(o + d * 4.8, o + d * 6.6);
            }
        }
        Icon::Check => {
            painter.add(egui::Shape::line(vec![at(-5.0, 0.0), at(-1.5, 3.5), at(5.0, -3.5)], Stroke::new(1.8_f32, c)));
        }
        Icon::Clock => {
            painter.circle_stroke(o, 6.0, st);
            line(o, at(0.0, -3.8));
            line(o, at(2.8, 1.6));
        }
        Icon::Schema => {
            // a cylinder: top ellipse, sides and two bands
            let w = 6.0;
            let top = |y: f32| -> Vec<egui::Pos2> { (0..=24).map(|i| { let a = std::f32::consts::TAU * i as f32 / 24.0; at(a.cos() * w, y + a.sin() * 2.0) }).collect() };
            painter.add(egui::Shape::closed_line(top(-4.5), st));
            line(at(-w, -4.5), at(-w, 4.5));
            line(at(w, -4.5), at(w, 4.5));
            for y in [0.0_f32, 4.5] {
                let half: Vec<egui::Pos2> = (0..=12).map(|i| { let a = std::f32::consts::PI * i as f32 / 12.0; at(a.cos() * w, y + a.sin() * 2.0) }).collect();
                painter.add(egui::Shape::line(half, st));
            }
        }
        Icon::Undo => {
            // an arrow curving back over the top: from the right, ending at the
            // left and pointing down
            let pts = arc(0.0, 1.5, 4.5, 360.0, 180.0);
            let tip = pts[pts.len() - 1];
            let prev = pts[pts.len() - 2];
            painter.add(egui::Shape::line(pts.clone(), st));
            let d = (tip - prev).normalized();
            let n = egui::vec2(-d.y, d.x);
            painter.add(egui::Shape::convex_polygon(vec![tip + d * 2.8, tip + n * 2.8 - d * 0.8, tip - n * 2.8 - d * 0.8], c, Stroke::NONE));
        }
    }
}
