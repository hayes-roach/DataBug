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

/// The find / replace bar (Ctrl/⌘ + F, Ctrl + H), modelled on VS Code:
/// match case, whole word and regex toggles, "3 of 12", Enter / Shift+Enter
/// to step, Esc to close.
#[derive(Default)]
pub struct Find {
    pub open: bool,
    pub query: String,
    pub replace: String,
    pub show_replace: bool,
    pub case: bool,
    pub word: bool,
    pub regex: bool,
    /// index into `matches` of the highlighted match
    current: Option<usize>,
    matches: Vec<Range<usize>>,
    error: Option<String>,
    key: Option<(u64, String, bool, bool, bool)>,
    focus_query: bool,
    scroll_to: Option<Range<usize>>,
}

impl Find {
    pub fn query_id(editor: egui::Id) -> egui::Id {
        editor.with("find-query")
    }

    fn regex(&self) -> Result<Option<regex::Regex>, String> {
        if self.query.is_empty() {
            return Ok(None);
        }
        let pat = if self.regex { self.query.clone() } else { regex::escape(&self.query) };
        let pat = if self.word { format!(r"\b(?:{pat})\b") } else { pat };
        regex::RegexBuilder::new(&pat).case_insensitive(!self.case).multi_line(true).build().map(Some).map_err(|e| {
            let msg = e.to_string();
            msg.lines().last().unwrap_or("invalid pattern").trim().to_string()
        })
    }

