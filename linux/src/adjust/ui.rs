//! Editors for adjustment settings: the Properties panel for adjustment layers, and the dialogs of
//! Image > Adjustments. Ranges match the macOS app's panels.

use std::ops::RangeInclusive;

use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, Vec2};

use super::curves::{identity_curve, CurvePoint, MAX_POINTS};
use super::hsv::{ColorRange, HueBand};
use super::levels::{Histogram, LevelRange, LevelsAuto, LevelsChannel, LevelsSettings};
use super::tone::AdjustmentColor;
use super::{Adjustment, AdjustmentKind};

/// The settings editor for the active adjustment layer, in the Properties panel. Returns true
/// when the settings changed.
pub fn properties_ui(ui: &mut egui::Ui, adjustment: &mut Adjustment) -> bool {
    editor(ui, adjustment, None)
}

/// The editor for any adjustment kind; Levels shows `histogram` when there is one.
pub fn editor(ui: &mut egui::Ui, adjustment: &mut Adjustment, histogram: Option<&Histogram>) -> bool {
    let before = adjustment.clone();
    ui.spacing_mut().slider_width = (ui.available_width() - 150.0).clamp(100.0, 260.0);
    match adjustment.kind {
        AdjustmentKind::HueSaturation => hue_saturation(ui, adjustment),
        AdjustmentKind::Levels => {
            levels_editor(ui, &mut adjustment.levels, histogram);
        }
        AdjustmentKind::Curves => {
            curves_editor(ui, &mut adjustment.curves, histogram);
        }
        AdjustmentKind::Exposure => {
            let mut s = adjustment.exposure();
            grid(ui, "exposure", |ui| {
                slider(ui, "Exposure", &mut s.exposure, -20.0..=20.0, 2, "", false);
                slider(ui, "Offset", &mut s.offset, -0.5..=0.5, 4, "", false);
                slider(ui, "Gamma", &mut s.gamma, 0.01..=9.99, 2, "", true);
            });
            if ui.button("Reset").clicked() {
                s = Default::default();
            }
            if s != adjustment.exposure() {
                adjustment.exposure_settings = Some(s.normalized());
            }
        }
        AdjustmentKind::GradientMap => {
            let mut s = adjustment.gradient_map();
            gradient_map(ui, &mut s);
            if s != adjustment.gradient_map() {
                adjustment.gradient_map_settings = Some(s);
            }
        }
        AdjustmentKind::Grain => {
            let mut s = adjustment.grain();
            grid(ui, "grain", |ui| {
                slider(ui, "Amount", &mut s.amount, 0.0..=100.0, 0, "", false);
                slider(ui, "Size", &mut s.size, 0.5..=20.0, 1, " px", true);
                slider(ui, "Roughness", &mut s.roughness, 0.0..=100.0, 0, "", false);
            });
            if ui.button("New Pattern").on_hover_text("Shuffle the grain").clicked() {
                s.seed = rand::random();
            }
            if s != adjustment.grain() {
                adjustment.grain_settings = Some(s.normalized());
            }
        }
        AdjustmentKind::BlackWhite => {
            let mut s = adjustment.black_white();
            let range = -200.0..=300.0;
            grid(ui, "black-white", |ui| {
                slider(ui, "Reds", &mut s.reds, range.clone(), 0, "%", false);
                slider(ui, "Yellows", &mut s.yellows, range.clone(), 0, "%", false);
                slider(ui, "Greens", &mut s.greens, range.clone(), 0, "%", false);
                slider(ui, "Cyans", &mut s.cyans, range.clone(), 0, "%", false);
                slider(ui, "Blues", &mut s.blues, range.clone(), 0, "%", false);
                slider(ui, "Magentas", &mut s.magentas, range.clone(), 0, "%", false);
            });
            ui.checkbox(&mut s.tint, "Tint").on_hover_text("Color the result while keeping its tones, for a sepia or a cyanotype");
            if s.tint {
                grid(ui, "black-white-tint", |ui| {
                    slider(ui, "Hue", &mut s.tint_hue, 0.0..=360.0, 0, "°", false);
                    slider(ui, "Saturation", &mut s.tint_saturation, 0.0..=100.0, 0, "%", false);
                });
            }
            if ui.button("Default").clicked() {
                s = Default::default();
            }
            if s != adjustment.black_white() {
                adjustment.black_white_settings = Some(s);
            }
        }
        AdjustmentKind::ColorBalance => {
            let mut s = adjustment.color_balance();
            let id = ui.id().with("color-balance-tone");
            let mut tone: usize = ui.data(|d| d.get_temp(id)).unwrap_or(1);
            ui.horizontal(|ui| {
                ui.label("Tone");
                for (i, name) in ["Shadows", "Midtones", "Highlights"].iter().enumerate() {
                    ui.selectable_value(&mut tone, i, *name);
                }
            });
            ui.data_mut(|d| d.insert_temp(id, tone));
            let (cr, mg, yb) = match tone {
                0 => (&mut s.shadow_cyan_red, &mut s.shadow_magenta_green, &mut s.shadow_yellow_blue),
                1 => (&mut s.mid_cyan_red, &mut s.mid_magenta_green, &mut s.mid_yellow_blue),
                _ => (&mut s.highlight_cyan_red, &mut s.highlight_magenta_green, &mut s.highlight_yellow_blue),
            };
            grid(ui, "color-balance", |ui| {
                slider(ui, "Cyan / Red", cr, -100.0..=100.0, 0, "", false);
                slider(ui, "Magenta / Green", mg, -100.0..=100.0, 0, "", false);
                slider(ui, "Yellow / Blue", yb, -100.0..=100.0, 0, "", false);
            });
            ui.checkbox(&mut s.preserve_luminosity, "Preserve Luminosity")
                .on_hover_text("Put each pixel's brightness back afterwards, so only the color moves");
            if ui.button("Reset").clicked() {
                s = Default::default();
            }
            if s != adjustment.color_balance() {
                adjustment.color_balance_settings = Some(s);
            }
        }
        AdjustmentKind::GaussianBlur => {
            let mut radius = adjustment.gaussian_radius();
            grid(ui, "gaussian", |ui| {
                slider(ui, "Radius", &mut radius, 0.1..=250.0, 1, " px", true);
            });
            if radius != adjustment.gaussian_radius() {
                adjustment.blur_radius = Some(radius);
            }
        }
        AdjustmentKind::MotionBlur => {
            let mut angle = adjustment.resolved_motion_angle();
            let mut distance = adjustment.resolved_motion_distance();
            grid(ui, "motion", |ui| {
                slider(ui, "Angle", &mut angle, -90.0..=90.0, 0, "°", false);
                slider(ui, "Distance", &mut distance, 1.0..=2000.0, 0, " px", true);
            });
            if angle != adjustment.resolved_motion_angle() {
                adjustment.motion_angle = Some(angle);
            }
            if distance != adjustment.resolved_motion_distance() {
                adjustment.motion_distance = Some(distance);
            }
        }
        AdjustmentKind::AddNoise => {
            let mut amount = adjustment.resolved_noise_amount();
            let mut gaussian = adjustment.noise_gaussian.unwrap_or(false);
            let mut mono = adjustment.noise_monochromatic.unwrap_or(false);
            grid(ui, "noise", |ui| {
                slider(ui, "Amount", &mut amount, 0.1..=400.0, 1, "%", true);
            });
            ui.horizontal(|ui| {
                ui.label("Distribution");
                ui.selectable_value(&mut gaussian, false, "Uniform");
                ui.selectable_value(&mut gaussian, true, "Gaussian");
            });
            ui.checkbox(&mut mono, "Monochromatic");
            if amount != adjustment.resolved_noise_amount() {
                adjustment.noise_amount = Some(amount);
            }
            if gaussian != adjustment.noise_gaussian.unwrap_or(false) {
                adjustment.noise_gaussian = Some(gaussian);
            }
            if mono != adjustment.noise_monochromatic.unwrap_or(false) {
                adjustment.noise_monochromatic = Some(mono);
            }
        }
        AdjustmentKind::Invert => {
            ui.label("Invert has no settings.");
        }
    }
    *adjustment != before
}

