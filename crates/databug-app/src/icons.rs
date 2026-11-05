//! Painted icons. Engine marks are DataBug's own monogram badges; if you
//! put official logo files in a `logos` folder they're used instead (see
//! `Logos::discover`).

use crate::theme::Palette;
use databug_core::{DbKind, ObjectKind};
use eframe::egui::{self, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind};
use std::collections::HashMap;
use std::path::PathBuf;

pub fn engine_color(kind: DbKind) -> Color32 {
    match kind {
        DbKind::Postgres => Color32::from_rgb(0x4C, 0x7E, 0xC9),
        DbKind::MySql => Color32::from_rgb(0xD9, 0x8E, 0x2E),
        DbKind::MsSql => Color32::from_rgb(0xC9, 0x4A, 0x4A),
        DbKind::Sqlite => Color32::from_rgb(0x3E, 0x9D, 0xB3),
        DbKind::MongoDb => Color32::from_rgb(0x4E, 0xA9, 0x5C),
        DbKind::Oracle => Color32::from_rgb(0xC7, 0x46, 0x34),
        DbKind::Db2 => Color32::from_rgb(0x1F, 0x70, 0xC1),
        DbKind::DuckDb => Color32::from_rgb(0xC9, 0x9A, 0x06),
        DbKind::Cassandra => Color32::from_rgb(0x2B, 0x6C, 0x8F),
        DbKind::CouchDb => Color32::from_rgb(0xE4, 0x25, 0x28),
        DbKind::CockroachDb => Color32::from_rgb(0x69, 0x33, 0xFF),
        DbKind::TimescaleDb => Color32::from_rgb(0x1F, 0x9E, 0x89),
        DbKind::QuestDb => Color32::from_rgb(0xD1, 0x2E, 0x6E),
        DbKind::InfluxDb => Color32::from_rgb(0x4F, 0x46, 0xE5),
        DbKind::Spark => Color32::from_rgb(0xE2, 0x5A, 0x1C),
        DbKind::H2 => Color32::from_rgb(0x5C, 0x6B, 0x7A),
    }
}

fn monogram(kind: DbKind) -> &'static str {
    match kind {
        DbKind::Postgres => "Pg",
        DbKind::MySql => "My",
        DbKind::MsSql => "SS",
        DbKind::Sqlite => "Li",
        DbKind::MongoDb => "Mo",
        DbKind::Oracle => "Or",
        DbKind::Db2 => "D2",
        DbKind::DuckDb => "Dk",
        DbKind::Cassandra => "Ca",
        DbKind::CouchDb => "Co",
        DbKind::CockroachDb => "Cr",
        DbKind::TimescaleDb => "Ts",
        DbKind::QuestDb => "Qd",
        DbKind::InfluxDb => "Ix",
        DbKind::Spark => "Sp",
        DbKind::H2 => "H2",
    }
}

pub fn file_stem(kind: DbKind) -> &'static str {
    match kind {
        DbKind::Postgres => "postgresql",
        DbKind::MySql => "mysql",
        DbKind::MsSql => "sqlserver",
        DbKind::Sqlite => "sqlite",
        DbKind::MongoDb => "mongodb",
        DbKind::Oracle => "oracle",
        DbKind::Db2 => "db2",
        DbKind::DuckDb => "duckdb",
        DbKind::Cassandra => "cassandra",
        DbKind::CouchDb => "couchdb",
        DbKind::CockroachDb => "cockroachdb",
        DbKind::TimescaleDb => "timescaledb",
        DbKind::QuestDb => "questdb",
        DbKind::InfluxDb => "influxdb",
        DbKind::Spark => "spark",
        DbKind::H2 => "h2",
    }
}