    fn refresh(&mut self, text: &str, version: u64) {
        let key = (version, self.query.clone(), self.case, self.word, self.regex);
        if self.key.as_ref() == Some(&key) {
            return;
        }
        self.key = Some(key);
        self.matches.clear();
        self.error = None;
        match self.regex() {
            Ok(Some(re)) => {
                for m in re.find_iter(text) {
                    if m.start() == m.end() {
                        continue;
                    }
                    self.matches.push(m.range());
                    if self.matches.len() >= 20_000 {
                        break;
                    }
                }
            }
            Ok(None) => {}
            Err(e) => self.error = Some(e),
        }
        if let Some(c) = self.current {
            if c >= self.matches.len() {
                self.current = None;
            }
        }
    }
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
    pub find: Find,
    /// Set when the user asks to search the whole database for the find text.
    pub search_database: Option<String>,
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
            find: Find::default(),
            search_database: None,
        }
    }

    /// Open the find bar (or the find + replace bar), seeded with the
    /// selection when it's on one line, and focus the search field.
    pub fn open_find(&mut self, replace: bool) {
        if let Some(sel) = self.selected_text() {
            if !sel.contains('\n') && sel.len() < 200 {
                self.find.query = sel.to_string();
            }
        }
        self.find.open = true;
        self.find.show_replace |= replace;
        self.find.focus_query = true;
        self.find.key = None;
    }

    pub fn close_find(&mut self) {
        self.find.open = false;
        self.find.current = None;
        self.wants_focus = true;
    }

    pub fn find_focused(&self, ctx: &egui::Context) -> bool {
        let q = Find::query_id(self.id);
        ctx.memory(|m| m.focused() == Some(q) || m.focused() == Some(self.id.with("find-replace")))
    }

    /// Step to the next (or previous) match after the cursor.
    pub fn find_step(&mut self, forward: bool) {
        self.find.refresh(&self.text, self.version);
        let n = self.find.matches.len();
        if n == 0 {
            return;
        }
        let i = match self.find.current {
            Some(c) => {
                if forward {
                    (c + 1) % n
                } else {
                    (c + n - 1) % n
                }
            }
            None => {
                let at = self.selection.as_ref().map(|r| r.start).unwrap_or(self.cursor);
                if forward {
                    self.find.matches.iter().position(|m| m.start >= at).unwrap_or(0)
                } else {
                    self.find.matches.iter().rposition(|m| m.start < at).unwrap_or(n - 1)
                }
            }
        };
        self.go_to_match(i);
    }

    fn go_to_match(&mut self, i: usize) {
        let Some(m) = self.find.matches.get(i).cloned() else { return };
        self.find.current = Some(i);
        self.find.scroll_to = Some(m.clone());
        // select the match without taking focus from the find field
        self.pending_cursor = Some((m.end, Some(m.start)));
        self.selection = Some(m.clone());
        self.cursor = m.end;
    }

    fn replacement_for(&self, re: &regex::Regex, matched: &str) -> String {
        if self.find.regex {
            re.replace(matched, self.find.replace.as_str()).into_owned()
        } else {
            self.find.replace.clone()
        }
    }

    pub fn replace_current(&mut self) {
        self.find.refresh(&self.text, self.version);
        let Ok(Some(re)) = self.find.regex() else { return };
        let Some(i) = self.find.current.filter(|i| *i < self.find.matches.len()) else {
            self.find_step(true);
            return;
        };
        let m = self.find.matches[i].clone();
        let new = self.replacement_for(&re, &self.text[m.clone()]);
        self.text.replace_range(m.clone(), &new);
        self.version += 1;
        self.job_cache = None;
        self.find.refresh(&self.text, self.version);
        // continue with the first match after the replaced text
        let after = m.start + new.len();
        let next = self.find.matches.iter().position(|r| r.start >= after).or(if self.find.matches.is_empty() { None } else { Some(0) });
        self.find.current = None;
        match next {
            Some(n) => self.go_to_match(n),
            None => self.pending_cursor = Some((after, None)),
        }
    }

    pub fn replace_all(&mut self) -> usize {
        self.find.refresh(&self.text, self.version);
        let Ok(Some(re)) = self.find.regex() else { return 0 };
        let n = self.find.matches.len();
        if n == 0 {
            return 0;
        }
        let new = if self.find.regex { re.replace_all(&self.text, self.find.replace.as_str()).into_owned() } else { re.replace_all(&self.text, regex::NoExpand(&self.find.replace)).into_owned() };
        let cursor = self.cursor.min(new.len());
        self.text = new;
        self.version += 1;
        self.job_cache = None;
        self.find.current = None;
        self.pending_cursor = Some((cursor, None));
        n
    }

    /// The find bar row. Returns true when the editor should take focus back.
    fn find_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        let qid = Find::query_id(self.id);
        let rid = self.id.with("find-replace");
        let focused = ctx.memory(|m| m.focused() == Some(qid) || m.focused() == Some(rid));
        let mut step: Option<bool> = None;
        let mut close = false;
        let mut replace_one = false;
        let mut replace_all = false;
        if focused {
            ui.input_mut(|i| {
                if i.consume_key(Modifiers::SHIFT, Key::Enter) {
                    step = Some(false);
                } else if i.consume_key(Modifiers::NONE, Key::Enter) {
                    if ctx.memory(|m| m.focused() == Some(rid)) {
                        replace_one = true;
                    } else {
                        step = Some(true);
                    }
                }
                if i.consume_key(Modifiers::COMMAND | Modifiers::ALT, Key::Enter) {
                    replace_all = true;
                }
                if i.consume_key(Modifiers::NONE, Key::Escape) {
                    close = true;
                }
                if i.consume_key(Modifiers::ALT, Key::C) {
                    self.find.case = !self.find.case;
                }
                if i.consume_key(Modifiers::ALT, Key::W) {
                    self.find.word = !self.find.word;
                }
                if i.consume_key(Modifiers::ALT, Key::R) {
                    self.find.regex = !self.find.regex;
                }
            });
        }
        let toggle = |ui: &mut egui::Ui, on: &mut bool, label: &str, tip: &str| {
            let rt = RichText::new(label).font(FontId::monospace(11.5)).color(if *on { p.text } else { p.muted });
            let b = egui::Button::new(rt).fill(if *on { p.selection } else { Color32::TRANSPARENT }).stroke(if *on { Stroke::new(1.0_f32, p.accent) } else { Stroke::NONE }).min_size(egui::vec2(22.0, 20.0));
            if ui.add(b).on_hover_text(tip).clicked() {
                *on = !*on;
            }
        };
        egui::Frame::new().fill(p.raised).stroke(Stroke::new(1.0_f32, p.border)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(6, 4)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                let arrow = if self.find.show_replace { "▼" } else { "▶" };
                if ui.add(egui::Button::new(RichText::new(arrow).color(p.muted)).frame(false)).on_hover_text("Toggle replace  (Ctrl + H)").clicked() {
                    self.find.show_replace = !self.find.show_replace;
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        let te = egui::TextEdit::singleline(&mut self.find.query).id(qid).hint_text("Find").desired_width(220.0).font(FontId::monospace(12.0));
                        let r = ui.add(te);
                        if self.find.focus_query {
                            r.request_focus();
                            let mut st = egui::TextEdit::load_state(&ctx, qid).unwrap_or_default();
                            let n = self.find.query.chars().count();
                            st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(n))));
                            st.store(&ctx, qid);
                            self.find.focus_query = false;
                        }
                        if r.changed() {
                            self.find.current = None;
                            self.find.refresh(&self.text, self.version);
                            // jump as you type, like VS Code
                            if !self.find.matches.is_empty() {
                                let at = self.selection.as_ref().map(|r| r.start).unwrap_or(self.cursor);
                                let i = self.find.matches.iter().position(|m| m.end >= at).unwrap_or(0);
                                self.go_to_match(i);
                            }
                        }
                        toggle(ui, &mut self.find.case, "Aa", "Match case  (Alt + C)");
                        toggle(ui, &mut self.find.word, "ab", "Match whole word  (Alt + W)");
                        toggle(ui, &mut self.find.regex, ".*", "Use regular expression  (Alt + R)");
                        self.find.refresh(&self.text, self.version);
                        let n = self.find.matches.len();
                        let (status, color) = if let Some(e) = &self.find.error {
                            (e.chars().take(40).collect::<String>(), p.danger)
                        } else if self.find.query.is_empty() {
                            (String::new(), p.muted)
                        } else if n == 0 {
                            ("No results".to_string(), p.danger)
                        } else {
                            let cur = self.find.current.map(|c| (c + 1).to_string()).unwrap_or_else(|| "?".into());
                            (format!("{cur} of {n}{}", if n >= 20_000 { "+" } else { "" }), p.muted)
                        };
                        ui.add_sized([74.0, 20.0], egui::Label::new(RichText::new(status).size(11.5).color(color)));
                        if ui.add_enabled(n > 0, egui::Button::new("↑").frame(false)).on_hover_text("Previous match  (Shift + Enter)").clicked() {
                            step = Some(false);
                        }
                        if ui.add_enabled(n > 0, egui::Button::new("↓").frame(false)).on_hover_text("Next match  (Enter)").clicked() {
                            step = Some(true);
                        }
                        if ui.add_enabled(!self.find.query.is_empty(), egui::Button::new(RichText::new("Search DB").size(11.5).color(p.accent)).frame(false)).on_hover_text("Search the whole database for this text  (Ctrl/⌘ + F again)").clicked() {
                            self.search_database = Some(self.find.query.clone());
                        }
                        if ui.add(egui::Button::new(RichText::new("×").size(14.0).color(p.muted)).frame(false)).on_hover_text("Close  (Esc)").clicked() {
                            close = true;
                        }
                    });
                    if self.find.show_replace {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;
                            ui.add(egui::TextEdit::singleline(&mut self.find.replace).id(rid).hint_text(if self.find.regex { "Replace ($1 for groups)" } else { "Replace" }).desired_width(220.0).font(FontId::monospace(12.0)));
                            let n = self.find.matches.len();
                            if ui.add_enabled(n > 0, egui::Button::new("Replace")).on_hover_text("Replace this match  (Enter in the replace field)").clicked() {
                                replace_one = true;
                            }
                            if ui.add_enabled(n > 0, egui::Button::new("All")).on_hover_text("Replace all  (Ctrl/⌘ + Alt + Enter)").clicked() {
                                replace_all = true;
                            }
                        });
                    }
                });
            });
        });
        if let Some(f) = step {
            self.find_step(f);
        }
        if replace_one {
            self.replace_current();
        }
        if replace_all {
            self.replace_all();
        }
        if close {
            self.close_find();
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
            // the theme: without it, switching themes kept the old colors
            // (light grey identifiers on the light background)
            crate::theme::fingerprint(p).hash(&mut h);
            font.size.to_bits().hash(&mut h);
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
        if self.find.open && (has_focus || self.find_focused(ui.ctx())) {
            let (next, prev, esc) = ui.input_mut(|i| {
                let prev = i.consume_key(Modifiers::SHIFT, Key::F3) || i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::G);
                let next = i.consume_key(Modifiers::NONE, Key::F3) || i.consume_key(Modifiers::COMMAND, Key::G);
                let esc = has_focus && self.popup.is_none() && i.consume_key(Modifiers::NONE, Key::Escape);
                (next, prev, esc)
            });
            if next {
                self.find_step(true);
            }
            if prev {
                self.find_step(false);
            }
            if esc {
                self.close_find();
            }
        }
        if self.find.open {
            ui.horizontal(|ui| self.find_bar(ui, p));
            ui.add_space(2.0);
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
            if !(self.find.open && self.find_focused(ui.ctx())) {
                self.wants_focus = true;
            }
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
                    if self.find.open {
                        let cur = self.find.current.and_then(|i| self.find.matches.get(i)).cloned();
                        let top = ui.clip_rect().top() - origin.y - 40.0;
                        let bottom = ui.clip_rect().bottom() - origin.y + 40.0;
                        for m in &self.find.matches {
                            let a = pos_of(&g, byte_to_char(&self.text, m.start));
                            if a.max.y < top {
                                continue;
                            }
                            if a.min.y > bottom {
                                break;
                            }
                            let b = pos_of(&g, byte_to_char(&self.text, m.end));
                            let is_cur = cur.as_ref() == Some(m);
                            let fill = if is_cur { p.warning.gamma_multiply(0.55) } else { p.warning.gamma_multiply(0.22) };
                            let r = if (a.min.y - b.min.y).abs() < 1.0 {
                                Rect::from_min_max(egui::pos2(a.min.x, a.min.y), egui::pos2(b.max.x.max(a.min.x + 2.0), a.max.y))
                            } else {
                                Rect::from_min_max(a.min, egui::pos2(g.rect.right(), a.max.y))
                            }
                            .translate(origin.to_vec2());
                            ui.painter().rect_filled(r, CornerRadius::same(2), fill);
                            if is_cur {
                                ui.painter().rect_stroke(r, CornerRadius::same(2), Stroke::new(1.0_f32, p.warning), egui::StrokeKind::Outside);
                            }
                        }
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

            if let Some(m) = self.find.scroll_to.take() {
                let a = pos_of(g, byte_to_char(&self.text, m.start.min(self.text.len())));
                ui.scroll_to_rect(a.translate(origin.to_vec2()).expand2(egui::vec2(40.0, 30.0)), Some(egui::Align::Center));
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
