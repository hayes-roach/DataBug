//! Document view for document databases (CouchDB, MongoDB): a list of the
//! result's documents and, for the selected one, every field as a tree (or
//! raw JSON) plus its attachments with links, previews and downloads.

use crate::grid::ResultView;
use crate::theme::Palette;
use eframe::egui::{self, Color32, CornerRadius, FontId, Margin, RichText, Stroke};
use serde_json::Value;
use std::collections::HashMap;

/// Images larger than this aren't previewed automatically.
const PREVIEW_MAX_BYTES: u64 = 25 * 1024 * 1024;

pub struct DocView {
    /// Showing documents (instead of the table).
    pub showing: bool,
    /// The database (CouchDB) or collection (MongoDB) the result came from.
    pub collection: Option<String>,
    /// The engine stores attachments (CouchDB).
    pub attachments: bool,
    selected: usize,
    filter: String,
    raw: bool,
    pub previews: HashMap<String, Preview>,
    /// Documents parsed from the result's rows, rebuilt when rows arrive.
    docs: Vec<Value>,
    parsed_rows: usize,
    pending_select: Option<String>,
    /// The statement that produced the result, re-run after an edit or delete.
    pub source: String,
    /// The selected document's JSON while it's being edited.
    edit: Option<String>,
    edit_error: Option<String>,
    confirm_delete: bool,
}

pub enum Preview {
    Loading,
    Bytes(Vec<u8>),
    Image(egui::TextureHandle),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Purpose {
    /// Show an image inline; the key is in `DocView::previews`.
    Preview(String),
    /// Save to a temporary file and open it with the system's viewer.
    Open,
    /// Save where the user chose.
    Save(std::path::PathBuf),
}

pub enum DocAction {
    Fetch { doc_id: String, name: String, purpose: Purpose },
    Copy(String),
    /// Run these statements (an update or delete of one document), then the
    /// result's own query again, selecting `reselect` in the new result.
    Run { stmts: Vec<String>, reselect: Option<String> },
}

/// `db.orders` for a plain name, `db.getCollection("…")` otherwise.
fn collection_ref(name: &str) -> String {
    let plain = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain { format!("db.{name}") } else { format!("db.getCollection({})", Value::String(name.into())) }
}

/// A filter matching exactly this document: its `_id` (as an ObjectId when
/// MongoDB stored one) and, for CouchDB, its `_rev`, so a document changed by
/// someone else in the meantime isn't overwritten.
pub fn document_filter(doc: &Value, couch: bool, object_id: bool) -> Option<String> {
    let id = doc.get("_id")?;
    if couch {
        let rev = doc.get("_rev")?;
        return Some(format!("{{\"_id\": {id}, \"_rev\": {rev}}}"));
    }
    match (object_id, id.as_str()) {
        (true, Some(hex)) if hex.len() == 24 && hex.chars().all(|c| c.is_ascii_hexdigit()) => Some(format!("{{_id: ObjectId(\"{hex}\")}}")),
        _ => Some(format!("{{\"_id\": {id}}}")),
    }
}

/// The top-level changes from `old` to `new` as an update document
/// (`$set` for new and changed fields, `$unset` for removed ones), or `None`
/// when nothing changed. `_id`, `_rev` and `_attachments` are left alone.
pub fn update_document(old: &Value, new: &Value) -> Result<Option<Value>, String> {
    let (Some(o), Some(n)) = (old.as_object(), new.as_object()) else { return Err("The document must be a JSON object".into()) };
    let keep = |k: &str| !matches!(k, "_id" | "_rev" | "_attachments");
    if n.get("_id").is_some_and(|id| Some(id) != o.get("_id")) {
        return Err("_id can't be changed".into());
    }
    let mut set = serde_json::Map::new();
    let mut unset = serde_json::Map::new();
    for (k, v) in n.iter().filter(|(k, _)| keep(k)) {
        if o.get(k) != Some(v) {
            set.insert(k.clone(), v.clone());
        }
    }
    for k in o.keys().filter(|k| keep(k)) {
        if !n.contains_key(k) {
            unset.insert(k.clone(), Value::String(String::new()));
        }
    }
    if set.is_empty() && unset.is_empty() {
        return Ok(None);
    }
    let mut u = serde_json::Map::new();
    if !set.is_empty() {
        u.insert("$set".into(), Value::Object(set));
    }
    if !unset.is_empty() {
        u.insert("$unset".into(), Value::Object(unset));
    }
    Ok(Some(Value::Object(u)))
}

impl DocView {
    /// Select the document with this `_id` (screenshot scenes).
    pub fn select_id(&mut self, id: &str) {
        self.pending_select = Some(id.to_string());
    }

