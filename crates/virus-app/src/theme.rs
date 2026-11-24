//! The panel looks (SPEC §5.10): the Virus Rack's crimson and black (default) or the Virus TI's
//! dark gunmetal with blue highlights. Every colour, rounding and spacing constant lives here so
//! the look can be tuned in one place.

use std::sync::atomic::{AtomicU8, Ordering};

use egui::{Color32, Context, FontId, Frame, Margin, RichText, Rounding, Stroke, Ui, Visuals};

// --- Themes --------------------------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeKind {
    /// Crimson panel, black chassis, amber LEDs.
    #[default]
    Rack,
    /// Dark gunmetal panel, light blue highlights, red LEDs, grey LCD.
    Ti,
}

impl ThemeKind {
    pub const ALL: [Self; 2] = [Self::Rack, Self::Ti];

    /// The settings key.
    pub fn key(self) -> &'static str {
        match self {
            Self::Rack => "rack",
            Self::Ti => "ti",
        }
    }

    /// Unknown keys fall back to the default.
    pub fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|t| t.key() == key)
            .unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Rack => "Virus Rack (red)",
            Self::Ti => "Virus TI (dark)",
        }
    }

    pub fn palette(self) -> &'static Palette {
        match self {
            Self::Rack => &RACK,
            Self::Ti => &TI,
        }
    }
}

/// The colours that differ between themes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// Header bar, and the legends and outline on it.
    pub header: Color32,
    pub on_header: Color32,
    pub header_edge: Color32,
    /// Selected tab, pressed widgets, text selection; with its legend.
    pub accent: Color32,
    pub on_accent: Color32,
    /// Hovered widgets (legend is white).
    pub accent_hover: Color32,
    /// Pressed widgets (legend is white). egui also draws "strong" text in that legend colour.
    pub pressed: Color32,
    /// The accent as small text on the dark backgrounds.
    pub accent_text: Color32,
    /// Knob arcs and the primary curve trace.
    pub arc: Color32,
    /// A held piano key (on white).
    pub key_held: Color32,
    /// Section title strips, their legend, and the section outline.
    pub strip: Color32,
    pub on_strip: Color32,
    pub section_edge: Color32,
    /// Window and panel background.
    pub chassis: Color32,
    /// Knob and section bodies.
    pub control_black: Color32,
    /// Buttons and combo boxes.
    pub control_grey: Color32,
    /// Edges of controls and separators.
    pub edge: Color32,
    /// LEDs: Rx/Tx, tab LEDs, second curve trace.
    pub led_on: Color32,
    pub led_off: Color32,
    /// LCD readout background, text, and error text on it.
    pub lcd_bg: Color32,
    pub lcd_text: Color32,
    pub lcd_alert: Color32,
}

/// The Virus Rack: crimson front panel (sampled from the hardware, #C42343) with white legends.
pub const RACK: Palette = Palette {
    header: Color32::from_rgb(0xC4, 0x23, 0x43),
    on_header: Color32::WHITE,
    header_edge: Color32::from_rgb(0x8A, 0x15, 0x2E),
    accent: Color32::from_rgb(0xC4, 0x23, 0x43),
    on_accent: Color32::WHITE,
    accent_hover: Color32::from_rgb(0x8A, 0x15, 0x2E),
    pressed: Color32::from_rgb(0xC4, 0x23, 0x43),
    accent_text: Color32::from_rgb(0xFF, 0x6A, 0x84),
    // The panel red is too dark for a thin arc on black; this is the same hue, lighter.
    arc: Color32::from_rgb(0xE0, 0x3A, 0x5C),
    key_held: Color32::from_rgb(0xC4, 0x23, 0x43),
    strip: Color32::from_rgb(0xC4, 0x23, 0x43),
    on_strip: Color32::WHITE,
    section_edge: Color32::from_rgb(0x8A, 0x15, 0x2E),
    chassis: Color32::from_rgb(0x14, 0x14, 0x14),
    control_black: Color32::from_rgb(0x1E, 0x1E, 0x1E),
    control_grey: Color32::from_rgb(0x2A, 0x2A, 0x2A),
    edge: Color32::from_rgb(0x4A, 0x4A, 0x4A),
    led_on: Color32::from_rgb(0xFF, 0xB0, 0x00),
    led_off: Color32::from_rgb(0x3A, 0x2A, 0x08),
    lcd_bg: Color32::from_rgb(0x1C, 0x2A, 0x33),
    lcd_text: Color32::from_rgb(0xCF, 0xE3, 0xEA),
    lcd_alert: DANGER,
};