/// Logos compiled into the binary; a file of the same name in the `logos`
/// folder next to the app replaces them. Where each one comes from, and its
/// license, is in `assets/logos/SOURCES.md`.
const BUILT_IN: [(DbKind, &str, &[u8]); 16] = [
    (DbKind::Postgres, "postgresql.svg", include_bytes!("../assets/logos/postgresql.svg")),
    (DbKind::MySql, "mysql.svg", include_bytes!("../assets/logos/mysql.svg")),
    (DbKind::MsSql, "sqlserver.png", include_bytes!("../assets/logos/sqlserver.png")),
    (DbKind::Sqlite, "sqlite.svg", include_bytes!("../assets/logos/sqlite.svg")),
    (DbKind::MongoDb, "mongodb.svg", include_bytes!("../assets/logos/mongodb.svg")),
    (DbKind::Oracle, "oracle.svg", include_bytes!("../assets/logos/oracle.svg")),
    (DbKind::Db2, "db2.svg", include_bytes!("../assets/logos/db2.svg")),
    (DbKind::DuckDb, "duckdb.svg", include_bytes!("../assets/logos/duckdb.svg")),
    (DbKind::Cassandra, "cassandra.svg", include_bytes!("../assets/logos/cassandra.svg")),
    (DbKind::CouchDb, "couchdb.svg", include_bytes!("../assets/logos/couchdb.svg")),
    (DbKind::CockroachDb, "cockroachdb.svg", include_bytes!("../assets/logos/cockroachdb.svg")),
    (DbKind::TimescaleDb, "timescaledb.svg", include_bytes!("../assets/logos/timescaledb.svg")),
    (DbKind::QuestDb, "questdb.svg", include_bytes!("../assets/logos/questdb.svg")),
    (DbKind::InfluxDb, "influxdb.svg", include_bytes!("../assets/logos/influxdb.svg")),
    (DbKind::Spark, "spark.svg", include_bytes!("../assets/logos/spark.svg")),
    (DbKind::H2, "h2.svg", include_bytes!("../assets/logos/h2.svg")),
];

/// Logo images found on disk, by engine.
#[derive(Default)]
pub struct Logos {
    uris: HashMap<DbKind, String>,
}

/// The folder the application lives in: the folder holding `databug` /
/// `DataBug.exe`, or on macOS the folder holding `DataBug.app`.
pub fn app_dir() -> PathBuf {
    let exe = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok().or(Some(p)));
    let Some(dir) = exe.as_deref().and_then(|e| e.parent()) else { return PathBuf::from(".") };
    app_dir_from(dir)
}

fn app_dir_from(dir: &std::path::Path) -> PathBuf {
    // DataBug.app/Contents/MacOS/databug → the folder containing DataBug.app
    if dir.ends_with("Contents/MacOS") {
        if let Some(bundle) = dir.parent().and_then(|c| c.parent()) {
            if bundle.extension().map(|e| e == "app").unwrap_or(false) {
                if let Some(parent) = bundle.parent() {
                    return strip_verbatim(parent);
                }
            }
        }
    }
    strip_verbatim(dir)
}

/// Windows' `\\?\C:\…` form from canonicalize → `C:\…`.
fn strip_verbatim(p: &std::path::Path) -> PathBuf {
    let s = p.display().to_string();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}

/// Where users put logo files: `logos` next to the application.
pub fn logos_dir() -> PathBuf {
    app_dir().join("logos")
}