    /// Open the selected document for editing (screenshot scenes).
    pub fn begin_edit(&mut self) {
        if let Some(d) = self.docs.get(self.selected) {
            self.edit = Some(serde_json::to_string_pretty(d).unwrap_or_default());
        }
    }

    pub fn new(collection: Option<String>, attachments: bool, showing: bool) -> Self {
        DocView { showing, collection, attachments, selected: 0, filter: String::new(), raw: false, previews: HashMap::new(), docs: Vec::new(), parsed_rows: 0, pending_select: None, source: String::new(), edit: None, edit_error: None, confirm_delete: false }
    }
}

/// A row of the result as the document it came from. Cells hold strings as
/// text and everything else as JSON, so non-string columns are parsed back.
pub fn row_document(view: &ResultView, row: usize) -> Value {
    let mut doc = serde_json::Map::new();
    for (c, col) in view.columns.iter().enumerate() {
        let Some(text) = view.data.cell(row, c) else { continue };
        let v = if col.type_name == "string" { Value::String(text.to_string()) } else { serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string())) };
        doc.insert(col.name.clone(), v);
    }
    Value::Object(doc)
}

/// The key for an attachment's preview: document, revision and name.
pub fn preview_key(doc: &Value, name: &str) -> String {
    format!("{}\u{1}{}\u{1}{name}", text(doc.get("_id")), text(doc.get("_rev")))
}

fn text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

pub fn human_size(n: u64) -> String {
    match n {
        n if n >= 1024 * 1024 * 1024 => format!("{:.1} GB", n as f64 / 1024.0 / 1024.0 / 1024.0),
        n if n >= 1024 * 1024 => format!("{:.1} MB", n as f64 / 1024.0 / 1024.0),
        n if n >= 1024 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{n} bytes"),
    }
}

/// A one-line description for the list: the first few short string fields.
fn summary(doc: &Value) -> String {
    let Some(o) = doc.as_object() else { return String::new() };
    o.iter().filter(|(k, _)| !k.starts_with('_')).filter_map(|(_, v)| v.as_str()).filter(|s| s.len() <= 40).take(3).collect::<Vec<_>>().join(" · ")
}

