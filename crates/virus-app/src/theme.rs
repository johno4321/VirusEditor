//! The Virus Rack look: signal red and chassis black (SPEC §5.10).
//! Every colour, rounding and spacing constant lives here so the look can be tuned in one place.

use egui::{Color32, Context, FontId, Frame, Margin, RichText, Rounding, Stroke, Ui, Visuals};

// --- Palette -------------------------------------------------------------------------------
/// Signal red (SPEC's #D7262B lightened so black legends reach WCAG AA, 4.5:1): header bar, section title strips, selected tab, active arcs.
pub const RED: Color32 = Color32::from_rgb(0xE8, 0x35, 0x2F);
/// Dark red: hover states and the "VIRUS" watermark.
pub const DARK_RED: Color32 = Color32::from_rgb(0x9E, 0x1B, 0x1F);
/// Chassis black: window and panel background.
pub const CHASSIS: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
/// Control black: knob bodies, section bodies.
pub const CONTROL_BLACK: Color32 = Color32::from_rgb(0x1E, 0x1E, 0x1E);
/// Control grey: buttons and combo boxes.
pub const CONTROL_GREY: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x2A);
/// Edges of controls and separators.
pub const EDGE: Color32 = Color32::from_rgb(0x4A, 0x4A, 0x4A);
/// Deepest black: text fields, curve displays; also legends on red.
pub const DEEP_BLACK: Color32 = Color32::from_rgb(0x0A, 0x0A, 0x0A);
/// Legend on red.
pub const ON_RED: Color32 = DEEP_BLACK;
/// Legend on black.
pub const LEGEND: Color32 = Color32::from_rgb(0xE8, 0xE8, 0xE8);
/// Secondary text on black.
pub const DIM: Color32 = Color32::from_rgb(0x9A, 0x9A, 0x9A);
/// Red that is legible as text on black (the signal red itself is too dark for small text).
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0xFF, 0x5A, 0x5F);
/// LED amber: Rx/Tx LEDs, knob needle highlights, second curve trace.
pub const LED_AMBER: Color32 = Color32::from_rgb(0xFF, 0xB0, 0x00);
/// An unlit LED.
pub const LED_OFF: Color32 = Color32::from_rgb(0x3A, 0x2A, 0x08);
/// Black ring around a section LED, so amber reads on the red header (amber on red alone is ~2.3:1).
pub const LED_BEZEL: Color32 = DEEP_BLACK;
/// LCD background (dark slate) and text.
pub const LCD_BG: Color32 = Color32::from_rgb(0x1C, 0x2A, 0x33);
pub const LCD_TEXT: Color32 = Color32::from_rgb(0xCF, 0xE3, 0xEA);
/// Warnings / errors, legible on black and on the LCD.
pub const WARN: Color32 = LED_AMBER;
pub const DANGER: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x5E);

// Knob
pub const KNOB_BODY: Color32 = Color32::from_rgb(0x1A, 0x1A, 0x1A);
pub const KNOB_BORDER: Color32 = EDGE;
pub const KNOB_TRACK: Color32 = CONTROL_GREY;
pub const KNOB_ARC: Color32 = RED;
pub const KNOB_POINTER: Color32 = Color32::from_rgb(0xF4, 0xF4, 0xF4);

// Curve displays
pub const SCOPE_BG: Color32 = DEEP_BLACK;
pub const SCOPE_BORDER: Color32 = EDGE;
pub const SCOPE_GRID: Color32 = CONTROL_BLACK;
pub const TRACE_PRIMARY: Color32 = RED;
pub const TRACE_SECONDARY: Color32 = LED_AMBER;

// Piano
pub const KEY_WHITE: Color32 = LEGEND;
pub const KEY_BLACK: Color32 = CHASSIS;
pub const KEY_HELD: Color32 = RED;
pub const KEY_OUTLINE: Color32 = EDGE;
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
/// The outlined "VIRUS" wordmark in the header (V2): text, size, outline width, free space
/// required on each side.
pub const WATERMARK_TEXT: &str = "VIRUS";
pub const WATERMARK_SIZE: f32 = 28.0;
pub const WATERMARK_OUTLINE: f32 = 1.5;
pub const WATERMARK_MARGIN: f32 = 12.0;

fn rounding() -> Rounding {
    Rounding::same(ROUNDING)
}

/// Installs the red-and-black visuals and spacing.
pub fn apply(ctx: &Context) {
    ctx.set_visuals(visuals());
    ctx.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 5.0);
        style.spacing.button_padding = egui::vec2(8.0, 3.0);
    });
}

