//! Colors, fonts and widget styling. The dark theme follows the look of
//! JetBrains' "Islands" UI: rounded panels floating on a darker canvas.

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, TextStyle, Visuals};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize, Default)]
pub enum ThemeKind {
    #[default]
    Dark,
    Light,
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

pub fn palette(kind: ThemeKind) -> &'static Palette {
    match kind {
        ThemeKind::Dark => &DARK,
        ThemeKind::Light => &LIGHT,
    }
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

pub fn apply(ctx: &egui::Context, kind: ThemeKind, code_size: f32) {
    let p = palette(kind);
    let mut v = match kind {
        ThemeKind::Dark => Visuals::dark(),
        ThemeKind::Light => Visuals::light(),
    };
    v.panel_fill = p.island;
    v.window_fill = p.island;
    // text fields sit slightly inset from panels and dialogs
    v.extreme_bg_color = match kind {
        ThemeKind::Dark => p.canvas,
        ThemeKind::Light => p.raised,
    };
    v.faint_bg_color = p.grid_alt;
    v.code_bg_color = p.raised;
    v.window_stroke = Stroke::new(1.0_f32, p.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(if kind == ThemeKind::Dark { 140 } else { 40 }) };
    v.popup_shadow = egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(if kind == ThemeKind::Dark { 120 } else { 30 }) };
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