/// A two-column grid of labeled controls.
pub fn grid(ui: &mut egui::Ui, id: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id).num_columns(2).spacing([10.0, 6.0]).show(ui, body);
}

/// A labeled slider row in a `grid`. Returns true when the value changed.
pub fn slider(ui: &mut egui::Ui, label: &str, value: &mut f64, range: RangeInclusive<f64>, decimals: usize, suffix: &str, logarithmic: bool) -> bool {
    ui.label(label);
    let response = ui.add(egui::Slider::new(value, range).logarithmic(logarithmic).fixed_decimals(decimals).suffix(suffix));
    ui.end_row();
    response.changed()
}

fn hue_color(hue: f64) -> Color32 {
    egui::ecolor::Hsva::new((hue / 360.0).rem_euclid(1.0) as f32, 1.0, 1.0, 1.0).into()
}

fn hue_saturation(ui: &mut egui::Ui, adjustment: &mut Adjustment) {
    let mut s = adjustment.resolved_hsv();
    let before = s.clone();
    ui.horizontal(|ui| {
        ui.label("Range");
        ui.add_enabled_ui(!s.colorize, |ui| {
            egui::ComboBox::from_id_salt("hsv-range").selected_text(s.range.name()).show_ui(ui, |ui| {
                for range in ColorRange::ALL {
                    ui.selectable_value(&mut s.range, range, range.name());
                }
            });
        });
    });
    let mut current = s.current();
    let (hue_range, saturation_range) = if s.colorize { (0.0..=360.0, 0.0..=100.0) } else { (-180.0..=180.0, -100.0..=100.0) };
    grid(ui, "hsv", |ui| {
        slider(ui, "Hue", &mut current.hue, hue_range, 0, "°", false);
        slider(ui, "Saturation", &mut current.saturation, saturation_range, 0, "", false);
        slider(ui, "Lightness", &mut current.lightness, -100.0..=100.0, 0, "", false);
    });
    if current != s.current() {
        *s.current_mut() = current;
    }
    if s.range != ColorRange::Master && !s.colorize {
        spectrum_editor(ui, &mut s);
        ui.checkbox(&mut s.invert_range, "Apply outside this range instead");
    }
    ui.horizontal(|ui| {
        let mut colorize = s.colorize;
        if ui.checkbox(&mut colorize, "Colorize").changed() {
            // Photoshop starts colorizing at hue 0, saturation 25.
            s = if colorize { super::HueSaturationSettings::colorize_start() } else { Default::default() };
        }
        if ui.button("Reset").clicked() {
            s = if s.colorize { super::HueSaturationSettings::colorize_start() } else { Default::default() };
        }
    });
    if s != before {
        adjustment.hsv_settings = Some(s);
    }
}