impl Logos {
    /// Look for `postgresql.svg|png`, `mysql…`, `sqlserver…`, `sqlite…`,
    /// `mongodb…` in the `logos` folder next to the application (and, for a
    /// macOS bundle, inside `DataBug.app/Contents/Resources/logos`).
    /// Anything not found on disk falls back to the copies built into the
    /// binary (`assets/logos`), so every build ships with logos.
    pub fn discover(ctx: &egui::Context) -> Self {
        let mut dirs = vec![logos_dir()];
        if let Some(dir) = std::env::current_exe().ok().as_deref().and_then(|e| e.parent()) {
            dirs.push(dir.join("logos"));
            dirs.push(dir.join("../Resources/logos"));
        }
        let mut uris = HashMap::new();
        for kind in DbKind::ALL {
            'found: for d in &dirs {
                for ext in ["svg", "png"] {
                    let f = d.join(format!("{}.{ext}", file_stem(kind)));
                    if f.is_file() {
                        // forward slashes and no `\\?\` prefix, so Windows paths load too
                        let path = strip_verbatim(&f).display().to_string().replace('\\', "/");
                        let uri = if path.starts_with('/') { format!("file://{path}") } else { format!("file:///{path}") };
                        uris.insert(kind, uri);
                        break 'found;
                    }
                }
            }
        }
        for (kind, name, bytes) in BUILT_IN {
            if !uris.contains_key(&kind) {
                let uri = format!("bytes://databug/logos/{name}");
                ctx.include_bytes(uri.clone(), bytes);
                uris.insert(kind, uri);
            }
        }
        Logos { uris }
    }

    /// Draw the engine's logo (or badge) in a `size`×`size` square.
    pub fn show(&self, ui: &mut egui::Ui, kind: DbKind, size: f32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        self.paint(ui, rect, kind);
    }

    pub fn paint(&self, ui: &egui::Ui, rect: Rect, kind: DbKind) {
        match self.uris.get(&kind) {
            Some(uri) => {
                // the MySQL dolphin is dark blue; give it a light tile so it reads on the dark theme
                if kind == DbKind::MySql && ui.visuals().dark_mode {
                    ui.painter().rect_filled(rect.expand(1.0), egui::CornerRadius::same(4), egui::Color32::from_rgb(0xE8, 0xEC, 0xF2));
                    egui::Image::new(uri.as_str()).fit_to_exact_size(rect.size() * 0.86).paint_at(ui, Rect::from_center_size(rect.center(), rect.size() * 0.86));
                } else {
                    egui::Image::new(uri.as_str()).fit_to_exact_size(rect.size()).paint_at(ui, rect);
                }
            }
            None => engine_badge(ui.painter(), rect, kind),
        }
    }
}

/// DataBug's own engine badge: a rounded tile in the engine's color with a
/// two-letter monogram.
pub fn engine_badge(painter: &Painter, rect: Rect, kind: DbKind) {
    let c = engine_color(kind);
    painter.rect_filled(rect, CornerRadius::same((rect.height() * 0.28) as u8), c);
    painter.text(rect.center() + egui::vec2(0.0, 0.5), egui::Align2::CENTER_CENTER, monogram(kind), crate::theme::bold(rect.height() * 0.5), Color32::WHITE);
}

pub fn kind_color(kind: ObjectKind, p: &Palette) -> Color32 {
    match kind {
        ObjectKind::Table => p.accent,
        ObjectKind::View => p.syn_quoted,
        ObjectKind::MaterializedView => p.syn_number,
        ObjectKind::Collection => p.success,
        ObjectKind::Procedure | ObjectKind::ClrProcedure => p.syn_keyword,
        ObjectKind::Function | ObjectKind::TableFunction | ObjectKind::Aggregate | ObjectKind::ClrFunction | ObjectKind::ClrTableFunction | ObjectKind::ClrAggregate => p.syn_function,
        ObjectKind::Trigger | ObjectKind::ClrTrigger => p.warning,
        ObjectKind::Sequence => p.syn_number,
        ObjectKind::Synonym | ObjectKind::Event => p.muted,
        ObjectKind::Assembly => p.syn_quoted,
    }
}

fn grid_glyph(painter: &Painter, r: Rect, color: Color32, filled_header: bool) {
    let s = Stroke::new(1.2_f32, color);
    painter.rect_stroke(r, CornerRadius::same(2), s, StrokeKind::Inside);
    let header = Rect::from_min_max(r.min, Pos2::new(r.max.x, r.min.y + r.height() * 0.32));
    if filled_header {
        painter.rect_filled(header, CornerRadius::same(2), color);
    } else {
        painter.line_segment([Pos2::new(r.left(), header.bottom()), Pos2::new(r.right(), header.bottom())], s);
    }
    painter.line_segment([Pos2::new(r.center().x, header.bottom()), Pos2::new(r.center().x, r.bottom())], Stroke::new(1.0_f32, color));
}