pub fn show(ui: &mut egui::Ui, p: &Palette, view: &mut ResultView, url_for: &dyn Fn(&str, &str) -> Option<String>) -> Vec<DocAction> {
    let mut actions = Vec::new();
    let rows = view.data.rows();
    let Some(dv) = view.docs.as_mut() else { return actions };
    if dv.parsed_rows != rows {
        // rows stream in; parse only the new ones
        let start = dv.parsed_rows.min(rows);
        dv.docs.truncate(start);
        let new: Vec<Value> = (start..rows).map(|r| row_document(view, r)).collect();
        let dv = view.docs.as_mut().unwrap();
        dv.docs.extend(new);
        dv.parsed_rows = rows;
    }
    let dv = view.docs.as_mut().unwrap();
    if dv.docs.is_empty() {
        ui.add_space(12.0);
        ui.label(RichText::new("No documents").color(p.faint));
        return actions;
    }
    if let Some(want) = dv.pending_select.take() {
        if let Some(i) = dv.docs.iter().position(|d| d.get("_id").and_then(|v| v.as_str()) == Some(want.as_str())) {
            dv.selected = i;
        }
    }
    dv.selected = dv.selected.min(dv.docs.len() - 1);
    let object_id = view.columns.iter().any(|c| c.name == "_id" && c.type_name == "objectId");
    let dv = view.docs.as_mut().unwrap();
    let id = ui.id().with("docview");
    egui::SidePanel::left(id.with("list")).resizable(true).default_width(260.0).width_range(160.0..=520.0).frame(egui::Frame::new().inner_margin(Margin { left: 0, right: 8, top: 6, bottom: 0 })).show_inside(ui, |ui| {
        crate::theme::text_field(ui, p, egui::TextEdit::singleline(&mut dv.filter).hint_text("Filter documents…"), ui.available_width(), crate::theme::FIELD_H_COMPACT);
        ui.add_space(4.0);
        let f = dv.filter.to_lowercase();
        let shown: Vec<usize> = (0..dv.docs.len()).filter(|&i| f.is_empty() || dv.docs[i].to_string().to_lowercase().contains(&f)).collect();
        ui.label(RichText::new(if shown.len() == dv.docs.len() { format!("{} documents", dv.docs.len()) } else { format!("{} of {} documents", shown.len(), dv.docs.len()) }).size(11.0).color(p.faint));
        ui.add_space(2.0);
        egui::ScrollArea::vertical().drag_to_scroll(false).id_salt(id.with("list-scroll")).auto_shrink([false, false]).show_rows(ui, 38.0, shown.len(), |ui, range| {
            for &i in &shown[range] {
                let doc = &dv.docs[i];
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 38.0), egui::Sense::click());
                let selected = i == dv.selected;
                if selected {
                    ui.painter().rect_filled(rect, CornerRadius::same(5), p.tree_selected);
                    ui.painter().rect_filled(egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 5.0), egui::vec2(2.5, 28.0)), CornerRadius::same(1), p.accent);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(5), p.hover);
                }
                let painter = ui.painter_at(rect.shrink2(egui::vec2(8.0, 0.0)));
                let id_text = match doc.get("_id") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => format!("Document {}", i + 1),
                };
                let clips = doc.get("_attachments").and_then(|a| a.as_object()).map(|a| a.len()).unwrap_or(0);
                let right = if clips > 0 { format!("📎 {clips}") } else { String::new() };
                painter.text(egui::pos2(rect.left() + 10.0, rect.top() + 11.0), egui::Align2::LEFT_CENTER, &id_text, FontId::monospace(12.0), if selected { p.text } else { p.text.gamma_multiply(0.92) });
                if !right.is_empty() {
                    painter.text(egui::pos2(rect.right() - 10.0, rect.top() + 11.0), egui::Align2::RIGHT_CENTER, right, FontId::proportional(11.0), p.muted);
                }
                painter.text(egui::pos2(rect.left() + 10.0, rect.top() + 27.0), egui::Align2::LEFT_CENTER, summary(doc), FontId::proportional(11.0), p.faint);
                if resp.clicked() && dv.selected != i {
                    dv.selected = i;
                    dv.edit = None;
                    dv.edit_error = None;
                    dv.confirm_delete = false;
                }
            }
        });
    });
    egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(Margin { left: 12, right: 4, top: 6, bottom: 0 })).show_inside(ui, |ui| {
        let doc = dv.docs[dv.selected].clone();
        let doc_id = text(doc.get("_id"));
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(RichText::new(if doc_id.is_empty() { "(no _id)" } else { &doc_id }).font(FontId::monospace(14.0)).strong().color(p.text)).selectable(true));
            if let Some(rev) = doc.get("_rev").and_then(|r| r.as_str()) {
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new(format!("rev {rev}")).font(FontId::monospace(11.0)).color(p.muted)).selectable(true));
            }
            let couch = dv.attachments;
            let filter = document_filter(&doc, couch, object_id);
            let target = dv.collection.as_deref().map(collection_ref);
            let editable = filter.is_some() && target.is_some() && !dv.source.is_empty();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if dv.edit.is_some() {
                    // editing: save the changes as one update, or drop them
                    if ui.add(egui::Button::new(RichText::new("Save").color(Color32::WHITE)).fill(p.accent)).clicked() {
                        let text = dv.edit.clone().unwrap_or_default();
                        match serde_json::from_str::<Value>(&text).map_err(|e| format!("Not valid JSON: {e}")).and_then(|new| update_document(&doc, &new)) {
                            Ok(None) => {
                                dv.edit = None;
                                dv.edit_error = None;
                            }
                            Ok(Some(update)) => {
                                if let (Some(t), Some(f)) = (&target, &filter) {
                                    actions.push(DocAction::Run { stmts: vec![format!("{t}.updateOne({f}, {update})")], reselect: Some(doc_id.clone()) });
                                }
                                dv.edit = None;
                                dv.edit_error = None;
                            }
                            Err(e) => dv.edit_error = Some(e),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        dv.edit = None;
                        dv.edit_error = None;
                    }
                    return;
                }
                if dv.confirm_delete {
                    if ui.add(egui::Button::new(RichText::new("Delete").color(Color32::WHITE)).fill(p.danger)).clicked() {
                        if let (Some(t), Some(f)) = (&target, &filter) {
                            actions.push(DocAction::Run { stmts: vec![format!("{t}.deleteOne({f})")], reselect: None });
                        }
                        dv.confirm_delete = false;
                    }
                    if ui.button("Cancel").clicked() {
                        dv.confirm_delete = false;
                    }
                    ui.label(RichText::new("Delete this document?").color(p.danger));
                    return;
                }
                if ui.button("Copy JSON").clicked() {
                    actions.push(DocAction::Copy(serde_json::to_string_pretty(&doc).unwrap_or_default()));
                }
                if ui.selectable_label(dv.raw, "JSON").clicked() {
                    dv.raw = true;
                }
                if ui.selectable_label(!dv.raw, "Tree").clicked() {
                    dv.raw = false;
                }
                ui.add_space(8.0);
                let why = "Needs a result from a find() on one collection, with documents that have an _id";
                if ui.add_enabled(editable, egui::Button::new(RichText::new("Delete").color(p.danger))).on_disabled_hover_text(why).clicked() {
                    dv.confirm_delete = true;
                }
                if ui.add_enabled(editable, egui::Button::new("Edit")).on_hover_text("Edit the document as JSON; saving sends only what changed").on_disabled_hover_text(why).clicked() {
                    dv.edit = Some(serde_json::to_string_pretty(&doc).unwrap_or_default());
                    dv.edit_error = None;
                }
            });
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical().drag_to_scroll(false).id_salt(id.with(("detail", dv.selected))).auto_shrink([false, false]).show(ui, |ui| {
            // while editing, the JSON comes first; attachments return after saving
            if let Some(atts) = doc.get("_attachments").and_then(|a| a.as_object()).filter(|a| !a.is_empty() && dv.edit.is_none()) {
                section(ui, p, &format!("Attachments  {}", atts.len()));
                for (name, meta) in atts {
                    attachment_card(ui, p, dv, &doc, &doc_id, name, meta, url_for, &mut actions);
                }
                ui.add_space(8.0);
            }
            section(ui, p, "Fields");
            if let Some(text) = dv.edit.as_mut() {
                if let Some(e) = &dv.edit_error {
                    ui.label(RichText::new(e).color(p.danger));
                }
                ui.add(egui::TextEdit::multiline(text).font(FontId::monospace(12.5)).desired_width(f32::INFINITY).desired_rows(16).code_editor());
                ui.label(RichText::new("_id, _rev and _attachments stay as they are. Fields you remove are unset.").size(11.0).color(p.faint));
            } else if dv.raw {
                let pretty = serde_json::to_string_pretty(&doc).unwrap_or_default();
                ui.add(egui::TextEdit::multiline(&mut pretty.as_str()).font(FontId::monospace(12.5)).desired_width(f32::INFINITY).frame(false));
            } else if let Some(o) = doc.as_object() {
                for (k, v) in o {
                    tree(ui, p, k, v, &format!("{doc_id}/{k}"), 0, &mut actions);
                }
            }
        });
    });
    actions
}