/// The Virus TI: dark gunmetal panel, light blue section highlights, red LEDs, grey LCD.
pub const TI: Palette = Palette {
    header: Color32::from_rgb(0x2B, 0x2E, 0x33),
    on_header: LEGEND,
    header_edge: Color32::from_rgb(0x4A, 0x4F, 0x57),
    accent: Color32::from_rgb(0x5F, 0xB0, 0xE8),
    on_accent: DEEP_BLACK,
    accent_hover: Color32::from_rgb(0x24, 0x50, 0x6E),
    pressed: Color32::from_rgb(0x2A, 0x6A, 0x96),
    accent_text: Color32::from_rgb(0x8C, 0xCB, 0xF5),
    arc: Color32::from_rgb(0x5F, 0xB0, 0xE8),
    key_held: Color32::from_rgb(0x1F, 0x6F, 0xA8),
    strip: Color32::from_rgb(0x3A, 0x3F, 0x46),
    on_strip: LEGEND,
    section_edge: Color32::from_rgb(0x4A, 0x50, 0x58),
    chassis: Color32::from_rgb(0x1C, 0x1E, 0x21),
    control_black: Color32::from_rgb(0x25, 0x28, 0x2C),
    control_grey: Color32::from_rgb(0x33, 0x37, 0x3C),
    edge: Color32::from_rgb(0x56, 0x5B, 0x63),
    led_on: Color32::from_rgb(0xFF, 0x3B, 0x1F),
    led_off: Color32::from_rgb(0x3D, 0x14, 0x10),
    lcd_bg: Color32::from_rgb(0xA7, 0xB0, 0xAB),
    lcd_text: Color32::from_rgb(0x10, 0x14, 0x14),
    lcd_alert: Color32::from_rgb(0x7A, 0x0A, 0x0A),
};

/// The active theme. Global rather than threaded through every widget: there is one window and
/// the theme only changes from the header menu.
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn current() -> ThemeKind {
    ThemeKind::ALL
        .get(usize::from(CURRENT.load(Ordering::Relaxed)))
        .copied()
        .unwrap_or_default()
}

/// The active theme's colours.
pub fn palette() -> &'static Palette {
    current().palette()
}

// --- Colours shared by every theme ---------------------------------------------------------
/// Deepest black: text fields, curve displays, the logo plate.
pub const DEEP_BLACK: Color32 = Color32::from_rgb(0x0A, 0x0A, 0x0A);
/// Legend on black.
pub const LEGEND: Color32 = Color32::from_rgb(0xE8, 0xE8, 0xE8);
/// Secondary text on black.
pub const DIM: Color32 = Color32::from_rgb(0x9A, 0x9A, 0x9A);
/// Black ring around a tab LED, so it reads on any header.
pub const LED_BEZEL: Color32 = DEEP_BLACK;
/// Warnings / errors, legible on the dark backgrounds.
pub const WARN: Color32 = Color32::from_rgb(0xFF, 0xB0, 0x00);
pub const DANGER: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x5E);

// Knob
pub const KNOB_BODY: Color32 = Color32::from_rgb(0x1A, 0x1A, 0x1A);
pub const KNOB_POINTER: Color32 = Color32::from_rgb(0xF4, 0xF4, 0xF4);

// Curve displays
pub const SCOPE_BG: Color32 = DEEP_BLACK;

// Piano
pub const KEY_WHITE: Color32 = LEGEND;
pub const KEY_BLACK: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
pub const KEY_LABEL: Color32 = Color32::from_gray(80);