/// A 14px glyph for an object kind.
pub fn paint_kind(painter: &Painter, rect: Rect, kind: ObjectKind, p: &Palette) {
    let c = kind_color(kind, p);
    let r = Rect::from_center_size(rect.center(), egui::vec2(12.0, 11.0));
    let text = |t: &str, size: f32| {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, t, FontId::proportional(size), c);
    };
    let tile = |t: &str| {
        painter.rect_filled(r, CornerRadius::same(3), c.gamma_multiply(0.22));
        painter.text(r.center(), egui::Align2::CENTER_CENTER, t, crate::theme::bold(8.5), c);
    };
    match kind {
        ObjectKind::Table => grid_glyph(painter, r, c, false),
        ObjectKind::View => {
            grid_glyph(painter, r, c, false);
            painter.circle_filled(Pos2::new(r.right() - 1.0, r.bottom() - 1.0), 2.5, c);
        }
        ObjectKind::MaterializedView => grid_glyph(painter, r, c, true),
        ObjectKind::Collection => {
            painter.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.2_f32, c), StrokeKind::Inside);
            text("{}", 9.0);
        }
        ObjectKind::Procedure => tile("P"),
        ObjectKind::Function => text("ƒ", 14.0),
        ObjectKind::TableFunction => {
            text("ƒ", 14.0);
            painter.rect_stroke(Rect::from_min_size(Pos2::new(r.right() - 4.0, r.bottom() - 4.0), egui::vec2(5.0, 5.0)), CornerRadius::ZERO, Stroke::new(1.0_f32, c), StrokeKind::Inside);
        }
        ObjectKind::Aggregate | ObjectKind::ClrAggregate => text("Σ", 12.5),
        ObjectKind::Trigger | ObjectKind::ClrTrigger => {
            let cx = rect.center();
            let pts = vec![cx + egui::vec2(1.5, -6.0), cx + egui::vec2(-3.0, 0.5), cx + egui::vec2(0.5, 0.5), cx + egui::vec2(-1.5, 6.0), cx + egui::vec2(3.5, -1.0), cx + egui::vec2(0.0, -1.0)];
            painter.add(egui::Shape::convex_polygon(pts, c, Stroke::NONE));
            if kind == ObjectKind::ClrTrigger {
                painter.circle_filled(Pos2::new(r.right(), r.top() + 1.0), 2.2, p.syn_quoted);
            }
        }
        ObjectKind::Sequence => text("#", 12.0),
        ObjectKind::Synonym => text("⇢", 12.0),
        ObjectKind::Event => {
            painter.circle_stroke(rect.center(), 5.0, Stroke::new(1.2_f32, c));
            painter.line_segment([rect.center(), rect.center() + egui::vec2(0.0, -3.5)], Stroke::new(1.2_f32, c));
            painter.line_segment([rect.center(), rect.center() + egui::vec2(2.5, 1.0)], Stroke::new(1.2_f32, c));
        }
        ObjectKind::ClrProcedure => tile("CP"),
        ObjectKind::ClrFunction | ObjectKind::ClrTableFunction => tile("Cƒ"),
        ObjectKind::Assembly => {
            // a small hexagon
            let cx = rect.center();
            let pts: Vec<Pos2> = (0..6).map(|i| {
                let a = std::f32::consts::FRAC_PI_3 * i as f32 + std::f32::consts::FRAC_PI_6;
                cx + egui::vec2(a.cos() * 6.0, a.sin() * 6.0)
            }).collect();
            painter.add(egui::Shape::closed_line(pts, Stroke::new(1.2_f32, c)));
            painter.text(cx, egui::Align2::CENTER_CENTER, ".N", FontId::proportional(7.0), c);
        }
    }
}

