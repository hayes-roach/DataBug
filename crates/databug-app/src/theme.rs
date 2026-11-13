//! Colors, fonts and widget styling. The dark theme follows the look of
//! JetBrains' "Islands" UI: rounded panels floating on a darker canvas.

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, TextStyle, Visuals};
use std::sync::Arc;

/// Built-in color themes. `Dark` is the default.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize, Default)]
pub enum ThemeKind {
    #[default]
    Dark,
    Light,
    Midnight,
    Forest,
    Paper,
    HighContrast,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 6] = [ThemeKind::Dark, ThemeKind::Light, ThemeKind::Midnight, ThemeKind::Forest, ThemeKind::Paper, ThemeKind::HighContrast];

    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::Dark => "Dark",
            ThemeKind::Light => "Light",
            ThemeKind::Midnight => "Midnight",
            ThemeKind::Forest => "Forest",
            ThemeKind::Paper => "Paper",
            ThemeKind::HighContrast => "High contrast",
        }
    }

    pub fn is_dark(self) -> bool {
        !matches!(self, ThemeKind::Light | ThemeKind::Paper)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub canvas: Color32,
    pub island: Color32,
    pub raised: Color32,
    pub hover: Color32,
    pub selection: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub success: Color32,
    pub danger: Color32,
    pub warning: Color32,
    pub grid_alt: Color32,
    pub edited: Color32,
    /// Background of the selected explorer row: darker than the panel.
    pub tree_selected: Color32,
    pub syn_keyword: Color32,
    pub syn_string: Color32,
    pub syn_number: Color32,
    pub syn_comment: Color32,
    pub syn_ident: Color32,
    pub syn_quoted: Color32,
    pub syn_function: Color32,
    pub syn_param: Color32,
    pub syn_operator: Color32,
}

pub const DARK: Palette = Palette {
    canvas: Color32::from_rgb(0x14, 0x15, 0x17),
    island: Color32::from_rgb(0x1E, 0x1F, 0x22),
    raised: Color32::from_rgb(0x2B, 0x2D, 0x30),
    hover: Color32::from_rgb(0x2E, 0x30, 0x35),
    selection: Color32::from_rgb(0x2E, 0x43, 0x6E),
    border: Color32::from_rgb(0x39, 0x3B, 0x40),
    text: Color32::from_rgb(0xDF, 0xE1, 0xE5),
    muted: Color32::from_rgb(0x8C, 0x8F, 0x96),
    faint: Color32::from_rgb(0x5A, 0x5D, 0x63),
    accent: Color32::from_rgb(0x35, 0x74, 0xF0),
    success: Color32::from_rgb(0x5F, 0xB8, 0x65),
    danger: Color32::from_rgb(0xF7, 0x54, 0x64),
    warning: Color32::from_rgb(0xE0, 0xB0, 0x4A),
    grid_alt: Color32::from_rgb(0x22, 0x23, 0x26),
    edited: Color32::from_rgb(0x3D, 0x3A, 0x22),
    tree_selected: Color32::from_rgb(0x0E, 0x0F, 0x11),
    syn_keyword: Color32::from_rgb(0xCF, 0x8E, 0x6D),
    syn_string: Color32::from_rgb(0x6A, 0xAB, 0x73),
    syn_number: Color32::from_rgb(0x2A, 0xAC, 0xB8),
    syn_comment: Color32::from_rgb(0x7A, 0x7E, 0x85),
    syn_ident: Color32::from_rgb(0xBC, 0xBE, 0xC4),
    syn_quoted: Color32::from_rgb(0xC7, 0x7D, 0xBB),
    syn_function: Color32::from_rgb(0x56, 0xA8, 0xF5),
    syn_param: Color32::from_rgb(0xC7, 0x7D, 0xBB),
    syn_operator: Color32::from_rgb(0xA9, 0xB7, 0xC6),
};

