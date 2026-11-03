//! The rename refactoring dialog (Shift+F6 in the editor, or Rename… in
//! the explorer).

use crate::theme::{bold, Palette};
use databug_sql::rename::RenameTarget;
use eframe::egui::{self, Color32, RichText};

pub struct RenameDialog {
    pub conn_id: String,
    pub schema: String,
    pub target: RenameTarget,
    pub new_name: String,
    /// Console tab the rename started from, if any.
    pub from_tab: Option<u64>,
    pub update_console: bool,
    pub rename_in_db: bool,
    pub update_dependents: bool,
    focused: bool,
}

pub enum RenameOutcome {
    Open,
    Cancel,
    Apply,
}

impl RenameDialog {
    pub fn new(conn_id: String, schema: String, target: RenameTarget, from_tab: Option<u64>) -> Self {
        let is_alias = matches!(target, RenameTarget::Alias { .. });
        RenameDialog {
            conn_id,
            schema,
            new_name: target.current_name().to_string(),
            target,
            from_tab,
            update_console: from_tab.is_some(),
            rename_in_db: !is_alias,
            update_dependents: !is_alias,
            focused: false,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, p: &Palette) -> RenameOutcome {
        let mut outcome = RenameOutcome::Open;
        let mut open = true;
        egui::Window::new(RichText::new(format!("Rename {}", self.target.describe())).font(bold(15.0)))
            .id(egui::Id::new("rename"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .fixed_size([460.0, 0.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -60.0])
            .show(ctx, |ui| {
                let resp = ui.add(egui::TextEdit::singleline(&mut self.new_name).desired_width(f32::INFINITY));
                if !self.focused {
                    resp.request_focus();
                    self.focused = true;
                }
                ui.add_space(6.0);
                match self.target {
                    RenameTarget::Alias { .. } => {
                        ui.label(RichText::new("Renames the alias everywhere in this statement.").color(p.muted));
                    }
                    _ => {
                        if self.from_tab.is_some() {
                            ui.checkbox(&mut self.update_console, "Update references in this console");
                        }
                        ui.checkbox(&mut self.rename_in_db, "Rename in the database");
                        ui.add_enabled(self.rename_in_db, egui::Checkbox::new(&mut self.update_dependents, "Update views, procedures and functions that use it"));
                        ui.label(RichText::new("The database changes open in a new console for you to review and run.").size(12.0).color(p.faint));
                    }
                }
                ui.add_space(6.0);
                let valid = !self.new_name.trim().is_empty() && self.new_name.trim() != self.target.current_name();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(valid, egui::Button::new(RichText::new("Rename").color(Color32::WHITE)).fill(p.accent)).clicked() || (valid && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                            outcome = RenameOutcome::Apply;
                        }
                        if ui.button("Cancel").clicked() {
                            outcome = RenameOutcome::Cancel;
                        }
                    });
                });
            });
        if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            outcome = RenameOutcome::Cancel;
        }
        outcome
    }
}
