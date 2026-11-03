//! ER diagram of one schema: tables as boxes, foreign keys as curved edges
//! with crow's-foot ends. Pan by dragging the background, zoom with the
//! wheel, move tables by dragging their header.

use crate::theme::{bold, Palette};
use databug_core::{ObjectKind, SchemaInfo, TableInfo};
use eframe::egui::{self, epaint::CubicBezierShape, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use std::collections::{HashMap, VecDeque};

const HEADER: f32 = 28.0;
const ROW: f32 = 19.0;
const MAX_ROWS: usize = 24;

struct Node {
    table: TableInfo,
    pos: Pos2,
    size: Vec2,
}

pub struct ErView {
    pub conn_id: String,
    pub schema: String,
    nodes: Vec<Node>,
    pan: Vec2,
    zoom: f32,
    dragging: Option<usize>,
    selected: Option<usize>,
    fitted: bool,
    pub include_views: bool,
}

pub enum ErAction {
    None,
    EditTable(String),
    Copy(String, String),
    Refresh,
}

impl ErView {
    pub fn new(conn_id: String, schema: &SchemaInfo) -> Self {
        let mut v = ErView { conn_id, schema: schema.name.clone(), nodes: Vec::new(), pan: Vec2::ZERO, zoom: 1.0, dragging: None, selected: None, fitted: false, include_views: false };
        v.load(schema);
        v
    }

    pub fn load(&mut self, schema: &SchemaInfo) {
        let old: HashMap<String, Pos2> = self.nodes.iter().map(|n| (n.table.name.clone(), n.pos)).collect();
        self.nodes = schema
            .tables
            .iter()
            .filter(|t| t.kind == ObjectKind::Table || (self.include_views && t.kind != ObjectKind::Collection))
            .map(|t| {
                let longest = t.columns.iter().take(MAX_ROWS).map(|c| c.name.chars().count() + c.data_type.chars().count() + 3).max().unwrap_or(10).max(t.name.chars().count() + 2);
                let rows = t.columns.len().min(MAX_ROWS) + usize::from(t.columns.len() > MAX_ROWS);
                Node { table: t.clone(), pos: Pos2::ZERO, size: Vec2::new((longest as f32 * 7.3 + 46.0).clamp(150.0, 380.0), HEADER + rows as f32 * ROW + 8.0) }
            })
            .collect();
        if old.is_empty() || self.nodes.iter().any(|n| !old.contains_key(&n.table.name)) {
            self.auto_layout();
            self.fitted = false;
        } else {
            for n in &mut self.nodes {
                n.pos = old[&n.table.name];
            }
        }
    }

    fn index(&self) -> HashMap<String, usize> {
        self.nodes.iter().enumerate().map(|(i, n)| (n.table.name.to_lowercase(), i)).collect()
    }

    /// Lay tables out in a grid, ordering them so related tables sit near
    /// each other (breadth-first over the foreign-key graph).
    pub fn auto_layout(&mut self) {
        let idx = self.index();
        let n = self.nodes.len();
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, node) in self.nodes.iter().enumerate() {
            for fk in &node.table.foreign_keys {
                if let Some(&j) = idx.get(&fk.ref_table.to_lowercase()) {
                    if i != j {
                        adj[i].push(j);
                        adj[j].push(i);
                    }
                }
            }
        }
        let mut order = Vec::with_capacity(n);
        let mut seen = vec![false; n];
        let mut starts: Vec<usize> = (0..n).collect();
        starts.sort_by_key(|&i| std::cmp::Reverse(adj[i].len()));
        for s in starts {
            if seen[s] {
                continue;
            }
            let mut q = VecDeque::from([s]);
            seen[s] = true;
            while let Some(u) = q.pop_front() {
                order.push(u);
                let mut next: Vec<usize> = adj[u].iter().copied().filter(|v| !seen[*v]).collect();
                next.sort_by_key(|&v| std::cmp::Reverse(adj[v].len()));
                next.dedup();
                for v in next {
                    if !seen[v] {
                        seen[v] = true;
                        q.push_back(v);
                    }
                }
            }
        }
        let cols = ((n as f32 * 1.6).sqrt().ceil() as usize).max(1);
        let gap = Vec2::new(70.0, 50.0);
        let mut col_w = vec![0.0f32; cols];
        let rows = n.div_ceil(cols);
        let mut row_h = vec![0.0f32; rows];
        for (k, &i) in order.iter().enumerate() {
            col_w[k % cols] = col_w[k % cols].max(self.nodes[i].size.x);
            row_h[k / cols] = row_h[k / cols].max(self.nodes[i].size.y);
        }
        for (k, &i) in order.iter().enumerate() {
            let (c, r) = (k % cols, k / cols);
            let x: f32 = col_w[..c].iter().sum::<f32>() + gap.x * c as f32;
            let y: f32 = row_h[..r].iter().sum::<f32>() + gap.y * r as f32;
            self.nodes[i].pos = Pos2::new(x, y);
        }
    }

    fn bounds(&self) -> Rect {
        self.nodes.iter().fold(Rect::NOTHING, |r, n| r.union(Rect::from_min_size(n.pos, n.size)))
    }

    fn fit(&mut self, view: Rect) {
        let b = self.bounds();
        if !b.is_positive() {
            return;
        }
        self.zoom = ((view.width() - 60.0) / b.width()).min((view.height() - 60.0) / b.height()).clamp(0.2, 1.2);
        self.pan = view.center().to_vec2() - view.min.to_vec2() - b.center().to_vec2() * self.zoom;
    }

    pub fn mermaid(&self) -> String {
        let mut s = String::from("erDiagram\n");
        for n in &self.nodes {
            s.push_str(&format!("    {} {{\n", n.table.name));
            for c in &n.table.columns {
                let ty: String = c.data_type.chars().filter(|ch| ch.is_alphanumeric() || *ch == '_').collect();
                let key = if c.primary_key { " PK" } else if n.table.is_fk_column(&c.name) { " FK" } else { "" };
                s.push_str(&format!("        {} {}{}\n", if ty.is_empty() { "text" } else { &ty }, c.name, key));
            }
            s.push_str("    }\n");
        }
        for n in &self.nodes {
            for fk in &n.table.foreign_keys {
                s.push_str(&format!("    {} ||--o{{ {} : \"{}\"\n", fk.ref_table, n.table.name, fk.columns.join(", ")));
            }
        }
        s
    }

    pub fn show(&mut self, ui: &mut egui::Ui, p: &Palette) -> ErAction {
        let mut action = ErAction::None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("ER diagram · {}", self.schema)).font(bold(14.0)));
            let edges: usize = self.nodes.iter().map(|n| n.table.foreign_keys.len()).sum();
            ui.label(RichText::new(format!("{} tables · {} relationships", self.nodes.len(), edges)).color(p.muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Copy as Mermaid").on_hover_text("Paste into GitHub, Notion or any Mermaid renderer").clicked() {
                    action = ErAction::Copy(self.mermaid(), "Copied the diagram as Mermaid".into());
                }
                if ui.button("Refresh").clicked() {
                    action = ErAction::Refresh;
                }
                if ui.button("Arrange").clicked() {
                    self.auto_layout();
                    self.fitted = false;
                }
                if ui.button("Fit").clicked() {
                    self.fitted = false;
                }
                if ui.checkbox(&mut self.include_views, "Views").changed() {
                    action = ErAction::Refresh;
                }
                ui.label(RichText::new(format!("{:.0}%", self.zoom * 100.0)).color(p.faint));
            });
        });
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(8), p.canvas);
        // dot grid
        let step = 24.0 * self.zoom;
        if step > 8.0 {
            let off = Vec2::new(self.pan.x.rem_euclid(step), self.pan.y.rem_euclid(step));
            let mut y = rect.top() + off.y;
            while y < rect.bottom() {
                let mut x = rect.left() + off.x;
                while x < rect.right() {
                    painter.circle_filled(Pos2::new(x, y), 0.8, p.border);
                    x += step;
                }
                y += step;
            }
        }
        if self.nodes.is_empty() {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "No tables in this schema", FontId::proportional(14.0), p.muted);
            return action;
        }
        if !self.fitted {
            self.fit(rect);
            self.fitted = true;
        }
        let z = self.zoom;
        let to_screen = |w: Pos2, pan: Vec2| rect.min + pan + w.to_vec2() * z;
        let to_world = |s: Pos2, pan: Vec2| ((s - rect.min - pan) / z).to_pos2();

        // ---- input: zoom, pan, drag nodes ----
        if resp.hovered() {
            let (scroll, pointer) = ui.input(|i| (i.smooth_scroll_delta.y + i.zoom_delta().ln() * 300.0, i.pointer.hover_pos()));
            if scroll.abs() > 0.0 {
                if let Some(ptr) = pointer {
                    let before = to_world(ptr, self.pan);
                    self.zoom = (self.zoom * (1.0 + scroll * 0.0015)).clamp(0.15, 2.5);
                    self.pan = ptr - rect.min - before.to_vec2() * self.zoom;
                }
            }
        }
        let hit = |pos: Pos2, nodes: &[Node], pan: Vec2| -> Option<(usize, bool)> {
            let w = to_world(pos, pan);
            nodes.iter().enumerate().rev().find(|(_, n)| Rect::from_min_size(n.pos, n.size).contains(w)).map(|(i, n)| (i, w.y - n.pos.y <= HEADER))
        };
        if resp.drag_started() {
            if let Some(pos) = resp.interact_pointer_pos() {
                self.dragging = hit(pos, &self.nodes, self.pan).map(|(i, _)| i);
            }
        }
        if resp.dragged() {
            let d = resp.drag_delta();
            match self.dragging {
                Some(i) => self.nodes[i].pos += d / self.zoom,
                None => self.pan += d,
            }
        }
        if resp.drag_stopped() {
            self.dragging = None;
        }
        if resp.clicked() {
            self.selected = resp.interact_pointer_pos().and_then(|pos| hit(pos, &self.nodes, self.pan)).map(|(i, _)| i);
        }
        if resp.double_clicked() {
            if let Some((i, _)) = resp.interact_pointer_pos().and_then(|pos| hit(pos, &self.nodes, self.pan)) {
                if self.nodes[i].table.kind == ObjectKind::Table {
                    action = ErAction::EditTable(self.nodes[i].table.name.clone());
                }
            }
        }
        let idx = self.index();

        // ---- edges ----
        let row_y = |n: &Node, col: &str| -> f32 {
            let i = n.table.columns.iter().take(MAX_ROWS).position(|c| c.name.eq_ignore_ascii_case(col)).unwrap_or(0);
            n.pos.y + HEADER + 4.0 + ROW * (i as f32 + 0.5)
        };
        for (ci, child) in self.nodes.iter().enumerate() {
            for fk in &child.table.foreign_keys {
                let Some(&pi) = idx.get(&fk.ref_table.to_lowercase()) else { continue };
                let parent = &self.nodes[pi];
                let highlighted = self.selected == Some(ci) || self.selected == Some(pi);
                let color = if highlighted { p.accent } else { p.muted.gamma_multiply(0.8) };
                let stroke = Stroke::new(if highlighted { 2.0 } else { 1.3 } * z.sqrt(), color);
                let cy = row_y(child, fk.columns.first().map(String::as_str).unwrap_or(""));
                let py = row_y(parent, fk.ref_columns.first().map(String::as_str).unwrap_or(""));
                let (cx, px, dir) = if ci == pi {
                    (child.pos.x + child.size.x, parent.pos.x + parent.size.x, 1.0)
                } else if parent.pos.x + parent.size.x / 2.0 > child.pos.x + child.size.x / 2.0 {
                    (child.pos.x + child.size.x, parent.pos.x, 1.0)
                } else {
                    (child.pos.x, parent.pos.x + parent.size.x, -1.0)
                };
                let a = to_screen(Pos2::new(cx, cy), self.pan);
                let b = to_screen(Pos2::new(px, py), self.pan);
                let bend = ((b.x - a.x).abs() * 0.5).max(40.0 * z);
                let back = if ci == pi { 1.0 } else { -1.0 };
                let curve = CubicBezierShape::from_points_stroke([a, a + Vec2::new(bend * dir, 0.0), b + Vec2::new(bend * dir * back, 0.0), b], false, Color32::TRANSPARENT, stroke);
                painter.add(curve);
                // crow's foot at the child ("many") end, bar at the parent ("one") end
                let f = 9.0 * z * dir;
                painter.line_segment([a + Vec2::new(f, 0.0), a + Vec2::new(0.0, -5.0 * z)], stroke);
                painter.line_segment([a + Vec2::new(f, 0.0), a + Vec2::new(0.0, 5.0 * z)], stroke);
                let g = -9.0 * z * dir * back;
                painter.line_segment([b + Vec2::new(g * 0.6, -5.0 * z), b + Vec2::new(g * 0.6, 5.0 * z)], stroke);
            }
        }

        // ---- tables ----
        for (i, n) in self.nodes.iter().enumerate() {
            let r = Rect::from_min_size(to_screen(n.pos, self.pan), n.size * z);
            if !r.intersects(rect) {
                continue;
            }
            let selected = self.selected == Some(i);
            let color = if n.table.kind == ObjectKind::Table { p.accent } else { p.syn_quoted };
            painter.rect(r, CornerRadius::same((8.0 * z) as u8), p.island, Stroke::new(if selected { 2.0_f32 } else { 1.0 }, if selected { p.accent } else { p.border }), egui::StrokeKind::Inside);
            let head = Rect::from_min_size(r.min, Vec2::new(r.width(), HEADER * z));
            painter.rect_filled(head.shrink(1.0), CornerRadius { nw: (7.0 * z) as u8, ne: (7.0 * z) as u8, sw: 0, se: 0 }, color.gamma_multiply(0.18));
            if z < 0.35 {
                painter.text(r.center(), egui::Align2::CENTER_CENTER, &n.table.name, bold(11.0), p.text);
                continue;
            }
            painter.text(head.left_center() + Vec2::new(10.0 * z, 0.0), egui::Align2::LEFT_CENTER, &n.table.name, bold(13.0 * z), p.text);
            for (k, c) in n.table.columns.iter().take(MAX_ROWS).enumerate() {
                let y = r.top() + (HEADER + 4.0 + ROW * (k as f32 + 0.5)) * z;
                let fk = n.table.is_fk_column(&c.name);
                let icon = Rect::from_center_size(Pos2::new(r.left() + 14.0 * z, y), Vec2::splat(12.0 * z));
                if z > 0.6 {
                    crate::icons::paint_column(&painter, icon, c.primary_key, fk, p);
                }
                let name_color = if c.primary_key { p.warning } else { p.text };
                painter.text(Pos2::new(r.left() + 26.0 * z, y), egui::Align2::LEFT_CENTER, &c.name, FontId::proportional(12.0 * z), name_color);
                painter.text(Pos2::new(r.right() - 9.0 * z, y), egui::Align2::RIGHT_CENTER, &c.data_type, FontId::monospace(10.5 * z), p.faint);
            }
            if n.table.columns.len() > MAX_ROWS {
                let y = r.top() + (HEADER + 4.0 + ROW * (MAX_ROWS as f32 + 0.5)) * z;
                painter.text(Pos2::new(r.left() + 26.0 * z, y), egui::Align2::LEFT_CENTER, format!("+ {} more", n.table.columns.len() - MAX_ROWS), FontId::proportional(11.0 * z), p.muted);
            }
        }
        if resp.hovered() && self.dragging.is_none() {
            if let Some((i, _)) = ui.input(|i| i.pointer.hover_pos()).and_then(|pos| hit(pos, &self.nodes, self.pan)) {
                let t = &self.nodes[i].table;
                resp.on_hover_text(format!("{} · {} columns · {} foreign keys\nDouble-click to modify", t.name, t.columns.len(), t.foreign_keys.len()));
            }
        }
        action
    }
}

use eframe::egui::RichText;