/// The selected range's hue band over a spectrum: drag a handle to reshape it, or the band to move
/// it. The lower bar shows where each hue ends up.
fn spectrum_editor(ui: &mut egui::Ui, s: &mut super::HueSaturationSettings) {
    let width = ui.available_width().min(360.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 52.0), Sense::click_and_drag());
    let painter = ui.painter_at(rect.expand(6.0));
    let top = Rect::from_min_size(rect.min, Vec2::new(width, 14.0));
    let bottom = Rect::from_min_size(rect.min + Vec2::new(0.0, 38.0), Vec2::new(width, 14.0));
    let to_x = |hue: f64| rect.left() + (hue / 360.0) as f32 * width;
    let to_hue = |x: f32| (((x - rect.left()) / width) as f64 * 360.0).clamp(0.0, 360.0);
    let steps = 90;
    for i in 0..steps {
        let h0 = i as f64 * 360.0 / steps as f64;
        let x0 = to_x(h0);
        let x1 = to_x(h0 + 360.0 / steps as f64) + 0.5;
        painter.rect_filled(Rect::from_x_y_ranges(x0..=x1, top.y_range()), 0.0, hue_color(h0));
        painter.rect_filled(Rect::from_x_y_ranges(x0..=x1, bottom.y_range()), 0.0, hue_color(s.shifted_hue(h0)));
    }
    // The band's weight as a line between the bars.
    let band = s.band(s.range);
    let middle = Rect::from_x_y_ranges(rect.x_range(), (top.bottom() + 3.0)..=(bottom.top() - 3.0));
    let points: Vec<Pos2> = (0..=180)
        .map(|i| {
            let hue = i as f64 * 2.0;
            let w = s.weight(s.range, hue) as f32;
            Pos2::new(to_x(hue), middle.bottom() - w * middle.height())
        })
        .collect();
    let stroke_color = ui.visuals().strong_text_color();
    painter.add(Shape::line(points, Stroke::new(1.5, stroke_color)));
    let handles = band.handles();
    for (i, h) in handles.iter().enumerate() {
        let x = to_x(*h);
        let filled = i == 1 || i == 2;
        let tri = vec![Pos2::new(x, middle.top() - 1.0), Pos2::new(x - 5.0, middle.top() - 7.0), Pos2::new(x + 5.0, middle.top() - 7.0)];
        if filled {
            painter.add(Shape::convex_polygon(tri, stroke_color, Stroke::NONE));
        } else {
            painter.add(Shape::convex_polygon(tri, Color32::TRANSPARENT, Stroke::new(1.0, stroke_color)));
        }
    }
    painter.rect_stroke(top, 0.0, Stroke::new(1.0, Color32::from_gray(90)), egui::StrokeKind::Inside);
    painter.rect_stroke(bottom, 0.0, Stroke::new(1.0, Color32::from_gray(90)), egui::StrokeKind::Inside);

    let id = response.id.with("drag");
    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            let nearest = handles
                .iter()
                .enumerate()
                .map(|(i, h)| (i, (to_x(*h) - pos.x).abs()))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|(_, d)| *d < 8.0)
                .map(|(i, _)| i);
            let grab = nearest.unwrap_or(4);
            ui.data_mut(|d| d.insert_temp(id, (grab, to_hue(pos.x))));
        }
    }
    if response.dragged() {
        if let (Some(pos), Some((grab, last))) = (response.interact_pointer_pos(), ui.data(|d| d.get_temp::<(usize, f64)>(id))) {
            let hue = to_hue(pos.x);
            let mut band: HueBand = s.band(s.range);
            if grab < 4 {
                band.set_handle(grab, hue);
            } else {
                band = band.rotated(hue - last);
            }
            s.bands.insert(s.range, band);
            ui.data_mut(|d| d.insert_temp(id, (grab, hue)));
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|d| d.remove::<(usize, f64)>(id));
    }
    response.on_hover_text("Drag a handle to reshape the range, or the band to move it");
}