pub const LIGHT: Palette = Palette {
    canvas: Color32::from_rgb(0xE6, 0xE8, 0xEC),
    island: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    raised: Color32::from_rgb(0xF2, 0xF3, 0xF5),
    hover: Color32::from_rgb(0xE9, 0xEB, 0xEF),
    selection: Color32::from_rgb(0xD4, 0xE2, 0xFF),
    border: Color32::from_rgb(0xD3, 0xD5, 0xDB),
    text: Color32::from_rgb(0x1E, 0x1F, 0x22),
    muted: Color32::from_rgb(0x6C, 0x70, 0x7E),
    faint: Color32::from_rgb(0xA8, 0xAD, 0xBD),
    accent: Color32::from_rgb(0x35, 0x74, 0xF0),
    success: Color32::from_rgb(0x20, 0x8A, 0x3C),
    danger: Color32::from_rgb(0xDB, 0x3B, 0x4B),
    warning: Color32::from_rgb(0xB8, 0x80, 0x10),
    grid_alt: Color32::from_rgb(0xF7, 0xF8, 0xFA),
    edited: Color32::from_rgb(0xFF, 0xF4, 0xC9),
    tree_selected: Color32::from_rgb(0xD5, 0xD8, 0xDF),
    syn_keyword: Color32::from_rgb(0x00, 0x33, 0xB3),
    syn_string: Color32::from_rgb(0x06, 0x7D, 0x17),
    syn_number: Color32::from_rgb(0x17, 0x50, 0xEB),
    syn_comment: Color32::from_rgb(0x8C, 0x8C, 0x8C),
    syn_ident: Color32::from_rgb(0x08, 0x08, 0x08),
    syn_quoted: Color32::from_rgb(0x87, 0x10, 0x94),
    syn_function: Color32::from_rgb(0x00, 0x62, 0x7A),
    syn_param: Color32::from_rgb(0x87, 0x10, 0x94),
    syn_operator: Color32::from_rgb(0x08, 0x08, 0x08),
};

const fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Deep navy with cool blues and teal accents.
pub const MIDNIGHT: Palette = Palette {
    canvas: rgb(0x0B1020),
    island: rgb(0x121A2E),
    raised: rgb(0x1C2640),
    hover: rgb(0x223050),
    selection: rgb(0x26407A),
    border: rgb(0x2B3756),
    text: rgb(0xE3E8F4),
    muted: rgb(0x8B97B5),
    faint: rgb(0x56617E),
    accent: rgb(0x5A8DFF),
    success: rgb(0x4FC08D),
    danger: rgb(0xFF6B7F),
    warning: rgb(0xF2C14E),
    grid_alt: rgb(0x151E34),
    edited: rgb(0x3A3620),
    tree_selected: rgb(0x080C18),
    syn_keyword: rgb(0x82AAFF),
    syn_string: rgb(0xC3E88D),
    syn_number: rgb(0xF78C6C),
    syn_comment: rgb(0x637196),
    syn_ident: rgb(0xD0D7EA),
    syn_quoted: rgb(0xC792EA),
    syn_function: rgb(0x7FDBCA),
    syn_param: rgb(0xFFCB6B),
    syn_operator: rgb(0x89DDFF),
};

/// Dark greens and warm earth tones.
pub const FOREST: Palette = Palette {
    canvas: rgb(0x111612),
    island: rgb(0x18201A),
    raised: rgb(0x243026),
    hover: rgb(0x2A372C),
    selection: rgb(0x2F5236),
    border: rgb(0x34413A),
    text: rgb(0xDDE5D8),
    muted: rgb(0x8E9E8A),
    faint: rgb(0x5B6A5A),
    accent: rgb(0x6CBF6A),
    success: rgb(0x8FD16B),
    danger: rgb(0xE5675D),
    warning: rgb(0xE3B45A),
    grid_alt: rgb(0x1C251E),
    edited: rgb(0x3D3A1F),
    tree_selected: rgb(0x0B0F0C),
    syn_keyword: rgb(0xD9A35B),
    syn_string: rgb(0xA8CF7A),
    syn_number: rgb(0x7FC8B5),
    syn_comment: rgb(0x6F7F6B),
    syn_ident: rgb(0xCBD5C3),
    syn_quoted: rgb(0xD39BC0),
    syn_function: rgb(0x8CC4E8),
    syn_param: rgb(0xE8C07D),
    syn_operator: rgb(0xB6C4AE),
};