// --- Geometry ------------------------------------------------------------------------------
pub const ROUNDING: f32 = 3.0;
pub const SECTION_PADDING: f32 = 8.0;
pub const STRIP_PADDING_X: f32 = 8.0;
pub const STRIP_PADDING_Y: f32 = 2.0;
pub const HEADER_PADDING_X: f32 = 10.0;
pub const HEADER_PADDING_Y: f32 = 6.0;
pub const LCD_PADDING_X: f32 = 8.0;
pub const LCD_PADDING_Y: f32 = 2.0;
/// Tab LEDs: bezel and lens radius.
pub const TAB_LED_BEZEL_RADIUS: f32 = 4.5;
pub const TAB_LED_RADIUS: f32 = 3.0;

fn rounding() -> Rounding {
    Rounding::same(ROUNDING)
}

/// Makes `kind` the active theme and installs its visuals and the spacing.
pub fn apply(ctx: &Context, kind: ThemeKind) {
    CURRENT.store(kind as u8, Ordering::Relaxed);
    ctx.set_visuals(visuals(kind.palette()));
    ctx.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 5.0);
        style.spacing.button_padding = egui::vec2(8.0, 3.0);
    });
}

pub fn visuals(p: &Palette) -> Visuals {
    let mut v = Visuals::dark();
    v.dark_mode = true;
    v.override_text_color = None;
    v.panel_fill = p.chassis;
    v.window_fill = p.chassis;
    v.window_stroke = Stroke::new(1.0_f32, p.accent);
    v.window_rounding = Rounding::same(ROUNDING + 2.0);
    v.menu_rounding = rounding();
    v.extreme_bg_color = DEEP_BLACK;
    v.faint_bg_color = p.control_black;
    v.code_bg_color = p.control_black;
    v.hyperlink_color = p.accent_text;
    v.warn_fg_color = WARN;
    v.error_fg_color = DANGER;
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0_f32, p.on_accent);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.control_black;
    w.noninteractive.weak_bg_fill = p.control_black;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.control_grey);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, LEGEND);

    w.inactive.bg_fill = p.control_grey;
    w.inactive.weak_bg_fill = p.control_grey;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, p.edge);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, LEGEND);

    w.hovered.bg_fill = p.accent_hover;
    w.hovered.weak_bg_fill = p.accent_hover;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, p.accent);
    w.hovered.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);

    w.active.bg_fill = p.pressed;
    w.active.weak_bg_fill = p.pressed;
    w.active.bg_stroke = Stroke::new(1.0_f32, p.accent_text);
    w.active.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);

    w.open.bg_fill = p.control_grey;
    w.open.weak_bg_fill = p.control_grey;
    w.open.bg_stroke = Stroke::new(1.0_f32, p.accent);
    w.open.fg_stroke = Stroke::new(1.0_f32, LEGEND);

    for state in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        state.rounding = rounding();
    }
    v
}

/// The header bar.
pub fn header_frame() -> Frame {
    let p = palette();
    Frame::none()
        .fill(p.header)
        .stroke(Stroke::new(1.0_f32, p.header_edge))
        .inner_margin(Margin::symmetric(HEADER_PADDING_X, HEADER_PADDING_Y))
}

/// A dark box with a coloured outline, the body of a panel section.
pub fn section_frame() -> Frame {
    let p = palette();
    Frame::none()
        .fill(p.control_black)
        .stroke(Stroke::new(1.0_f32, p.section_edge))
        .rounding(rounding())
        .inner_margin(Margin::ZERO)
}

/// The LCD readout box.
pub fn lcd_frame() -> Frame {
    Frame::none()
        .fill(palette().lcd_bg)
        .stroke(Stroke::new(1.0_f32, DEEP_BLACK))
        .rounding(rounding())
        .inner_margin(Margin::symmetric(LCD_PADDING_X, LCD_PADDING_Y))
}

pub fn lcd_text(text: impl Into<String>) -> RichText {
    RichText::new(text)
        .font(FontId::monospace(15.0))
        .strong()
        .color(palette().lcd_text)
}