fn section(ui: &mut egui::Ui, p: &Palette, title: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(title).size(11.5).strong().color(p.muted));
    ui.add_space(2.0);
}

#[allow(clippy::too_many_arguments)]
fn attachment_card(ui: &mut egui::Ui, p: &Palette, dv: &mut DocView, doc: &Value, doc_id: &str, name: &str, meta: &Value, url_for: &dyn Fn(&str, &str) -> Option<String>, actions: &mut Vec<DocAction>) {
    let ctype = meta.get("content_type").and_then(|v| v.as_str()).unwrap_or("application/octet-stream").to_string();
    let length = meta.get("length").and_then(|v| v.as_u64());
    let mut facts = vec![ctype.clone()];
    if let Some(n) = length {
        facts.push(human_size(n));
    }
    if let Some(r) = meta.get("revpos").and_then(|v| v.as_u64()) {
        facts.push(format!("added in rev {r}"));
    }
    if let Some(e) = meta.get("encoding").and_then(|v| v.as_str()) {
        facts.push(format!("stored {e}"));
    }
    let url = url_for(doc_id, name);
    let image = ctype.starts_with("image/") && !ctype.contains("svg");
    egui::Frame::new().fill(p.raised).stroke(Stroke::new(1.0_f32, p.border)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(RichText::new(format!("📎 {name}")).strong().color(p.text)).selectable(true));
            ui.label(RichText::new(facts.join(" · ")).size(11.5).color(p.muted));
        });
        if let Some(d) = meta.get("digest").and_then(|v| v.as_str()) {
            ui.add(egui::Label::new(RichText::new(format!("digest {d}")).font(FontId::monospace(11.0)).color(p.faint)).selectable(true));
        }
        if let Some(u) = &url {
            ui.add(egui::Label::new(RichText::new(u).font(FontId::monospace(11.5)).color(p.accent)).selectable(true)).on_hover_text("Needs the connection's user name and password");
        }
        ui.add_space(4.0);
        let can_fetch = dv.attachments && dv.collection.is_some() && !doc_id.is_empty();
        ui.horizontal(|ui| {
            if ui.add_enabled(can_fetch, egui::Button::new("Open")).on_hover_text("Download and open it with the system's default app").clicked() {
                actions.push(DocAction::Fetch { doc_id: doc_id.to_string(), name: name.to_string(), purpose: Purpose::Open });
            }
            if ui.add_enabled(can_fetch, egui::Button::new("Save…")).clicked() {
                if let Some(path) = rfd::FileDialog::new().set_file_name(name).save_file() {
                    actions.push(DocAction::Fetch { doc_id: doc_id.to_string(), name: name.to_string(), purpose: Purpose::Save(path) });
                }
            }
            if let Some(u) = &url {
                if ui.button("Copy link").clicked() {
                    actions.push(DocAction::Copy(u.clone()));
                }
            }
        });
        if image && can_fetch {
            let key = preview_key(doc, name);
            let small = length.is_none_or(|n| n <= PREVIEW_MAX_BYTES);
            // small images load on their own; big ones wait for a click
            let fetch = match dv.previews.get(&key) {
                None if small => true,
                None => ui.button("Preview").clicked(),
                Some(_) => false,
            };
            if fetch {
                dv.previews.insert(key.clone(), Preview::Loading);
                actions.push(DocAction::Fetch { doc_id: doc_id.to_string(), name: name.to_string(), purpose: Purpose::Preview(key.clone()) });
            }
            if let Some(Preview::Bytes(bytes)) = dv.previews.get(&key) {
                let decoded = image::load_from_memory(bytes).map_err(|e| format!("Can't preview: {e}")).map(|img| {
                    let rgba = img.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    ui.ctx().load_texture(format!("attachment-{key}"), egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()), egui::TextureOptions::LINEAR)
                });
                dv.previews.insert(key.clone(), match decoded {
                    Ok(t) => Preview::Image(t),
                    Err(e) => Preview::Failed(e),
                });
            }
            match dv.previews.get(&key) {
                Some(Preview::Loading) => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(12.0));
                        ui.label(RichText::new("Loading preview…").size(11.5).color(p.muted));
                    });
                }
                Some(Preview::Image(t)) => {
                    ui.add_space(6.0);
                    let s = t.size_vec2();
                    let scale = (ui.available_width() / s.x).min(200.0 / s.y).min(1.0);
                    let resp = ui.add(egui::Image::new(t).fit_to_exact_size(s * scale).corner_radius(CornerRadius::same(4)).sense(egui::Sense::click()));
                    ui.label(RichText::new(format!("{} × {} px", s.x as u32, s.y as u32)).size(11.0).color(p.faint));
                    if resp.on_hover_text("Click to open full size").clicked() {
                        actions.push(DocAction::Fetch { doc_id: doc_id.to_string(), name: name.to_string(), purpose: Purpose::Open });
                    }
                }
                Some(Preview::Failed(e)) => {
                    ui.label(RichText::new(e).size(11.5).color(p.danger));
                }
                _ => {}
            }
        }
    });
    ui.add_space(6.0);
}

