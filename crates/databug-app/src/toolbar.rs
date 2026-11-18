//! Controls for the top toolbar. Every one is `H` tall with the same corners,
//! border and text size, so the toolbar lines up: buttons (plain, primary,
//! danger, success, accent, icon-only), dropdown buttons, a segmented switch,
//! badges, the connection chip and dividers. Icons are painted, not emoji, so
//! they look the same on every platform.

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
    /// Panel fill with a thin border.
    Normal,
    /// The main action (Run): filled with the success color.
    Primary,
    /// Red tint (Cancel while something runs).
    Danger,
    /// Green tint (Commit).
    Success,
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
    Database,
    Consoles,
    Format,
    Gear,
    Moon,
    Sun,
    Check,
    Undo,
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
        (_, false) => (p.raised.gamma_multiply(0.7), thin(p.border.gamma_multiply(0.7)), p.faint),
        (Kind::Normal, true) => (if pressed { p.selection } else if hovered || open { p.hover } else { p.raised }, thin(if hovered || open { p.muted.gamma_multiply(0.5) } else { p.border }), p.text),
        (Kind::Ghost, true) => (if pressed { p.selection } else if hovered || open { p.hover } else { Color32::TRANSPARENT }, none, if hovered || open { p.text } else { p.muted }),
        (Kind::Primary, true) => {
            let f = if pressed { p.success.gamma_multiply(0.8) } else if hovered { p.success.gamma_multiply(0.9) } else { p.success };
            (f, none, on(p.success))
        }
        (Kind::Accent, true) => {
            let f = if pressed { p.accent.gamma_multiply(0.8) } else if hovered { p.accent.gamma_multiply(0.9) } else { p.accent };
            (f, none, on(p.accent))
        }
        (Kind::Danger, true) => (p.danger.gamma_multiply(if hovered { 0.26 } else { 0.16 }), thin(p.danger.gamma_multiply(0.5)), p.danger),
        (Kind::Success, true) => (p.success.gamma_multiply(if hovered { 0.26 } else { 0.16 }), thin(p.success.gamma_multiply(0.5)), p.success),
    };
    // a disabled Run still shows its play icon in green, faintly
    let icon_color = if !enabled && kind == Kind::Primary { p.success.gamma_multiply(0.45) } else { fg };
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

/// A segmented switch (Auto | Manual). Returns the option clicked.
pub fn segmented(ui: &mut Ui, p: &Palette, options: &[&str], selected: usize, tips: &[&str]) -> Option<usize> {
    let widths: Vec<f32> = options.iter().map(|o| ui.painter().layout_no_wrap(o.to_string(), font(), Color32::WHITE).size().x + 22.0).collect();
    let total = widths.iter().sum::<f32>() + 4.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(total, H), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(RADIUS), p.island, Stroke::new(1.0_f32, p.border), egui::StrokeKind::Inside);
    let mut x = rect.left() + 2.0;
    let mut picked = None;
    for (i, (o, w)) in options.iter().zip(&widths).enumerate() {
        let seg = Rect::from_min_size(egui::pos2(x, rect.top() + 2.0), egui::vec2(*w, H - 4.0));
        let resp = ui.interact(seg, ui.id().with(("seg", o)), Sense::click());
        let on_it = i == selected;
        if on_it {
            ui.painter().rect(seg, CornerRadius::same(RADIUS - 2), p.accent.gamma_multiply(0.22), Stroke::new(1.0_f32, p.accent.gamma_multiply(0.55)), egui::StrokeKind::Inside);
        } else if resp.hovered() {
            ui.painter().rect_filled(seg, CornerRadius::same(RADIUS - 2), p.hover);
        }
        ui.painter().text(seg.center(), egui::Align2::CENTER_CENTER, *o, font(), if on_it { p.text } else { p.muted });
        let resp = match tips.get(i) {
            Some(t) => resp.on_hover_text(*t),
            None => resp,
        };
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() && !on_it {
            picked = Some(i);
        }
        x += w;
    }
    picked
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

/// The active connection: engine logo, live dot (filled) or ring, name and engine.
pub fn connection_chip(ui: &mut Ui, p: &Palette, logos: &crate::icons::Logos, cfg: &databug_core::ConnectionConfig, live: bool) -> Response {
    let (kind, color, name, engine) = (cfg.kind, crate::theme::connection_color(cfg.color), cfg.name.as_str(), cfg.kind.label());
    let small = FontId::proportional(11.5);
    let nw = ui.painter().layout_no_wrap(name.to_string(), font(), p.text).size().x;
    let ew = ui.painter().layout_no_wrap(engine.to_string(), small.clone(), p.muted).size().x;
    let w = 8.0 + 16.0 + 8.0 + 8.0 + 6.0 + nw + 6.0 + ew + 10.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, H), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(RADIUS), p.island, Stroke::new(1.0_f32, p.border), egui::StrokeKind::Inside);
    let cy = rect.center().y;
    let mut x = rect.left() + 8.0;
    logos.paint(ui, Rect::from_min_size(egui::pos2(x, cy - 8.0), egui::vec2(16.0, 16.0)), kind);
    x += 16.0 + 8.0;
    let dot = egui::pos2(x + 4.0, cy);
    if live {
        ui.painter().circle_filled(dot, 4.0, color);
    } else {
        ui.painter().circle_stroke(dot, 3.4, Stroke::new(1.4_f32, color));
    }
    x += 8.0 + 6.0;
    ui.painter().text(egui::pos2(x, cy), egui::Align2::LEFT_CENTER, name, font(), p.text);
    x += nw + 6.0;
    ui.painter().text(egui::pos2(x, cy + 0.5), egui::Align2::LEFT_CENTER, engine, small, p.muted);
    resp
}