/// A section: coloured title strip over a dark body. `width` defaults to filling the row.
pub fn section(ui: &mut Ui, title: &str, width: Option<f32>, add: impl FnOnce(&mut Ui)) {
    let p = palette();
    section_frame().show(ui, |ui| {
        ui.set_width(width.unwrap_or_else(|| ui.available_width()));
        ui.spacing_mut().item_spacing.y = 0.0;
        let top = Rounding {
            nw: ROUNDING,
            ne: ROUNDING,
            sw: 0.0,
            se: 0.0,
        };
        Frame::none()
            .fill(p.strip)
            .rounding(top)
            .inner_margin(Margin::symmetric(STRIP_PADDING_X, STRIP_PADDING_Y))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(title).strong().color(p.on_strip));
            });
        Frame::none()
            .inner_margin(Margin::same(SECTION_PADDING))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 5.0;
                add(ui);
            });
    });
}

/// A tab button: dark grey, accent-coloured when selected, with an LED beside it that is lit for
/// the selected tab, like the section LEDs on the panel.
pub fn tab<T: PartialEq>(ui: &mut Ui, current: &mut T, value: T, label: &str) {
    let p = palette();
    let selected = *current == value;
    let (fill, text) = if selected {
        (p.accent, p.on_accent)
    } else {
        (p.control_grey, LEGEND)
    };
    let stroke = if selected {
        Stroke::new(1.0_f32, p.accent_text)
    } else {
        Stroke::new(1.0_f32, p.edge)
    };
    let button = egui::Button::new(RichText::new(label).strong().color(text))
        .fill(fill)
        .stroke(stroke);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        led(ui, selected);
        if ui.add(button).clicked() {
            *current = value;
        }
    });
}

/// A small round LED in a black bezel.
pub fn led(ui: &mut Ui, lit: bool) {
    let p = palette();
    let size = egui::Vec2::splat(TAB_LED_BEZEL_RADIUS * 2.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(rect.center(), TAB_LED_BEZEL_RADIUS, LED_BEZEL);
    painter.circle_filled(
        rect.center(),
        TAB_LED_RADIUS,
        if lit { p.led_on } else { p.led_off },
    );
}

/// A glyph's convex polygons, as points on a unit-height grid (y down).
type GlyphPolys = &'static [&'static [(f32, f32)]];

/// "VIRUS" as blocky letters. Pairs of (advance width, polygons).
const LOGO_GLYPHS: &[(f32, GlyphPolys)] = &[
    // V
    (
        0.9,
        &[
            &[(0.0, 0.0), (0.3, 0.0), (0.63, 1.0), (0.27, 1.0)],
            &[(0.6, 0.0), (0.9, 0.0), (0.63, 1.0), (0.27, 1.0)],
        ],
    ),
    // I
    (0.28, &[&[(0.0, 0.0), (0.28, 0.0), (0.28, 1.0), (0.0, 1.0)]]),
    // R
    (
        0.95,
        &[
            &[(0.0, 0.0), (0.26, 0.0), (0.26, 1.0), (0.0, 1.0)],
            &[(0.0, 0.0), (0.78, 0.0), (0.78, 0.25), (0.0, 0.25)],
            &[
                (0.62, 0.0),
                (0.78, 0.0),
                (0.9, 0.12),
                (0.9, 0.62),
                (0.62, 0.62),
            ],
            &[(0.0, 0.4), (0.9, 0.4), (0.9, 0.62), (0.0, 0.62)],
            &[(0.5, 0.6), (0.8, 0.6), (0.95, 1.0), (0.65, 1.0)],
        ],
    ),
    // U
    (
        0.9,
        &[
            &[(0.0, 0.0), (0.26, 0.0), (0.26, 0.85), (0.0, 0.85)],
            &[(0.64, 0.0), (0.9, 0.0), (0.9, 0.85), (0.64, 0.85)],
            &[
                (0.0, 0.75),
                (0.9, 0.75),
                (0.9, 0.88),
                (0.78, 1.0),
                (0.12, 1.0),
                (0.0, 0.88),
            ],
        ],
    ),
    // S
    (
        0.9,
        &[
            &[
                (0.0, 0.12),
                (0.12, 0.0),
                (0.9, 0.0),
                (0.9, 0.24),
                (0.0, 0.24),
            ],
            &[(0.0, 0.12), (0.26, 0.12), (0.26, 0.6), (0.0, 0.6)],
            &[
                (0.0, 0.38),
                (0.78, 0.38),
                (0.9, 0.5),
                (0.9, 0.62),
                (0.0, 0.62),
            ],
            &[(0.64, 0.5), (0.9, 0.5), (0.9, 0.88), (0.64, 0.88)],
            &[
                (0.0, 0.76),
                (0.9, 0.76),
                (0.9, 0.88),
                (0.78, 1.0),
                (0.0, 1.0),
            ],
        ],
    ),
];
const LOGO_LETTER_GAP: f32 = 0.14;
/// Horizontal lean, as a fraction of the letter height.
const LOGO_SLANT: f32 = 0.22;
/// Cap height of the header wordmark, and the black plate's padding around it.
pub const LOGO_HEIGHT: f32 = 18.0;
pub const LOGO_PADDING: egui::Vec2 = egui::vec2(8.0, 4.0);