/// Warm off-white paper with ink-dark text.
pub const PAPER: Palette = Palette {
    canvas: rgb(0xE9E2D3),
    island: rgb(0xFBF7EE),
    raised: rgb(0xF1EBDD),
    hover: rgb(0xEAE2D0),
    selection: rgb(0xF2DFAE),
    border: rgb(0xD8CFBB),
    text: rgb(0x2B2620),
    muted: rgb(0x6E6455),
    faint: rgb(0xA89D8A),
    accent: rgb(0xB5651D),
    success: rgb(0x4E7D2B),
    danger: rgb(0xB8352F),
    warning: rgb(0x9C6A00),
    grid_alt: rgb(0xF5F0E4),
    edited: rgb(0xFFEFB8),
    tree_selected: rgb(0xE3D9C3),
    syn_keyword: rgb(0x8A3B12),
    syn_string: rgb(0x3D6B1F),
    syn_number: rgb(0x1F5E8A),
    syn_comment: rgb(0x9A8E78),
    syn_ident: rgb(0x2B2620),
    syn_quoted: rgb(0x7A2E73),
    syn_function: rgb(0x0F5C6E),
    syn_param: rgb(0x7A2E73),
    syn_operator: rgb(0x3E372E),
};

/// Black background, bright text and saturated syntax colors.
pub const HIGH_CONTRAST: Palette = Palette {
    canvas: rgb(0x000000),
    island: rgb(0x0A0A0A),
    raised: rgb(0x1E1E1E),
    hover: rgb(0x2A2A2A),
    selection: rgb(0x0050C8),
    border: rgb(0x6F6F6F),
    text: rgb(0xFFFFFF),
    muted: rgb(0xC8C8C8),
    faint: rgb(0x9A9A9A),
    accent: rgb(0x3FA9FF),
    success: rgb(0x4CE07A),
    danger: rgb(0xFF5C5C),
    warning: rgb(0xFFD33F),
    grid_alt: rgb(0x141414),
    edited: rgb(0x4A4200),
    tree_selected: rgb(0x003A8C),
    syn_keyword: rgb(0x5CC8FF),
    syn_string: rgb(0x7CFF8A),
    syn_number: rgb(0xFFB45C),
    syn_comment: rgb(0xB0B0B0),
    syn_ident: rgb(0xFFFFFF),
    syn_quoted: rgb(0xFF8CF0),
    syn_function: rgb(0xFFE66B),
    syn_param: rgb(0xFF8CF0),
    syn_operator: rgb(0xFFFFFF),
};

pub fn preset(kind: ThemeKind) -> &'static Palette {
    match kind {
        ThemeKind::Dark => &DARK,
        ThemeKind::Light => &LIGHT,
        ThemeKind::Midnight => &MIDNIGHT,
        ThemeKind::Forest => &FOREST,
        ThemeKind::Paper => &PAPER,
        ThemeKind::HighContrast => &HIGH_CONTRAST,
    }
}

/// Every palette color a user can customize: (key, label, accessor).
pub type ColorSlot = (&'static str, &'static str, fn(&mut Palette) -> &mut Color32);

pub const SLOTS: &[ColorSlot] = &[
    ("canvas", "Window background", |p| &mut p.canvas),
    ("island", "Panels", |p| &mut p.island),
    ("raised", "Buttons and inputs", |p| &mut p.raised),
    ("hover", "Hover", |p| &mut p.hover),
    ("selection", "Selection", |p| &mut p.selection),
    ("border", "Borders", |p| &mut p.border),
    ("text", "Text", |p| &mut p.text),
    ("muted", "Secondary text", |p| &mut p.muted),
    ("faint", "Faint text", |p| &mut p.faint),
    ("accent", "Accent", |p| &mut p.accent),
    ("success", "Success", |p| &mut p.success),
    ("danger", "Danger", |p| &mut p.danger),
    ("warning", "Warning", |p| &mut p.warning),
    ("grid_alt", "Alternate grid rows", |p| &mut p.grid_alt),
    ("edited", "Edited cells", |p| &mut p.edited),
    ("tree_selected", "Selected explorer row", |p| &mut p.tree_selected),
    ("syn_keyword", "Code: keywords", |p| &mut p.syn_keyword),
    ("syn_ident", "Code: identifiers", |p| &mut p.syn_ident),
    ("syn_string", "Code: strings", |p| &mut p.syn_string),
    ("syn_number", "Code: numbers", |p| &mut p.syn_number),
    ("syn_comment", "Code: comments", |p| &mut p.syn_comment),
    ("syn_quoted", "Code: quoted names", |p| &mut p.syn_quoted),
    ("syn_function", "Code: functions", |p| &mut p.syn_function),
    ("syn_param", "Code: parameters", |p| &mut p.syn_param),
    ("syn_operator", "Code: operators", |p| &mut p.syn_operator),
];