fn channel_color(channel: LevelsChannel, dark: bool) -> Color32 {
    match channel {
        LevelsChannel::Rgb => {
            if dark {
                Color32::from_gray(200)
            } else {
                Color32::from_gray(60)
            }
        }
        LevelsChannel::Red => Color32::from_rgb(230, 70, 70),
        LevelsChannel::Green => Color32::from_rgb(70, 200, 90),
        LevelsChannel::Blue => Color32::from_rgb(80, 130, 240),
    }
}

fn channel_picker(ui: &mut egui::Ui, channel: &mut LevelsChannel) {
    ui.horizontal(|ui| {
        ui.label("Channel");
        for c in LevelsChannel::ALL {
            ui.selectable_value(channel, c, c.name());
        }
    });
}

fn draw_histogram(painter: &egui::Painter, rect: Rect, bins: &[f64; 256], color: Color32) {
    let scale = Histogram::display_scale(bins);
    if scale <= 0.0 {
        return;
    }
    let bar = rect.width() / 256.0;
    for (i, b) in bins.iter().enumerate() {
        let h = ((b / scale).min(1.0) as f32) * rect.height();
        if h <= 0.0 {
            continue;
        }
        let x = rect.left() + i as f32 * bar;
        painter.rect_filled(Rect::from_min_max(Pos2::new(x, rect.bottom() - h), Pos2::new(x + bar.max(1.0), rect.bottom())), 0.0, color);
    }
}