fn logo_width() -> f32 {
    let advances: f32 = LOGO_GLYPHS.iter().map(|(w, _)| w).sum();
    advances + LOGO_LETTER_GAP * (LOGO_GLYPHS.len() - 1) as f32 + LOGO_SLANT
}

/// The glyph polygons, slanted and scaled to `height`, with the top-left corner at `origin`.
fn logo_polygons(origin: egui::Pos2, height: f32) -> Vec<Vec<egui::Pos2>> {
    let mut x0 = 0.0;
    let mut out = Vec::new();
    for (advance, polys) in LOGO_GLYPHS {
        for poly in *polys {
            out.push(
                poly.iter()
                    .map(|&(x, y)| {
                        origin + egui::vec2((x0 + x + (1.0 - y) * LOGO_SLANT) * height, y * height)
                    })
                    .collect(),
            );
        }
        x0 += advance + LOGO_LETTER_GAP;
    }
    out
}

/// The slanted, blocky "VIRUS" wordmark in white on a black plate, like the badge on the
/// hardware's panel. Drawn with shapes, so it needs no font or image asset.
pub fn logo(ui: &mut Ui) -> egui::Response {
    let size = egui::vec2(logo_width() * LOGO_HEIGHT, LOGO_HEIGHT) + 2.0 * LOGO_PADDING;
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, rounding(), DEEP_BLACK);
    for poly in logo_polygons(rect.min + LOGO_PADDING, LOGO_HEIGHT) {
        painter.add(egui::Shape::convex_polygon(poly, LEGEND, Stroke::NONE));
    }
    response
}