/// Paint `icon` in a 14×14 `r`. `bg` is the button fill (for cut-outs).
pub fn paint_icon(painter: &egui::Painter, r: Rect, icon: Icon, c: Color32, bg: Color32) {
    let st = Stroke::new(1.5_f32, c);
    let ctr = r.center();
    match icon {
        Icon::Play => {
            let pts = vec![ctr + egui::vec2(-4.0, -5.5), ctr + egui::vec2(5.5, 0.0), ctr + egui::vec2(-4.0, 5.5)];
            painter.add(egui::Shape::convex_polygon(pts, c, Stroke::NONE));
        }
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(ctr, egui::vec2(9.0, 9.0)), CornerRadius::same(2), c);
        }
        Icon::Plan => {
            // a small tree: root, then two children
            painter.circle_filled(ctr + egui::vec2(-4.5, -4.5), 1.8, c);
            painter.line_segment([ctr + egui::vec2(-4.5, -3.0), ctr + egui::vec2(-4.5, 4.5)], st);
            painter.line_segment([ctr + egui::vec2(-4.5, 0.0), ctr + egui::vec2(1.0, 0.0)], st);
            painter.line_segment([ctr + egui::vec2(-4.5, 4.5), ctr + egui::vec2(1.0, 4.5)], st);
            painter.circle_filled(ctr + egui::vec2(3.5, 0.0), 1.8, c);
            painter.circle_filled(ctr + egui::vec2(3.5, 4.5), 1.8, c);
        }
        Icon::Database => {
            let w = 10.0;
            let top = ctr.y - 4.5;
            let bot = ctr.y + 4.5;
            painter.add(egui::Shape::ellipse_stroke(egui::pos2(ctr.x, top), egui::vec2(w / 2.0, 2.0), st));
            painter.line_segment([egui::pos2(ctr.x - w / 2.0, top), egui::pos2(ctr.x - w / 2.0, bot)], st);
            painter.line_segment([egui::pos2(ctr.x + w / 2.0, top), egui::pos2(ctr.x + w / 2.0, bot)], st);
            let arc: Vec<egui::Pos2> = (0..=12).map(|i| {
                let a = std::f32::consts::PI * i as f32 / 12.0;
                egui::pos2(ctr.x + a.cos() * w / 2.0, bot + a.sin() * 2.0)
            }).collect();
            painter.add(egui::Shape::line(arc, st));
        }
        Icon::Consoles => {
            let back = Rect::from_center_size(ctr + egui::vec2(2.0, -2.0), egui::vec2(9.0, 11.0));
            let front = Rect::from_center_size(ctr + egui::vec2(-1.5, 1.5), egui::vec2(9.0, 11.0));
            painter.line_segment([back.left_top(), back.right_top()], st);
            painter.line_segment([back.right_top(), back.right_bottom()], st);
            painter.rect_filled(front, CornerRadius::same(2), bg);
            painter.rect_stroke(front, CornerRadius::same(2), st, egui::StrokeKind::Middle);
            for dy in [-1.5_f32, 1.5] {
                painter.line_segment([egui::pos2(front.left() + 2.5, front.center().y + dy), egui::pos2(front.right() - 2.5, front.center().y + dy)], Stroke::new(1.0_f32, c));
            }
        }
        Icon::Format => {
            for (dy, w) in [(-4.5_f32, 11.0_f32), (-1.5, 7.0), (1.5, 11.0), (4.5, 7.0)] {
                let x0 = ctr.x - 5.5;
                painter.line_segment([egui::pos2(x0, ctr.y + dy), egui::pos2(x0 + w, ctr.y + dy)], st);
            }
        }
        Icon::Gear => {
            let teeth = 8;
            for i in 0..teeth {
                let a = std::f32::consts::TAU * i as f32 / teeth as f32;
                let d = egui::vec2(a.cos(), a.sin());
                painter.line_segment([ctr + d * 4.5, ctr + d * 6.8], Stroke::new(2.4_f32, c));
            }
            painter.circle_stroke(ctr, 4.6, Stroke::new(1.6_f32, c));
            painter.circle_filled(ctr, 1.6, c);
        }
        Icon::Moon => {
            painter.circle_filled(ctr, 5.5, c);
            painter.circle_filled(ctr + egui::vec2(3.0, -2.2), 4.8, bg);
        }
        Icon::Sun => {
            painter.circle_filled(ctr, 3.2, c);
            for i in 0..8 {
                let a = std::f32::consts::TAU * i as f32 / 8.0;
                let d = egui::vec2(a.cos(), a.sin());
                painter.line_segment([ctr + d * 5.0, ctr + d * 6.8], Stroke::new(1.4_f32, c));
            }
        }
        Icon::Check => {
            painter.line_segment([ctr + egui::vec2(-5.0, 0.5), ctr + egui::vec2(-1.5, 4.0)], Stroke::new(1.8_f32, c));
            painter.line_segment([ctr + egui::vec2(-1.5, 4.0), ctr + egui::vec2(5.0, -4.0)], Stroke::new(1.8_f32, c));
        }
        Icon::Undo => {
            let pts: Vec<egui::Pos2> = (0..=14).map(|i| {
                let a = std::f32::consts::PI * (1.1 + 1.2 * i as f32 / 14.0);
                ctr + egui::vec2(1.0 + a.cos() * -4.5, 1.0 + a.sin() * -4.5)
            }).collect();
            let start = pts[0];
            painter.add(egui::Shape::line(pts, st));
            painter.add(egui::Shape::convex_polygon(vec![start + egui::vec2(-3.5, -1.0), start + egui::vec2(2.0, -3.0), start + egui::vec2(1.0, 2.5)], c, Stroke::NONE));
        }
    }
}