fn value_color(p: &Palette, v: &Value) -> Color32 {
    match v {
        Value::String(_) => p.syn_string,
        Value::Number(_) => p.syn_number,
        Value::Bool(_) => p.syn_keyword,
        Value::Null => p.faint,
        _ => p.text,
    }
}

/// One field: objects and arrays as collapsible nodes, values as text.
fn tree(ui: &mut egui::Ui, p: &Palette, key: &str, v: &Value, path: &str, depth: usize, actions: &mut Vec<DocAction>) {
    let key_text = RichText::new(key).font(FontId::monospace(12.5)).color(p.syn_ident);
    match v {
        Value::Object(o) => {
            let header = egui::CollapsingHeader::new(RichText::new(format!("{key}  {{{}}}", o.len())).font(FontId::monospace(12.5)).color(p.syn_ident)).id_salt(path).default_open(depth < 2);
            let resp = header.show(ui, |ui| {
                for (k, c) in o {
                    tree(ui, p, k, c, &format!("{path}/{k}"), depth + 1, actions);
                }
            });
            copy_menu(&resp.header_response, v, actions);
        }
        Value::Array(a) => {
            let header = egui::CollapsingHeader::new(RichText::new(format!("{key}  [{}]", a.len())).font(FontId::monospace(12.5)).color(p.syn_ident)).id_salt(path).default_open(depth < 2 && a.len() <= 20);
            let resp = header.show(ui, |ui| {
                for (i, c) in a.iter().enumerate() {
                    tree(ui, p, &i.to_string(), c, &format!("{path}/{i}"), depth + 1, actions);
                }
            });
            copy_menu(&resp.header_response, v, actions);
        }
        scalar => {
            let shown = match scalar {
                Value::String(s) => format!("\"{s}\""),
                other => other.to_string(),
            };
            let row = ui.horizontal_wrapped(|ui| {
                ui.add_space(18.0);
                ui.label(key_text);
                ui.label(RichText::new(":").color(p.faint));
                ui.add(egui::Label::new(RichText::new(shown).font(FontId::monospace(12.5)).color(value_color(p, scalar))).selectable(true));
            });
            copy_menu(&row.response, scalar, actions);
        }
    }
}