/// A preset with the user's color overrides (key → RGB) applied.
pub fn palette(kind: ThemeKind, overrides: &std::collections::BTreeMap<String, [u8; 3]>) -> Palette {
    let mut p = *preset(kind);
    for (key, _, get) in SLOTS {
        if let Some(c) = overrides.get(*key) {
            *get(&mut p) = Color32::from_rgb(c[0], c[1], c[2]);
        }
    }
    p
}

/// A stable fingerprint of a palette, so caches keyed on colors can tell
/// when the theme changed.
pub fn fingerprint(p: &Palette) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut q = *p;
    for (_, _, get) in SLOTS {
        get(&mut q).to_array().hash(&mut h);
    }
    h.finish()
}

pub const BOLD: &str = "bold";

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("inter".into(), Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Inter-Regular.otf"))));
    fonts.font_data.insert("inter-semibold".into(), Arc::new(FontData::from_static(include_bytes!("../assets/fonts/Inter-SemiBold.otf"))));
    fonts.font_data.insert("jbmono".into(), Arc::new(FontData::from_static(include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"))));
    // Keep egui's bundled fonts as fallbacks for symbols and emoji.
    fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "inter".into());
    fonts.families.get_mut(&FontFamily::Monospace).unwrap().insert(0, "jbmono".into());
    let mut bold = vec!["inter-semibold".to_string()];
    bold.extend(fonts.families[&FontFamily::Proportional].iter().skip(1).cloned());
    fonts.families.insert(FontFamily::Name(BOLD.into()), bold);
    ctx.set_fonts(fonts);
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}

pub fn apply(ctx: &egui::Context, p: &Palette, dark: bool, code_size: f32) {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.island;
    v.window_fill = p.island;
    // text fields sit slightly inset from panels and dialogs
    v.extreme_bg_color = if dark { p.canvas } else { p.raised };
    v.faint_bg_color = p.grid_alt;
    v.code_bg_color = p.raised;
    v.window_stroke = Stroke::new(1.0_f32, p.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(if dark { 140 } else { 40 }) };
    v.popup_shadow = egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(if dark { 120 } else { 30 }) };
    v.selection.bg_fill = p.selection;
    v.selection.stroke = Stroke::new(1.0_f32, p.text);
    v.hyperlink_color = p.accent;
    v.override_text_color = Some(p.text);
    v.striped = true;
    v.text_cursor.stroke = Stroke::new(2.0_f32, p.accent);
    v.indent_has_left_vline = false;
    v.collapsing_header_frame = false;

    let r = CornerRadius::same(6);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.island;
    w.noninteractive.weak_bg_fill = p.island;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = p.raised;
    w.inactive.weak_bg_fill = p.raised;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = p.hover;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, p.border);
    w.hovered.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;
    w.active.bg_fill = p.selection;
    w.active.weak_bg_fill = p.selection;
    w.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    w.active.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;
    w.open.bg_fill = p.hover;
    w.open.weak_bg_fill = p.hover;
    w.open.corner_radius = r;

    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Body, FontId::proportional(13.0)),
            (TextStyle::Button, FontId::proportional(13.0)),
            (TextStyle::Heading, bold(18.0)),
            (TextStyle::Monospace, FontId::monospace(code_size)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 4.0);
        s.spacing.button_padding = egui::vec2(9.0, 3.0);
        s.spacing.interact_size = egui::vec2(24.0, 24.0);
        s.spacing.menu_margin = Margin::same(6);
        s.spacing.window_margin = Margin::same(16);
        s.spacing.indent = 14.0;
        s.spacing.scroll = egui::style::ScrollStyle::thin();
        // scroll immediately instead of gliding: snappier lists and grids
        s.scroll_animation = egui::style::ScrollAnimation::none();
        s.interaction.selectable_labels = false;
        s.interaction.tooltip_delay = 0.35;
        s.animation_time = 0.08;
    });
}

pub fn connection_color(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

pub const SWATCHES: [[u8; 3]; 8] = [
    [86, 156, 214],
    [95, 184, 101],
    [224, 176, 74],
    [199, 125, 187],
    [42, 172, 184],
    [207, 142, 109],
    [140, 143, 150],
    [247, 84, 100],
];