pub fn visuals() -> Visuals {
    let mut v = Visuals::dark();
    v.dark_mode = true;
    v.override_text_color = None;
    v.panel_fill = CHASSIS;
    v.window_fill = CHASSIS;
    v.window_stroke = Stroke::new(1.0_f32, RED);
    v.window_rounding = Rounding::same(ROUNDING + 2.0);
    v.menu_rounding = rounding();
    v.extreme_bg_color = DEEP_BLACK;
    v.faint_bg_color = CONTROL_BLACK;
    v.code_bg_color = CONTROL_BLACK;
    v.hyperlink_color = ACCENT_TEXT;
    v.warn_fg_color = WARN;
    v.error_fg_color = DANGER;
    v.selection.bg_fill = RED;
    v.selection.stroke = Stroke::new(1.0_f32, ON_RED);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = CONTROL_BLACK;
    w.noninteractive.weak_bg_fill = CONTROL_BLACK;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, CONTROL_GREY);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, LEGEND);

    w.inactive.bg_fill = CONTROL_GREY;
    w.inactive.weak_bg_fill = CONTROL_GREY;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, EDGE);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, LEGEND);

    w.hovered.bg_fill = DARK_RED;
    w.hovered.weak_bg_fill = DARK_RED;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, RED);
    w.hovered.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);

    w.active.bg_fill = RED;
    w.active.weak_bg_fill = RED;
    w.active.bg_stroke = Stroke::new(1.0_f32, ACCENT_TEXT);
    w.active.fg_stroke = Stroke::new(1.5_f32, ON_RED);

    w.open.bg_fill = CONTROL_GREY;
    w.open.weak_bg_fill = CONTROL_GREY;
    w.open.bg_stroke = Stroke::new(1.0_f32, RED);
    w.open.fg_stroke = Stroke::new(1.0_f32, LEGEND);

    for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.rounding = rounding();
    }
    v
}

/// The red header bar.
pub fn header_frame() -> Frame {
    Frame::none()
        .fill(RED)
        .stroke(Stroke::new(1.0_f32, DARK_RED))
        .inner_margin(Margin::symmetric(HEADER_PADDING_X, HEADER_PADDING_Y))
}

/// A dark box with a red outline, the body of a panel section.
pub fn section_frame() -> Frame {
    Frame::none()
        .fill(CONTROL_BLACK)
        .stroke(Stroke::new(1.0_f32, DARK_RED))
        .rounding(rounding())
        .inner_margin(Margin::ZERO)
}

/// The LCD readout box.
pub fn lcd_frame() -> Frame {
    Frame::none()
        .fill(LCD_BG)
        .stroke(Stroke::new(1.0_f32, DEEP_BLACK))
        .rounding(rounding())
        .inner_margin(Margin::symmetric(LCD_PADDING_X, LCD_PADDING_Y))
}

pub fn lcd_text(text: impl Into<String>) -> RichText {
    RichText::new(text).font(FontId::monospace(15.0)).strong().color(LCD_TEXT)
}

/// A section: red title strip with black legend over a dark body. `width` defaults to filling the row.
pub fn section(ui: &mut Ui, title: &str, width: Option<f32>, add: impl FnOnce(&mut Ui)) {
    section_frame().show(ui, |ui| {
        ui.set_width(width.unwrap_or_else(|| ui.available_width()));
        ui.spacing_mut().item_spacing.y = 0.0;
        let top = Rounding { nw: ROUNDING, ne: ROUNDING, sw: 0.0, se: 0.0 };
        Frame::none().fill(RED).rounding(top).inner_margin(Margin::symmetric(STRIP_PADDING_X, STRIP_PADDING_Y)).show(
            ui,
            |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(title).strong().color(ON_RED));
            },
        );
        Frame::none().inner_margin(Margin::same(SECTION_PADDING)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 5.0;
            add(ui);
        });
    });
}