pub fn paint_column(painter: &Painter, rect: Rect, pk: bool, fk: bool, p: &Palette) {
    let c = rect.center();
    if pk {
        painter.circle_stroke(c + egui::vec2(-2.5, 0.0), 2.8, Stroke::new(1.4_f32, p.warning));
        painter.line_segment([c + egui::vec2(0.3, 0.0), c + egui::vec2(5.5, 0.0)], Stroke::new(1.4_f32, p.warning));
        painter.line_segment([c + egui::vec2(4.0, 0.0), c + egui::vec2(4.0, 2.5)], Stroke::new(1.2_f32, p.warning));
    } else if fk {
        painter.line_segment([c + egui::vec2(-5.0, 0.0), c + egui::vec2(4.0, 0.0)], Stroke::new(1.4_f32, p.syn_number));
        painter.line_segment([c + egui::vec2(1.0, -3.0), c + egui::vec2(4.5, 0.0)], Stroke::new(1.4_f32, p.syn_number));
        painter.line_segment([c + egui::vec2(1.0, 3.0), c + egui::vec2(4.5, 0.0)], Stroke::new(1.4_f32, p.syn_number));
    } else {
        painter.rect_filled(Rect::from_center_size(c, egui::vec2(6.0, 6.0)), CornerRadius::same(1), p.faint);
    }
}

pub fn paint_index(painter: &Painter, rect: Rect, unique: bool, p: &Palette) {
    let c = rect.center();
    let color = if unique { p.syn_keyword } else { p.muted };
    for (i, w) in [8.0_f32, 6.0, 4.0].iter().enumerate() {
        let y = c.y - 3.0 + i as f32 * 3.0;
        painter.line_segment([Pos2::new(c.x - 4.0, y), Pos2::new(c.x - 4.0 + w, y)], Stroke::new(1.4_f32, color));
    }
}

/// A pencil, for edit buttons.
pub fn paint_pencil(painter: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let a = c + egui::vec2(-4.0, 4.0);
    let b = c + egui::vec2(3.5, -3.5);
    painter.line_segment([a, b], Stroke::new(2.4_f32, color));
    painter.line_segment([b + egui::vec2(-0.8, -0.8), b + egui::vec2(0.9, 0.9)], Stroke::new(2.6_f32, color.gamma_multiply(0.6)));
    painter.add(egui::Shape::convex_polygon(vec![a + egui::vec2(-1.5, 1.5), a + egui::vec2(-0.5, -1.6), a + egui::vec2(1.6, 0.5)], color, Stroke::NONE));
}

/// Expand/collapse triangle.
pub fn paint_arrow(painter: &Painter, rect: Rect, open: bool, color: Color32) {
    let c = rect.center();
    let pts = if open {
        vec![c + egui::vec2(-3.5, -1.8), c + egui::vec2(3.5, -1.8), c + egui::vec2(0.0, 2.6)]
    } else {
        vec![c + egui::vec2(-1.8, -3.5), c + egui::vec2(2.6, 0.0), c + egui::vec2(-1.8, 3.5)]
    };
    painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_dir_skips_the_mac_bundle() {
        assert_eq!(app_dir_from(std::path::Path::new("/Applications/DataBug.app/Contents/MacOS")), PathBuf::from("/Applications"));
        assert_eq!(app_dir_from(std::path::Path::new("/opt/databug")), PathBuf::from("/opt/databug"));
        assert_eq!(strip_verbatim(std::path::Path::new(r"\\?\C:\Tools\DataBug")), PathBuf::from(r"C:\Tools\DataBug"));
    }

    #[test]
    fn every_engine_has_a_built_in_logo() {
        for kind in DbKind::ALL {
            let (_, name, bytes) = BUILT_IN.iter().find(|(k, _, _)| *k == kind).unwrap_or_else(|| panic!("no logo for {kind:?}"));
            assert!(name.starts_with(file_stem(kind)), "{name}");
            assert!(!bytes.is_empty());
        }
    }
}
