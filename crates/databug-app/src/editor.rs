//! The SQL editor: syntax highlighting, line numbers, current-line and
//! running-statement highlights, inspections, and the completion popup.

use crate::theme::Palette;
use eframe::egui::{self, text::CCursor, text::CCursorRange, Color32, CornerRadius, FontId, Galley, Key, Margin, Modifiers, Pos2, Rect, RichText, Sense, Stroke, TextFormat};
use std::ops::Range;
use std::sync::Arc;
use databug_core::{Catalog, Dialect};
use databug_sql::{complete, inspect, tokenize, CompletionItem, Diagnostic, ItemKind, Severity, TokKind};

struct Popup {
    items: Vec<CompletionItem>,
    replace: Range<usize>,
    selected: usize,
    scroll_to_selected: bool,
}

pub struct Editor {
    pub text: String,
    pub id: egui::Id,
    /// Cursor and selection as byte offsets, updated every frame.
    pub cursor: usize,
    pub selection: Option<Range<usize>>,
    pub running: Option<Range<usize>>,
    pub error: Option<Range<usize>>,
    pub dialect: Dialect,
    popup: Option<Popup>,
    diagnostics: Vec<Diagnostic>,
    diag_key: (u64, u64),
    version: u64,
    job_cache: Option<(u64, f32, egui::text::LayoutJob)>,
    last_galley: Option<(Arc<Galley>, Pos2)>,
    pending_cursor: Option<(usize, Option<usize>)>,
    wants_focus: bool,
}

fn char_to_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}

fn byte_to_char(s: &str, b: usize) -> usize {
    s[..b.min(s.len())].chars().count()
}

fn pos_of(g: &Galley, ci: usize) -> Rect {
    g.pos_from_cursor(&g.from_ccursor(CCursor::new(ci)))
}

impl Editor {
    pub fn new(id: egui::Id, text: String, dialect: Dialect) -> Self {
        Editor {
            text,
            id,
            cursor: 0,
            selection: None,
            running: None,
            error: None,
            dialect,
            popup: None,
            diagnostics: Vec::new(),
            diag_key: (u64::MAX, u64::MAX),
            version: 1,
            job_cache: None,
            last_galley: None,
            pending_cursor: None,
            wants_focus: true,
        }
    }

    pub fn focus(&mut self) {
        self.wants_focus = true;
    }