/// The app name beside the logo: spaced-out uppercase monospace, like the LCD's start-up
/// screen and the panel's "VIRUS | CLASSIC" lettering.
pub fn app_name(text: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let format = egui::TextFormat {
        font_id: FontId::monospace(13.0),
        color: palette().on_header,
        extra_letter_spacing: 1.5,
        valign: egui::Align::Center,
        ..Default::default()
    };
    job.append(&text.to_uppercase(), 0.0, format);
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(c: u8) -> f64 {
        let c = f64::from(c) / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn luminance(c: Color32) -> f64 {
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn main_text_pairs_meet_wcag_aa() {
        for kind in ThemeKind::ALL {
            let p = kind.palette();
            let pairs = [
                ("legend on header", p.on_header, p.header),
                ("legend on accent", p.on_accent, p.accent),
                ("legend on hover", Color32::WHITE, p.accent_hover),
                ("legend on pressed", Color32::WHITE, p.pressed),
                (
                    "strong text on control black",
                    Color32::WHITE,
                    p.control_black,
                ),
                ("legend on strip", p.on_strip, p.strip),
                ("legend on chassis", LEGEND, p.chassis),
                ("legend on control grey", LEGEND, p.control_grey),
                ("legend on control black", LEGEND, p.control_black),
                ("dim on chassis", DIM, p.chassis),
                ("dim on control black", DIM, p.control_black),
                ("accent text on chassis", p.accent_text, p.chassis),
                (
                    "accent text on control black",
                    p.accent_text,
                    p.control_black,
                ),
                ("LCD text on LCD", p.lcd_text, p.lcd_bg),
                ("alert on LCD", p.lcd_alert, p.lcd_bg),
                ("danger on chassis", DANGER, p.chassis),
                ("warn on chassis", WARN, p.chassis),
                ("key label on white key", KEY_LABEL, KEY_WHITE),
                ("legend on text field", LEGEND, DEEP_BLACK),
            ];
            for (name, fg, bg) in pairs {
                let ratio = contrast(fg, bg);
                assert!(ratio >= 4.5, "{kind:?} {name}: contrast {ratio:.2} < 4.5");
            }
        }
    }

    #[test]
    fn graphics_meet_non_text_contrast() {
        for kind in ThemeKind::ALL {
            let p = kind.palette();
            for (name, fg, bg) in [
                ("knob arc", p.arc, p.control_black),
                ("knob pointer", KNOB_POINTER, KNOB_BODY),
                ("primary trace", p.arc, SCOPE_BG),
                ("secondary trace", p.led_on, SCOPE_BG),
                ("held key", p.key_held, KEY_WHITE),
                ("lit LED on its bezel", p.led_on, LED_BEZEL),
                ("lit vs unlit LED", p.led_on, p.led_off),
            ] {
                let ratio = contrast(fg, bg);
                assert!(ratio >= 3.0, "{kind:?} {name}: contrast {ratio:.2} < 3.0");
            }
            // The tab LED must stand out from the header, by its bezel or by its own light.
            let led = contrast(LED_BEZEL, p.header).max(contrast(p.led_on, p.header));
            assert!(
                led >= 3.0,
                "{kind:?} tab LED on header: contrast {led:.2} < 3.0"
            );
        }
    }

    #[test]
    fn theme_keys_round_trip_and_unknown_falls_back() {
        for kind in ThemeKind::ALL {
            assert_eq!(ThemeKind::from_key(kind.key()), kind);
        }
        assert_eq!(ThemeKind::from_key("neon"), ThemeKind::Rack);
    }

    #[test]
    fn logo_polygons_are_convex() {
        // egui's convex_polygon tessellates incorrectly for concave shapes.
        for poly in logo_polygons(egui::pos2(0.0, 0.0), LOGO_HEIGHT) {
            let n = poly.len();
            let signs: Vec<f32> = (0..n)
                .map(|i| {
                    let (a, b, c) = (poly[i], poly[(i + 1) % n], poly[(i + 2) % n]);
                    (b - a).x * (c - b).y - (b - a).y * (c - b).x
                })
                .filter(|z| z.abs() > 1e-4)
                .collect();
            assert!(
                signs.iter().all(|z| *z > 0.0) || signs.iter().all(|z| *z < 0.0),
                "not convex: {poly:?}"
            );
        }
    }

    #[test]
    fn renders_frames_headlessly() {
        let ctx = Context::default();
        let mut tab_value = 0;
        for kind in ThemeKind::ALL {
            apply(&ctx, kind);
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::TopBottomPanel::top("h")
                    .frame(header_frame())
                    .show(ctx, |ui| {
                        ui.label(lcd_text("Init"));
                        logo(ui);
                        ui.label(app_name("Rack Editor"));
                    });
                egui::CentralPanel::default().show(ctx, |ui| {
                    section(ui, "Oscillators", None, |ui| {
                        tab(ui, &mut tab_value, 1, "One");
                        ui.add(egui::Slider::new(&mut 5.0_f32, 0.0..=10.0));
                    });
                    lcd_frame().show(ui, |ui| ui.label(lcd_text("x")));
                });
            });
        }
        assert_eq!(tab_value, 0);
    }
}