fn copy_menu(resp: &egui::Response, v: &Value, actions: &mut Vec<DocAction>) {
    resp.context_menu(|ui| {
        if ui.button("Copy value").clicked() {
            actions.push(DocAction::Copy(match v {
                Value::String(s) => s.clone(),
                other => serde_json::to_string_pretty(other).unwrap_or_default(),
            }));
            ui.close_menu();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use databug_core::{ColumnMeta, Dialect};

    #[test]
    fn rows_become_documents_again() {
        let cols = vec![ColumnMeta::new("_id", "string"), ColumnMeta::new("n", "integer"), ColumnMeta::new("tags", "array"), ColumnMeta::new("_attachments", "object"), ColumnMeta::new("note", "string")];
        let mut v = ResultView::new("r".into(), cols, Dialect::Mongo, 1);
        for cell in [Some("photo:1"), Some("5"), Some(r#"["a","b"]"#), Some(r#"{"x.png":{"content_type":"image/png","length":10,"stub":true}}"#), None] {
            v.data.push(cell);
        }
        let d = row_document(&v, 0);
        assert_eq!(d["_id"], "photo:1");
        assert_eq!(d["n"], 5);
        assert_eq!(d["tags"][1], "b");
        assert_eq!(d["_attachments"]["x.png"]["length"], 10);
        assert!(d.get("note").is_none());
        assert_eq!(preview_key(&d, "x.png"), "photo:1\u{1}\u{1}x.png");
    }

    #[test]
    fn edit_filters_and_changes() {
        use serde_json::json;
        let couch = json!({"_id": "order:1", "_rev": "3-abc", "n": 1});
        assert_eq!(document_filter(&couch, true, false).unwrap(), r#"{"_id": "order:1", "_rev": "3-abc"}"#);
        let mongo = json!({"_id": "65a1b2c3d4e5f60718293a4b"});
        assert_eq!(document_filter(&mongo, false, true).unwrap(), r#"{_id: ObjectId("65a1b2c3d4e5f60718293a4b")}"#);
        assert_eq!(document_filter(&json!({"_id": 7}), false, false).unwrap(), r#"{"_id": 7}"#);
        assert!(document_filter(&json!({"n": 1}), false, false).is_none());
        assert_eq!(collection_ref("orders"), "db.orders");
        assert_eq!(collection_ref("my-orders"), r#"db.getCollection("my-orders")"#);

        let old = json!({"_id": "a", "_rev": "1-x", "_attachments": {}, "keep": 1, "change": "a", "drop": true});
        let new = json!({"_id": "a", "keep": 1, "change": "b", "add": [1]});
        assert_eq!(update_document(&old, &new).unwrap().unwrap(), json!({"$set": {"change": "b", "add": [1]}, "$unset": {"drop": ""}}));
        assert!(update_document(&old, &old).unwrap().is_none());
        assert!(update_document(&old, &json!({"_id": "b"})).is_err());
        assert!(update_document(&old, &json!([1])).is_err());
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(900), "900 bytes");
        assert_eq!(human_size(3860), "3.8 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }
}