fn triangle(painter: &egui::Painter, x: f32, y: f32, fill: Color32, outline: Color32) {
    let points = vec![Pos2::new(x, y), Pos2::new(x - 6.0, y + 10.0), Pos2::new(x + 6.0, y + 10.0)];
    painter.add(Shape::convex_polygon(points, fill, Stroke::new(1.0, outline)));
}

/// Levels: channel, histogram, input black/gamma/white handles and output range.
pub fn levels_editor(ui: &mut egui::Ui, levels: &mut LevelsSettings, histogram: Option<&Histogram>) -> bool {
    let before = levels.clone();
    while levels.ranges.len() < 4 {
        levels.ranges.push(LevelRange::default());
    }
    channel_picker(ui, &mut levels.channel);
    let index = levels.channel.index();
    let mut range = levels.ranges[index].normalized();
    let dark = ui.visuals().dark_mode;
    let width = ui.available_width().min(320.0);

    // Histogram and input handles.
    let hist_height = if histogram.is_some() { 110.0 } else { 0.0 };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, hist_height + 30.0), Sense::click_and_drag());
    let painter = ui.painter_at(rect.expand(2.0));
    let plot = Rect::from_min_size(rect.min, Vec2::new(width, hist_height));
    if let Some(h) = histogram {
        painter.rect_filled(plot, 2.0, ui.visuals().extreme_bg_color);
        draw_histogram(&painter, plot, &h.bins[index], channel_color(levels.channel, dark));
    }
    let bar = Rect::from_min_size(Pos2::new(rect.left(), plot.bottom() + 4.0), Vec2::new(width, 10.0));
    gradient_bar(&painter, bar, Color32::BLACK, channel_end_color(levels.channel));
    let to_x = |v: f64| rect.left() + (v / 255.0) as f32 * width;
    let to_v = |x: f32| (((x - rect.left()) / width) as f64 * 255.0).clamp(0.0, 255.0);
    let gamma_x = |r: &LevelRange| to_x(r.black + (r.white - r.black) * 0.5f64.powf(r.gamma));
    let handle_y = bar.bottom() + 2.0;
    let outline = ui.visuals().strong_text_color();
    triangle(&painter, to_x(range.black), handle_y, Color32::BLACK, outline);
    triangle(&painter, gamma_x(&range), handle_y, Color32::from_gray(128), outline);
    triangle(&painter, to_x(range.white), handle_y, Color32::WHITE, outline);
    let id = response.id.with("levels-input");
    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            let candidates = [to_x(range.black), gamma_x(&range), to_x(range.white)];
            let nearest = (0..3).min_by(|a, b| (candidates[*a] - pos.x).abs().total_cmp(&(candidates[*b] - pos.x).abs())).unwrap();
            ui.data_mut(|d| d.insert_temp(id, nearest));
        }
    }
    if response.dragged() {
        if let (Some(pos), Some(handle)) = (response.interact_pointer_pos(), ui.data(|d| d.get_temp::<usize>(id))) {
            let v = to_v(pos.x).round();
            match handle {
                0 => range.black = v.min(range.white - 1.0),
                1 => {
                    let t = ((v - range.black) / (range.white - range.black)).clamp(0.01, 0.99);
                    range.gamma = ((t.ln() / 0.5f64.ln()) * 100.0).round() / 100.0;
                }
                _ => range.white = v.max(range.black + 1.0),
            }
        }
    }
    ui.horizontal(|ui| {
        ui.label("Input");
        let white = range.white;
        ui.add(egui::DragValue::new(&mut range.black).range(0.0..=(white - 1.0).max(0.0)).speed(1.0).max_decimals(0));
        ui.add(egui::DragValue::new(&mut range.gamma).range(0.1..=9.99).speed(0.01).fixed_decimals(2));
        let black = range.black;
        ui.add(egui::DragValue::new(&mut range.white).range((black + 1.0)..=255.0).speed(1.0).max_decimals(0));
    });

    // Output range.
    let (out_rect, out_response) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::click_and_drag());
    let painter = ui.painter_at(out_rect.expand(2.0));
    let out_bar = Rect::from_min_size(out_rect.min, Vec2::new(width, 10.0));
    gradient_bar(&painter, out_bar, Color32::BLACK, channel_end_color(levels.channel));
    let to_x = |v: f64| out_rect.left() + (v / 255.0) as f32 * width;
    let to_v = |x: f32| (((x - out_rect.left()) / width) as f64 * 255.0).clamp(0.0, 255.0);
    triangle(&painter, to_x(range.output_black), out_bar.bottom() + 2.0, Color32::BLACK, outline);
    triangle(&painter, to_x(range.output_white), out_bar.bottom() + 2.0, Color32::WHITE, outline);
    let out_id = out_response.id.with("levels-output");
    if out_response.drag_started() {
        if let Some(pos) = out_response.interact_pointer_pos() {
            let to_black = (to_x(range.output_black) - pos.x).abs();
            let to_white = (to_x(range.output_white) - pos.x).abs();
            ui.data_mut(|d| d.insert_temp(out_id, to_white < to_black));
        }
    }
    if out_response.dragged() {
        if let (Some(pos), Some(white)) = (out_response.interact_pointer_pos(), ui.data(|d| d.get_temp::<bool>(out_id))) {
            let v = to_v(pos.x).round();
            if white {
                range.output_white = v;
            } else {
                range.output_black = v;
            }
        }
    }
    ui.horizontal(|ui| {
        ui.label("Output");
        ui.add(egui::DragValue::new(&mut range.output_black).range(0.0..=255.0).speed(1.0).max_decimals(0));
        ui.add(egui::DragValue::new(&mut range.output_white).range(0.0..=255.0).speed(1.0).max_decimals(0));
    });
    levels.ranges[index] = range.normalized();
    ui.horizontal(|ui| {
        if let Some(h) = histogram {
            ui.label("Auto");
            for (name, mode, tip) in [
                ("Contrast", LevelsAuto::Contrast, "Stretch the tones, keeping the colors' balance"),
                ("Color", LevelsAuto::Color, "Stretch each channel on its own"),
                ("Neutral", LevelsAuto::Neutral, "Stretch each channel and neutralize the midtones"),
            ] {
                if ui.button(name).on_hover_text(tip).clicked() {
                    let channel = levels.channel;
                    *levels = LevelsSettings::automatic(mode, h);
                    levels.channel = channel;
                }
            }
        }
        if ui.button("Reset").clicked() {
            let channel = levels.channel;
            *levels = LevelsSettings::default();
            levels.channel = channel;
        }
    });
    *levels != before
}