/// A tab button: dark grey, red when selected, with an amber LED beside it that is lit for the
/// selected tab, like the section LEDs on the panel.
pub fn tab<T: PartialEq>(ui: &mut Ui, current: &mut T, value: T, label: &str) {
    let selected = *current == value;
    let (fill, text) = if selected { (RED, ON_RED) } else { (CONTROL_GREY, LEGEND) };
    let stroke = if selected { Stroke::new(1.0_f32, ACCENT_TEXT) } else { Stroke::new(1.0_f32, EDGE) };
    let button = egui::Button::new(RichText::new(label).strong().color(text)).fill(fill).stroke(stroke);
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
    let size = egui::Vec2::splat(TAB_LED_BEZEL_RADIUS * 2.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(rect.center(), TAB_LED_BEZEL_RADIUS, LED_BEZEL);
    painter.circle_filled(rect.center(), TAB_LED_RADIUS, if lit { LED_AMBER } else { LED_OFF });
}

/// Whether a wordmark of `text` size fits in `gap` with [`WATERMARK_MARGIN`] on each side.
pub fn watermark_fits(gap: egui::Rect, text: egui::Vec2) -> bool {
    gap.is_positive() && gap.width() >= text.x + 2.0 * (WATERMARK_MARGIN + WATERMARK_OUTLINE)
}

/// Paints the outlined, slanted "VIRUS" wordmark in dark red, centred in `gap` (empty header
/// space), and only if it fits, so it never sits behind text or controls. Purely decorative.
pub fn watermark(ui: &Ui, gap: egui::Rect) {
    let layout = |color: Color32| {
        let mut job = egui::text::LayoutJob::default();
        let format = egui::TextFormat {
            font_id: FontId::proportional(WATERMARK_SIZE),
            color,
            italics: true,
            extra_letter_spacing: 3.0,
            ..Default::default()
        };
        job.append(WATERMARK_TEXT, 0.0, format);
        ui.fonts(|f| f.layout_job(job))
    };
    let outline = layout(DARK_RED);
    if !watermark_fits(gap, outline.size()) {
        return;
    }
    let top_left = gap.center() - outline.size() / 2.0;
    let painter = ui.painter();
    let w = WATERMARK_OUTLINE;
    for (dx, dy) in [(-w, -w), (0.0, -w), (w, -w), (-w, 0.0), (w, 0.0), (-w, w), (0.0, w), (w, w)] {
        painter.galley(top_left + egui::vec2(dx, dy), outline.clone(), DARK_RED);
    }
    painter.galley(top_left, layout(RED), RED);
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
        let pairs = [
            ("legend on red", ON_RED, RED),
            ("legend on dark red", Color32::WHITE, DARK_RED),
            ("legend on black", LEGEND, CHASSIS),
            ("legend on control grey", LEGEND, CONTROL_GREY),
            ("legend on control black", LEGEND, CONTROL_BLACK),
            ("dim on chassis", DIM, CHASSIS),
            ("dim on control black", DIM, CONTROL_BLACK),
            ("accent text on chassis", ACCENT_TEXT, CHASSIS),
            ("accent text on control black", ACCENT_TEXT, CONTROL_BLACK),
            ("LCD text on LCD", LCD_TEXT, LCD_BG),
            ("danger on LCD", DANGER, LCD_BG),
            ("danger on chassis", DANGER, CHASSIS),
            ("warn on chassis", WARN, CHASSIS),
            ("key label on white key", KEY_LABEL, KEY_WHITE),
            ("legend on text field", LEGEND, DEEP_BLACK),
        ];
        for (name, fg, bg) in pairs {
            let ratio = contrast(fg, bg);
            assert!(ratio >= 4.5, "{name}: contrast {ratio:.2} < 4.5");
        }
    }

    #[test]
    fn graphics_meet_non_text_contrast() {
        for (name, fg, bg) in [
            ("knob arc", KNOB_ARC, CONTROL_BLACK),
            ("knob pointer", KNOB_POINTER, KNOB_BODY),
            ("primary trace", TRACE_PRIMARY, SCOPE_BG),
            ("secondary trace", TRACE_SECONDARY, SCOPE_BG),
            ("held key", KEY_HELD, KEY_WHITE),
            ("lit LED on its bezel", LED_AMBER, LED_BEZEL),
            ("LED bezel on the red header", LED_BEZEL, RED),
            ("lit vs unlit LED", LED_AMBER, LED_OFF),
        ] {
            let ratio = contrast(fg, bg);
            assert!(ratio >= 3.0, "{name}: contrast {ratio:.2} < 3.0");
        }
    }

    #[test]
    fn watermark_needs_room_on_both_sides() {
        let text = egui::vec2(100.0, 30.0);
        let room = 100.0 + 2.0 * (WATERMARK_MARGIN + WATERMARK_OUTLINE);
        let gap = |w: f32| egui::Rect::from_min_size(egui::pos2(50.0, 0.0), egui::vec2(w, 24.0));
        assert!(watermark_fits(gap(room), text));
        assert!(!watermark_fits(gap(room - 1.0), text));
        assert!(!watermark_fits(egui::Rect::NOTHING, text));
    }

    #[test]
    fn renders_frames_headlessly() {
        let ctx = Context::default();
        apply(&ctx);
        let mut tab_value = 0;
        for _ in 0..3 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::TopBottomPanel::top("h").frame(header_frame()).show(ctx, |ui| {
                    ui.label(lcd_text("Init"));
                    watermark(ui, ui.available_rect_before_wrap());
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