    /// Replace the whole text and put the cursor at the end.
    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.version += 1;
        let end = self.text.len();
        self.pending_cursor = Some((end, None));
        self.popup = None;
        self.error = None;
    }

    /// Insert text at the cursor (replacing the selection).
    pub fn insert(&mut self, s: &str) {
        let (a, b) = match &self.selection {
            Some(r) => (r.start, r.end),
            None => (self.cursor, self.cursor),
        };
        let a = a.min(self.text.len());
        let b = b.min(self.text.len()).max(a);
        self.text.replace_range(a..b, s);
        self.version += 1;
        self.pending_cursor = Some((a + s.len(), None));
        self.wants_focus = true;
    }

    /// Select a byte range (used to show which statement will run).
    pub fn select(&mut self, r: Range<usize>) {
        self.pending_cursor = Some((r.end, Some(r.start)));
    }

    pub fn selected_text(&self) -> Option<&str> {
        let r = self.selection.as_ref()?;
        let s = self.text.get(r.clone())?;
        (!s.trim().is_empty()).then_some(s)
    }

    pub fn line_col(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor.min(self.text.len())];
        let line = before.matches('\n').count() + 1;
        let col = before.rsplit('\n').next().map(|l| l.chars().count()).unwrap_or(0) + 1;
        (line, col)
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    fn open_completion(&mut self, catalog: Option<&Catalog>, explicit: bool) {
        let c = complete(&self.text, self.cursor, catalog, self.dialect);
        if c.items.is_empty() || (!explicit && c.prefix.is_empty() && !self.text[..self.cursor].ends_with('.')) {
            self.popup = None;
            return;
        }
        self.popup = Some(Popup { items: c.items, replace: c.replace, selected: 0, scroll_to_selected: true });
    }

    fn accept(&mut self, idx: usize) {
        let Some(p) = self.popup.take() else { return };
        let Some(item) = p.items.get(idx) else { return };
        let r = p.replace.start.min(self.text.len())..p.replace.end.min(self.text.len());
        self.text.replace_range(r.clone(), &item.insert);
        self.version += 1;
        self.pending_cursor = Some((r.start + item.insert.len(), None));
    }

    fn toggle_comment(&mut self) {
        let (a, b) = match &self.selection {
            Some(r) => (r.start, r.end),
            None => (self.cursor, self.cursor),
        };
        let start = self.text[..a].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let end = self.text[b..].find('\n').map(|p| b + p).unwrap_or(self.text.len());
        let block = &self.text[start..end];
        let marker = if self.dialect == Dialect::Mongo { "//" } else { "--" };
        let all_commented = block.lines().filter(|l| !l.trim().is_empty()).all(|l| l.trim_start().starts_with(marker));
        let new: Vec<String> = block
            .split('\n')
            .map(|l| {
                if all_commented {
                    let indent = l.len() - l.trim_start().len();
                    let rest = l.trim_start().strip_prefix(marker).unwrap_or(l.trim_start());
                    let rest = rest.strip_prefix(' ').unwrap_or(rest);
                    format!("{}{}", &l[..indent], rest)
                } else if l.trim().is_empty() {
                    l.to_string()
                } else {
                    format!("{marker} {l}")
                }
            })
            .collect();
        let new = new.join("\n");
        let delta = new.len() as isize - block.len() as isize;
        self.text.replace_range(start..end, &new);
        self.version += 1;
        let new_end = (end as isize + delta).max(start as isize) as usize;
        self.pending_cursor = Some((new_end, if a != b { Some(start) } else { None }));
    }

    fn layout_job(&mut self, text: &str, p: &Palette, wrap: f32, font: &FontId) -> egui::text::LayoutJob {
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut h);
            self.diag_key.hash(&mut h);
            h.finish()
        };
        if let Some((k, w, job)) = &self.job_cache {
            if *k == key && *w == wrap {
                return job.clone();
            }
        }
        let mut job = egui::text::LayoutJob { text: text.to_string(), ..Default::default() };
        job.wrap.max_width = wrap;
        let toks = tokenize(text, self.dialect);
        let fmt = |color: Color32, italics: bool, underline: Option<Color32>| TextFormat {
            font_id: font.clone(),
            color,
            italics,
            underline: underline.map(|c| Stroke::new(1.5_f32, c)).unwrap_or(Stroke::NONE),
            ..Default::default()
        };
        for (i, t) in toks.iter().enumerate() {
            let s = t.text(text);
            let next_is_paren = toks[i + 1..].iter().find(|t| t.kind != TokKind::Whitespace).map(|n| n.text(text) == "(").unwrap_or(false);
            let (color, italic) = match t.kind {
                TokKind::Keyword => (p.syn_keyword, false),
                TokKind::String => (p.syn_string, false),
                TokKind::Number => (p.syn_number, false),
                TokKind::Comment => (p.syn_comment, true),
                TokKind::QuotedIdent => (p.syn_quoted, false),
                TokKind::Param => (p.syn_param, false),
                TokKind::Operator | TokKind::Punct => (p.syn_operator, false),
                TokKind::Ident if next_is_paren => (p.syn_function, false),
                TokKind::Ident if self.dialect == Dialect::Mongo && (s == "db" || s.starts_with('$')) => (p.syn_keyword, false),
                _ => (p.syn_ident, false),
            };
            let underline = self.diagnostics.iter().find(|d| d.range.start <= t.start && t.end <= d.range.end && t.kind != TokKind::Whitespace).map(|d| if d.severity == Severity::Error { p.danger } else { p.warning });
            job.sections.push(egui::text::LayoutSection { leading_space: 0.0, byte_range: t.start..t.end, format: fmt(color, italic, underline) });
        }
        self.job_cache = Some((key, wrap, job.clone()));
        job
    }

    /// Draw the editor. `catalog_gen` changes whenever the catalog is refreshed.
    pub fn show(&mut self, ui: &mut egui::Ui, p: &Palette, catalog: Option<&Catalog>, catalog_gen: u64, auto_complete: bool, font_size: f32) {
        // ---- inspections (recomputed when text or catalog change) ----
        if self.diag_key != (self.version, catalog_gen) && self.text.len() < 300_000 {
            self.diagnostics = inspect(&self.text, catalog, self.dialect);
            self.diag_key = (self.version, catalog_gen);
            self.job_cache = None;
        }

        let has_focus = ui.ctx().memory(|m| m.has_focus(self.id));

        // ---- keys we handle before the text edit sees them ----
        let mut accept: Option<usize> = None;
        let mut explicit_complete = false;
        let mut toggle_comment = false;
        let mut insert_spaces = false;
        if has_focus {
            ui.input_mut(|i| {
                if let Some(pop) = self.popup.as_mut() {
                    let n = pop.items.len();
                    if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                        pop.selected = (pop.selected + 1) % n;
                        pop.scroll_to_selected = true;
                    }
                    if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                        pop.selected = (pop.selected + n - 1) % n;
                        pop.scroll_to_selected = true;
                    }
                    if i.consume_key(Modifiers::NONE, Key::PageDown) {
                        pop.selected = (pop.selected + 8).min(n - 1);
                        pop.scroll_to_selected = true;
                    }
                    if i.consume_key(Modifiers::NONE, Key::PageUp) {
                        pop.selected = pop.selected.saturating_sub(8);
                        pop.scroll_to_selected = true;
                    }
                    if i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Tab) {
                        accept = Some(pop.selected);
                    }
                    if i.consume_key(Modifiers::NONE, Key::Escape) {
                        self.popup = None;
                    }
                } else if i.consume_key(Modifiers::NONE, Key::Tab) {
                    insert_spaces = true;
                }
                if i.consume_key(Modifiers::CTRL, Key::Space) {
                    explicit_complete = true;
                }
                if i.consume_key(Modifiers::COMMAND, Key::Slash) {
                    toggle_comment = true;
                }
            });
        }
        if let Some(idx) = accept {
            self.accept(idx);
        }
        if toggle_comment {
            self.toggle_comment();
        }
        if insert_spaces {
            self.insert("    ");
        }

        // ---- apply a pending cursor move ----
        if let Some((c, anchor)) = self.pending_cursor.take() {
            let c = c.min(self.text.len());
            let mut state = egui::TextEdit::load_state(ui.ctx(), self.id).unwrap_or_default();
            let primary = CCursor::new(byte_to_char(&self.text, c));
            let range = match anchor {
                Some(a) => CCursorRange::two(CCursor::new(byte_to_char(&self.text, a)), primary),
                None => CCursorRange::one(primary),
            };
            state.cursor.set_char_range(Some(range));
            state.store(ui.ctx(), self.id);
            self.cursor = c;
            self.wants_focus = true;
        }

        let font = FontId::monospace(font_size);
        let line_count = self.text.matches('\n').count() + 1;
        let gutter_w = (line_count.to_string().len().max(2) as f32) * font_size * 0.62 + 26.0;

        // ---- backgrounds from last frame's layout (text unchanged since) ----
        let paint_bg = |ui: &egui::Ui, g: &Galley, origin: Pos2, r: &Range<usize>, text: &str, fill: Color32| {
            let a = byte_to_char(text, r.start);
            let b = byte_to_char(text, r.end);
            let ra = pos_of(g, a);
            let rb = pos_of(g, b);
            let full = Rect::from_min_max(egui::pos2(origin.x - 4.0, origin.y + ra.min.y - 1.0), egui::pos2(ui.max_rect().right(), origin.y + rb.max.y + 1.0));
            ui.painter().rect_filled(full, CornerRadius::same(3), fill);
        };

        let mut changed = false;
        let mut text_rect = Rect::NOTHING;
        egui::ScrollArea::both().id_salt(self.id.with("scroll")).auto_shrink([false, false]).show(ui, |ui| {
            if let Some((g, origin)) = self.last_galley.clone() {
                if g.text() == self.text {
                    if let Some(r) = &self.running {
                        paint_bg(ui, &g, origin, r, &self.text, p.accent.gamma_multiply(0.14));
                    } else if let Some(r) = &self.error {
                        paint_bg(ui, &g, origin, r, &self.text, p.danger.gamma_multiply(0.12));
                    }
                    if has_focus && self.selection.is_none() {
                        let cr = pos_of(&g, byte_to_char(&self.text, self.cursor));
                        let line = Rect::from_min_max(egui::pos2(ui.max_rect().left(), origin.y + cr.min.y), egui::pos2(ui.max_rect().right(), origin.y + cr.max.y));
                        ui.painter().rect_filled(line, CornerRadius::ZERO, p.hover.gamma_multiply(0.7));
                    }
                }
            }
            // The layouter borrows `self`, so the buffer is moved out while editing.
            let mut text = std::mem::take(&mut self.text);
            let id = self.id;
            let out = {
                let mut layouter = |ui: &egui::Ui, s: &str, wrap: f32| {
                    let job = self.layout_job(s, p, wrap, &font);
                    ui.fonts(|f| f.layout_job(job))
                };
                egui::TextEdit::multiline(&mut text)
                    .id(id)
                    .font(font.clone())
                    .code_editor()
                    .frame(false)
                    .lock_focus(true)
                    .desired_width(f32::INFINITY)
                    .desired_rows(12)
                    .margin(Margin { left: gutter_w as i8, right: 8, top: 6, bottom: 40 })
                    .layouter(&mut layouter)
                    .show(ui)
            };
            if out.response.changed() {
                changed = true;
            }
            self.text = text;
            if changed {
                self.version += 1;
                self.job_cache = None;
                self.error = None;
            }
            text_rect = out.response.rect;

            if let Some(cr) = out.cursor_range {
                let a = char_to_byte(&self.text, cr.primary.ccursor.index);
                let b = char_to_byte(&self.text, cr.secondary.ccursor.index);
                self.cursor = a;
                self.selection = if a != b { Some(a.min(b)..a.max(b)) } else { None };
            }

            // line numbers in the left margin
            let g = &out.galley;
            let origin = out.galley_pos;
            let clip = ui.clip_rect();
            let mut line = 1;
            let mut new_line = true;
            let cur_line = self.line_col().0;
            for row in &g.rows {
                let y = origin.y + row.rect.min.y;
                if new_line && y + row.rect.height() >= clip.top() && y <= clip.bottom() {
                    let color = if line == cur_line && has_focus { p.muted } else { p.faint };
                    ui.painter().text(egui::pos2(origin.x - 12.0, y + row.rect.height() / 2.0), egui::Align2::RIGHT_CENTER, line.to_string(), FontId::monospace(font_size - 1.0), color);
                }
                if new_line && y > clip.bottom() {
                    break;
                }
                new_line = row.ends_with_newline;
                if new_line {
                    line += 1;
                }
            }

            // hover tooltips for inspections
            if let Some(pos) = out.response.hover_pos() {
                let c = g.cursor_from_pos(pos - origin);
                let b = char_to_byte(&self.text, c.ccursor.index);
                if let Some(d) = self.diagnostics.iter().find(|d| d.range.start <= b && b < d.range.end) {
                    let color = if d.severity == Severity::Error { p.danger } else { p.warning };
                    let msg = d.message.clone();
                    out.response.clone().on_hover_ui_at_pointer(|ui| {
                        ui.label(RichText::new(msg).color(color));
                    });
                }
            }

            if self.wants_focus {
                out.response.request_focus();
                self.wants_focus = false;
            }
            self.last_galley = Some((out.galley.clone(), origin));
        });

        // ---- completion: open / refresh / close ----
        if explicit_complete {
            self.open_completion(catalog, true);
        } else if changed {
            let before = self.text[..self.cursor.min(self.text.len())].chars().next_back();
            let typing_word = before.map(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '$').unwrap_or(false);
            if typing_word && (auto_complete || self.popup.is_some()) {
                self.open_completion(catalog, false);
            } else {
                self.popup = None;
            }
        } else if let Some(pop) = &self.popup {
            if self.cursor < pop.replace.start || self.cursor > pop.replace.end + 64 || !has_focus && !ui.ctx().memory(|m| m.has_focus(self.id)) {
                self.popup = None;
            }
        }
        self.draw_popup(ui, p, font_size);
        let _ = text_rect;
    }

    fn draw_popup(&mut self, ui: &mut egui::Ui, p: &Palette, font_size: f32) {
        let Some((g, origin)) = self.last_galley.clone() else { return };
        let Some(pop) = self.popup.as_mut() else { return };
        let start_char = byte_to_char(&self.text, pop.replace.start.min(self.text.len()));
        let r = pos_of(&g, start_char);
        let anchor = egui::pos2(origin.x + r.min.x - 26.0, origin.y + r.max.y + 3.0);
        let mut clicked: Option<usize> = None;
        let row_h = 22.0;
        egui::Area::new(self.id.with("completion"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .constrain(true)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).fill(p.raised).inner_margin(Margin::same(4)).show(ui, |ui| {
                    ui.set_width(420.0);
                    let visible = pop.items.len().min(10);
                    egui::ScrollArea::vertical().max_height(row_h * visible as f32 + 2.0).auto_shrink([false, true]).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (i, item) in pop.items.iter().enumerate() {
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), Sense::click());
                            let selected = i == pop.selected;
                            if selected {
                                ui.painter().rect_filled(rect, CornerRadius::same(4), p.selection);
                                if pop.scroll_to_selected {
                                    resp.scroll_to_me(None);
                                }
                            } else if resp.hovered() {
                                ui.painter().rect_filled(rect, CornerRadius::same(4), p.hover);
                            }
                            let (badge, color) = kind_badge(item.kind, p);
                            let b_rect = Rect::from_center_size(egui::pos2(rect.left() + 13.0, rect.center().y), egui::vec2(16.0, 16.0));
                            ui.painter().rect_filled(b_rect, CornerRadius::same(4), color.gamma_multiply(0.22));
                            ui.painter().text(b_rect.center(), egui::Align2::CENTER_CENTER, badge, FontId::proportional(10.5), color);
                            let label_galley = ui.painter().layout_no_wrap(item.label.clone(), FontId::monospace(font_size - 0.5), p.text);
                            let lw = label_galley.size().x;
                            ui.painter().galley(egui::pos2(rect.left() + 28.0, rect.center().y - label_galley.size().y / 2.0), label_galley, p.text);
                            let max_detail = (rect.width() - lw - 48.0).max(0.0);
                            if max_detail > 30.0 {
                                let detail: String = item.detail.chars().take((max_detail / 6.5) as usize).collect();
                                ui.painter().text(egui::pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, detail, FontId::proportional(11.5), p.muted);
                            }
                            if resp.clicked() {
                                clicked = Some(i);
                            }
                        }
                    });
                    pop.scroll_to_selected = false;
                    ui.add_space(2.0);
                    ui.label(RichText::new("↑↓ to move · Enter or Tab to insert · Esc to close").size(10.5).color(p.faint));
                });
            });
        if let Some(i) = clicked {
            self.accept(i);
            self.wants_focus = true;
        }
    }
}

fn kind_badge(k: ItemKind, p: &Palette) -> (&'static str, Color32) {
    match k {
        ItemKind::Column => ("c", p.syn_function),
        ItemKind::Table => ("t", p.accent),
        ItemKind::View => ("v", p.syn_quoted),
        ItemKind::Schema => ("s", p.warning),
        ItemKind::Alias => ("a", p.syn_number),
        ItemKind::Join => ("⋈", p.success),
        ItemKind::Function => ("ƒ", p.syn_function),
        ItemKind::Keyword => ("k", p.syn_keyword),
        ItemKind::Method => ("m", p.syn_function),
        ItemKind::Operator => ("$", p.syn_param),
    }
}