fn channel_end_color(channel: LevelsChannel) -> Color32 {
    match channel {
        LevelsChannel::Rgb => Color32::WHITE,
        LevelsChannel::Red => Color32::from_rgb(255, 0, 0),
        LevelsChannel::Green => Color32::from_rgb(0, 255, 0),
        LevelsChannel::Blue => Color32::from_rgb(0, 0, 255),
    }
}

fn gradient_bar(painter: &egui::Painter, rect: Rect, from: Color32, to: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), from);
    mesh.colored_vertex(rect.right_top(), to);
    mesh.colored_vertex(rect.right_bottom(), to);
    mesh.colored_vertex(rect.left_bottom(), from);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_gray(90)), egui::StrokeKind::Inside);
}

#[derive(Clone, Copy, Default)]
struct CurveDrag {
    dragging: Option<usize>,
    selected: Option<usize>,
}

/// Curves: click to add a point, drag to move it, drag an inner point off the graph to remove it.
pub fn curves_editor(ui: &mut egui::Ui, curves: &mut super::CurvesSettings, histogram: Option<&Histogram>) -> bool {
    let before = curves.clone();
    while curves.channels.len() < 4 {
        curves.channels.push(identity_curve());
    }
    let state_id = ui.id().with("curves-state");
    let mut state: CurveDrag = ui.data(|d| d.get_temp(state_id)).unwrap_or_default();
    let channel = curves.channel;
    channel_picker(ui, &mut curves.channel);
    if curves.channel != channel {
        state = CurveDrag::default();
    }
    let index = curves.channel.index();
    if !super::curves::is_valid_curve(&curves.channels[index]) {
        curves.channels[index] = identity_curve();
    }
    let side = ui.available_width().clamp(160.0, 300.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::click_and_drag());
    let painter = ui.painter_at(rect.expand(5.0));
    let dark = ui.visuals().dark_mode;
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    if let Some(h) = histogram {
        let tint = channel_color(curves.channel, dark).gamma_multiply(0.25);
        draw_histogram(&painter, rect, &h.bins[index], tint);
    }
    let grid_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    for i in 1..4 {
        let f = i as f32 / 4.0;
        painter.line_segment([Pos2::new(rect.left() + f * side, rect.top()), Pos2::new(rect.left() + f * side, rect.bottom())], Stroke::new(1.0, grid_color));
        painter.line_segment([Pos2::new(rect.left(), rect.top() + f * side), Pos2::new(rect.right(), rect.top() + f * side)], Stroke::new(1.0, grid_color));
    }
    painter.line_segment([rect.left_bottom(), rect.right_top()], Stroke::new(1.0, grid_color));
    let to_screen = |x: f64, y: f64| Pos2::new(rect.left() + (x / 255.0) as f32 * side, rect.bottom() - (y / 255.0) as f32 * side);
    let to_value = |p: Pos2| {
        (
            (((p.x - rect.left()) / side) as f64 * 255.0).clamp(0.0, 255.0),
            (((rect.bottom() - p.y) / side) as f64 * 255.0).clamp(0.0, 255.0),
        )
    };
    // The other channels' curves, faintly, when they've been changed.
    for (c, ch) in LevelsChannel::ALL.iter().enumerate() {
        if c == index || curves.channels[c].iter().all(|p| p.x == p.y) {
            continue;
        }
        let points: Vec<Pos2> = (0..=64).map(|i| i as f64 * 255.0 / 64.0).map(|x| to_screen(x, curves.value(x, c))).collect();
        painter.add(Shape::line(points, Stroke::new(1.0, channel_color(*ch, dark).gamma_multiply(0.5))));
    }

    // Interaction.
    if response.drag_started() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let (x, y) = to_value(pos);
            let points = &mut curves.channels[index];
            let nearest = (0..points.len())
                .map(|i| (i, ((points[i].x - x).powi(2) + (points[i].y - y).powi(2)).sqrt()))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            match nearest {
                Some((i, d)) if d < 14.0 => {
                    state.dragging = Some(i);
                    state.selected = Some(i);
                }
                _ if points.len() < MAX_POINTS && x > 1.0 && x < 254.0 && points.iter().all(|p| (p.x - x).abs() > 1.0) => {
                    points.push(CurvePoint { x, y });
                    points.sort_by(|a, b| a.x.total_cmp(&b.x));
                    let i = points.iter().position(|p| p.x == x);
                    state.dragging = i;
                    state.selected = i;
                }
                _ => {}
            }
        }
    }
    if response.dragged() {
        if let (Some(pos), Some(i)) = (response.interact_pointer_pos(), state.dragging) {
            let points = &mut curves.channels[index];
            if i < points.len() {
                let inner = i > 0 && i < points.len() - 1;
                if inner && !rect.expand(24.0).contains(pos) {
                    // Dragged off the graph: gone, as in Photoshop.
                    points.remove(i);
                    state.dragging = None;
                    state.selected = None;
                } else {
                    let (x, y) = to_value(pos);
                    points[i].y = y.round();
                    if inner {
                        let (lo, hi) = (points[i - 1].x + 1.0, points[i + 1].x - 1.0);
                        points[i].x = if lo <= hi { x.round().clamp(lo, hi) } else { (points[i - 1].x + points[i + 1].x) / 2.0 };
                    }
                }
            }
        }
    }
    if response.drag_stopped() {
        state.dragging = None;
    }

    // The curve and its points.
    let points = &curves.channels[index];
    let line: Vec<Pos2> = (0..=255).map(|x| to_screen(x as f64, curves.value(x as f64, index))).collect();
    painter.add(Shape::line(line, Stroke::new(2.0, channel_color(curves.channel, dark))));
    for (i, p) in points.iter().enumerate() {
        let center = to_screen(p.x, p.y);
        let selected = state.selected == Some(i);
        let fill = if selected { ui.visuals().selection.bg_fill } else { ui.visuals().strong_text_color() };
        painter.circle(center, 4.0, fill, Stroke::new(1.0, ui.visuals().extreme_bg_color));
    }
    response.on_hover_text("Click to add a point. Drag to adjust; drag a point off the graph to remove it.");

    ui.horizontal(|ui| {
        let points = &mut curves.channels[index];
        let len = points.len();
        match state.selected.filter(|i| *i < len) {
            Some(i) => {
                let (lo, hi) = if i == 0 || i == len - 1 { (points[i].x, points[i].x) } else { (points[i - 1].x + 1.0, (points[i + 1].x - 1.0).max(points[i - 1].x + 1.0)) };
                ui.label("Input");
                ui.add_enabled(i > 0 && i < len - 1, egui::DragValue::new(&mut points[i].x).range(lo..=hi).max_decimals(0));
                ui.label("Output");
                ui.add(egui::DragValue::new(&mut points[i].y).range(0.0..=255.0).max_decimals(0));
                if ui.add_enabled(i > 0 && i < len - 1, egui::Button::new("Remove Point")).clicked() {
                    points.remove(i);
                    state.selected = None;
                }
            }
            None => {
                ui.label("Click the graph to add a point.");
            }
        }
    });
    if ui.button("Reset Curve").clicked() {
        curves.channels[index] = identity_curve();
        state = CurveDrag::default();
    }
    ui.data_mut(|d| d.insert_temp(state_id, state));
    *curves != before
}

fn color_button(ui: &mut egui::Ui, color: &mut AdjustmentColor) -> bool {
    let mut rgb = [color.red as f32, color.green as f32, color.blue as f32];
    let changed = egui::color_picker::color_edit_button_rgb(ui, &mut rgb).changed();
    if changed {
        *color = AdjustmentColor { red: rgb[0] as f64, green: rgb[1] as f64, blue: rgb[2] as f64 };
    }
    changed
}

fn gradient_map(ui: &mut egui::Ui, s: &mut super::GradientMapSettings) {
    let (dark, light) = s.ends();
    let width = ui.available_width().min(300.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 18.0), Sense::hover());
    let c = |v: [f32; 3]| Color32::from_rgb((v[0] * 255.0) as u8, (v[1] * 255.0) as u8, (v[2] * 255.0) as u8);
    gradient_bar(ui.painter(), rect, c(dark), c(light));
    ui.horizontal(|ui| {
        ui.label("Shadows");
        color_button(ui, &mut s.shadows);
        ui.label("Highlights");
        color_button(ui, &mut s.highlights);
        if ui.small_button(crate::ui::icons::SWAP).on_hover_text("Swap the colors").clicked() {
            std::mem::swap(&mut s.shadows, &mut s.highlights);
        }
    });
    ui.checkbox(&mut s.reversed, "Reverse");
}
