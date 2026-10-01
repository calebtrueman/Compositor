//! The canvas: renders the document in tiles at a level of detail matched to the zoom, passes
//! pointer input to the current tool, and draws guides, the selection and tool overlays.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, ColorImage, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use image::GrayImage;

use crate::doc::GuideAxis;
use crate::project::Project;
use crate::render::{self, Region, RenderCache};
use crate::tools::{Colors, PointerEvent, PointerPhase, ToolCtx, ToolKind, Tools};

const TILE: usize = 256;

/// Maps document pixels to screen points.
#[derive(Clone, Copy, Debug)]
pub struct ViewTransform {
    /// Screen position of the document's top-left corner.
    pub origin: Pos2,
    /// Screen points per document pixel.
    pub zoom: f32,
}

impl ViewTransform {
    pub fn to_screen(&self, x: f64, y: f64) -> Pos2 {
        Pos2::new(self.origin.x + x as f32 * self.zoom, self.origin.y + y as f32 * self.zoom)
    }
    pub fn to_doc(&self, p: Pos2) -> (f64, f64) {
        (((p.x - self.origin.x) / self.zoom) as f64, ((p.y - self.origin.y) / self.zoom) as f64)
    }
    pub fn doc_rect(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::from_min_max(self.to_screen(x0, y0), self.to_screen(x1, y1))
    }
}

struct Tile {
    texture: TextureHandle,
    dirty: bool,
    nearest: bool,
}

pub struct CanvasView {
    pub zoom: f32,
    /// Offset of the document's center from the viewport's center, in points.
    pub pan: Vec2,
    fit_requested: bool,
    zoom_request: Option<f32>,
    pending_zoom_about: Option<((f64, f64), f32)>,
    tiles: HashMap<(u32, i64, i64), Tile>,
    pub viewport: Rect,
    pressing: bool,
    panning: bool,
    press_origin: (f64, f64),
    last_doc_pos: Option<(f64, f64)>,
    selection_outline: Option<(usize, Arc<Vec<[(f32, f32); 2]>>)>,
    pub show_grid: bool,
    pub show_guides: bool,
    pub show_selection: bool,
}

impl Default for CanvasView {
    fn default() -> Self {
        CanvasView {
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit_requested: true,
            zoom_request: None,
            pending_zoom_about: None,
            tiles: HashMap::new(),
            viewport: Rect::NOTHING,
            pressing: false,
            panning: false,
            press_origin: (0.0, 0.0),
            last_doc_pos: None,
            selection_outline: None,
            show_grid: false,
            show_guides: true,
            show_selection: true,
        }
    }
}

impl CanvasView {
    pub fn request_fit(&mut self) {
        self.fit_requested = true;
    }

    pub fn request_zoom(&mut self, zoom: f32) {
        self.zoom_request = Some(zoom);
    }

    pub fn transform(&self, doc_w: u32, doc_h: u32) -> ViewTransform {
        let center = self.viewport.center() + self.pan;
        ViewTransform {
            origin: Pos2::new(center.x - doc_w as f32 * self.zoom / 2.0, center.y - doc_h as f32 * self.zoom / 2.0),
            zoom: self.zoom,
        }
    }

    /// Zooms by `factor` keeping the document point under `doc` fixed on screen.
    pub fn zoom_about_doc(&mut self, doc: (f64, f64), factor: f32) {
        self.pending_zoom_about = Some((doc, factor));
    }

    pub fn last_pointer(&self) -> Option<(f64, f64)> {
        self.last_doc_pos
    }

    fn set_zoom_keeping(&mut self, new_zoom: f32, screen: Pos2, doc_w: u32, doc_h: u32) {
        let new_zoom = new_zoom.clamp(0.01, 64.0);
        let view = self.transform(doc_w, doc_h);
        let (dx, dy) = view.to_doc(screen);
        self.zoom = new_zoom;
        let after = self.transform(doc_w, doc_h).to_screen(dx, dy);
        self.pan += screen - after;
    }

    pub fn invalidate_tiles(&mut self, rect: Option<crate::project::DocRect>) {
        match rect {
            None => self.tiles.values_mut().for_each(|t| t.dirty = true),
            Some((x0, y0, x1, y1)) => {
                for ((lod, tx, ty), tile) in self.tiles.iter_mut() {
                    let span = (TILE as i64) * (*lod as i64);
                    let (tx0, ty0) = (tx * span, ty * span);
                    if tx0 < x1 && tx0 + span > x0 && ty0 < y1 && ty0 + span > y0 {
                        tile.dirty = true;
                    }
                }
            }
        }
    }

    pub fn drop_tiles(&mut self) {
        self.tiles.clear();
    }
}

pub struct CanvasOutput {
    pub hovered_doc: Option<(f64, f64)>,
}

/// Shows the canvas for `project` filling the available space.
pub fn show(
    ui: &mut egui::Ui,
    project: &mut Project,
    tools: &mut Tools,
    colors: &mut Colors,
    cache: &RenderCache,
    status: &mut Option<String>,
    temporary_hand: bool,
) -> CanvasOutput {
    let rect = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let (doc_w, doc_h) = (project.doc.width, project.doc.height);
    project.view.viewport = rect;

    if project.view.fit_requested && rect.width() > 10.0 {
        let zoom = ((rect.width() - 40.0) / doc_w as f32).min((rect.height() - 40.0) / doc_h as f32);
        project.view.zoom = zoom.clamp(0.01, 16.0);
        project.view.pan = Vec2::ZERO;
        project.view.fit_requested = false;
    }
    if let Some(zoom) = project.view.zoom_request.take() {
        project.view.set_zoom_keeping(zoom, rect.center(), doc_w, doc_h);
    }
    if let Some((doc, factor)) = project.view.pending_zoom_about.take() {
        let screen = project.view.transform(doc_w, doc_h).to_screen(doc.0, doc.1);
        let zoom = project.view.zoom * factor;
        project.view.set_zoom_keeping(zoom, screen, doc_w, doc_h);
    }

    // Scrolling pans; Ctrl+scroll and pinch zoom around the pointer.
    let hover_pos = ui.input(|i| i.pointer.hover_pos()).filter(|p| rect.contains(*p));
    if let Some(pos) = hover_pos {
        let (scroll, zoom_delta, ctrl) =
            ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.command || i.modifiers.ctrl));
        if zoom_delta != 1.0 {
            let zoom = project.view.zoom * zoom_delta;
            project.view.set_zoom_keeping(zoom, pos, doc_w, doc_h);
        } else if ctrl && scroll.y != 0.0 {
            let zoom = project.view.zoom * (scroll.y / 200.0).exp();
            project.view.set_zoom_keeping(zoom, pos, doc_w, doc_h);
        } else if scroll != Vec2::ZERO {
            project.view.pan += scroll;
        }
    }

    // Pointer: panning with the Hand tool, Space or the middle button; everything else to the tool.
    let view = project.view.transform(doc_w, doc_h);
    let (primary_pressed, primary_down, primary_released, middle_down, modifiers, pointer_pos, delta) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.pointer.primary_released(),
            i.pointer.middle_down(),
            i.modifiers,
            i.pointer.interact_pos(),
            i.pointer.delta(),
        )
    });
    let hand = temporary_hand || tools.current == ToolKind::Hand;
    if (middle_down && response.hovered()) || (hand && primary_down && (response.hovered() || project.view.panning)) {
        project.view.panning = true;
        project.view.pan += delta;
    } else {
        project.view.panning = false;
    }
    let doc_pos = pointer_pos.map(|p| view.to_doc(p));
    let mut ctx_zoom = project.view.zoom;
    if !hand {
        let mut send = |phase: PointerPhase, pos: (f64, f64), project: &mut Project, tools: &mut Tools| {
            let event = PointerEvent { phase, pos, press_origin: project.view.press_origin, modifiers, pressure: 1.0 };
            let mut ctx = ToolCtx { project, colors, cache, zoom: ctx_zoom, status };
            tools.current_mut().pointer(&event, &mut ctx);
        };
        if let Some(pos) = doc_pos {
            if primary_pressed && response.hovered() && !project.view.pressing {
                project.view.pressing = true;
                project.view.press_origin = pos;
                if response.double_clicked() {
                    send(PointerPhase::DoubleClick, pos, project, tools);
                }
                send(PointerPhase::Press, pos, project, tools);
            } else if project.view.pressing && primary_down {
                if project.view.last_doc_pos != Some(pos) {
                    send(PointerPhase::Drag, pos, project, tools);
                }
            } else if project.view.pressing && (primary_released || !primary_down) {
                project.view.pressing = false;
                send(PointerPhase::Release, pos, project, tools);
            } else if response.hovered() && project.view.last_doc_pos != Some(pos) {
                send(PointerPhase::Hover, pos, project, tools);
            }
            if response.double_clicked() && !primary_pressed {
                send(PointerPhase::DoubleClick, pos, project, tools);
            }
        } else if project.view.pressing && !primary_down {
            project.view.pressing = false;
            let pos = project.view.last_doc_pos.unwrap_or(project.view.press_origin);
            send(PointerPhase::Release, pos, project, tools);
        }
        ctx_zoom = project.view.zoom;
        let _ = ctx_zoom;
    }
    project.view.last_doc_pos = doc_pos;

    // Redraw whatever edits touched.
    for rect in std::mem::take(&mut project.dirty) {
        project.view.invalidate_tiles(rect);
    }

    let view = project.view.transform(project.doc.width, project.doc.height);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    let doc_rect = view.doc_rect(0.0, 0.0, project.doc.width as f64, project.doc.height as f64);
    painter.rect_filled(doc_rect.expand(1.0), 0.0, Color32::from_black_alpha(90));
    draw_tiles(ui, &painter, project, cache, &view, rect);
    draw_decorations(&painter, project, &view, rect);

    let hover_doc = hover_pos.map(|p| view.to_doc(p));
    tools.current().overlay(&painter, &view, project, hover_doc);

    if let Some(pos) = hover_doc {
        let icon = if hand {
            if project.view.panning { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab }
        } else {
            tools.current().cursor(project, pos, modifiers)
        };
        ui.ctx().set_cursor_icon(icon);
    }
    if project.view.pressing || project.view.panning {
        ui.ctx().request_repaint();
    }
    CanvasOutput { hovered_doc: hover_doc }
}

fn lod_for(zoom: f32, ppp: f32) -> u32 {
    let device = zoom * ppp;
    let mut lod = 1u32;
    while (lod * 2) as f32 * device <= 1.0 && lod < 64 {
        lod *= 2;
    }
    lod
}

fn draw_tiles(ui: &egui::Ui, painter: &egui::Painter, project: &mut Project, cache: &RenderCache, view: &ViewTransform, clip: Rect) {
    let ppp = ui.ctx().pixels_per_point();
    let lod = lod_for(view.zoom, ppp);
    let nearest = view.zoom * ppp > 1.0;
    let (doc_w, doc_h) = (project.doc.width as i64, project.doc.height as i64);
    let out_w = (doc_w + lod as i64 - 1) / lod as i64;
    let out_h = (doc_h + lod as i64 - 1) / lod as i64;
    let (vx0, vy0) = view.to_doc(clip.min);
    let (vx1, vy1) = view.to_doc(clip.max);
    let span = (TILE * lod as usize) as f64;
    let tx0 = (vx0 / span).floor().max(0.0) as i64;
    let ty0 = (vy0 / span).floor().max(0.0) as i64;
    let tx1 = ((vx1 / span).ceil() as i64).min((out_w + TILE as i64 - 1) / TILE as i64);
    let ty1 = ((vy1 / span).ceil() as i64).min((out_h + TILE as i64 - 1) / TILE as i64);

    let deadline = Instant::now() + Duration::from_millis(80);
    let mut pending = false;
    let options = if nearest { TextureOptions::NEAREST } else { TextureOptions::LINEAR };
    for ty in ty0..ty1 {
        for tx in tx0..tx1 {
            let key = (lod, tx, ty);
            let needs = project.view.tiles.get(&key).is_none_or(|t| t.dirty || t.nearest != nearest);
            let x = tx * TILE as i64;
            let y = ty * TILE as i64;
            let w = (TILE as i64).min(out_w - x).max(0) as usize;
            let h = (TILE as i64).min(out_h - y).max(0) as usize;
            if w == 0 || h == 0 {
                continue;
            }
            if needs {
                if Instant::now() > deadline && project.view.tiles.contains_key(&key) {
                    pending = true;
                } else {
                    let region = Region { x, y, width: w, height: h, scale: lod as f64 };
                    let buffer = render::composite(&project.doc, region, cache);
                    let image = to_color_image(&buffer, region, project.doc.width, project.doc.height);
                    match project.view.tiles.get_mut(&key) {
                        Some(tile) if tile.nearest == nearest => {
                            tile.texture.set(image, options);
                            tile.dirty = false;
                        }
                        _ => {
                            let texture = ui.ctx().load_texture(format!("tile-{lod}-{tx}-{ty}"), image, options);
                            project.view.tiles.insert(key, Tile { texture, dirty: false, nearest });
                        }
                    }
                }
            }
            if let Some(tile) = project.view.tiles.get(&key) {
                let x0 = (x * lod as i64) as f64;
                let y0 = (y * lod as i64) as f64;
                let x1 = (((x + w as i64) * lod as i64) as f64).min(doc_w as f64);
                let y1 = (((y + h as i64) * lod as i64) as f64).min(doc_h as f64);
                let screen = view.doc_rect(x0, y0, x1, y1);
                let uv = Rect::from_min_max(
                    Pos2::ZERO,
                    Pos2::new(((x1 - x0) / lod as f64 / w as f64) as f32, ((y1 - y0) / lod as f64 / h as f64) as f32),
                );
                painter.image(tile.texture.id(), screen, uv, Color32::WHITE);
            }
        }
    }
    if pending {
        ui.ctx().request_repaint();
    }
    // Forget tiles at other zoom levels once there are many.
    if project.view.tiles.len() > 600 {
        project.view.tiles.retain(|(l, _, _), _| *l == lod);
    }
}

/// Composites the tile over a checkerboard into an opaque texture.
fn to_color_image(buffer: &render::Buffer, region: Region, doc_w: u32, doc_h: u32) -> ColorImage {
    let mut pixels = vec![Color32::BLACK; buffer.width * buffer.height];
    let lod = region.scale;
    for j in 0..buffer.height {
        for i in 0..buffer.width {
            let p = buffer.px[j * buffer.width + i];
            let gx = ((region.x + i as i64) as f64 * lod) as i64;
            let gy = ((region.y + j as i64) as f64 * lod) as i64;
            let check = if ((gx / 8) + (gy / 8)) % 2 == 0 { 1.0 } else { 0.8 };
            let k = 1.0 - p[3];
            let r = p[0] + check * k;
            let g = p[1] + check * k;
            let b = p[2] + check * k;
            let outside = gx >= doc_w as i64 || gy >= doc_h as i64;
            pixels[j * buffer.width + i] = if outside {
                Color32::TRANSPARENT
            } else {
                Color32::from_rgb(render::to_u8(r), render::to_u8(g), render::to_u8(b))
            };
        }
    }
    ColorImage::new([buffer.width, buffer.height], pixels)
}

fn draw_decorations(painter: &egui::Painter, project: &mut Project, view: &ViewTransform, clip: Rect) {
    let doc = &project.doc;
    // Pixel grid when zoomed far in.
    if view.zoom >= 12.0 {
        let (x0, y0) = view.to_doc(clip.min);
        let (x1, y1) = view.to_doc(clip.max);
        let stroke = Stroke::new(1.0, Color32::from_black_alpha(40));
        let xs = x0.max(0.0).floor() as i64;
        let xe = x1.min(doc.width as f64).ceil() as i64;
        let ys = y0.max(0.0).floor() as i64;
        let ye = y1.min(doc.height as f64).ceil() as i64;
        for x in xs..=xe {
            painter.line_segment([view.to_screen(x as f64, ys as f64), view.to_screen(x as f64, ye as f64)], stroke);
        }
        for y in ys..=ye {
            painter.line_segment([view.to_screen(xs as f64, y as f64), view.to_screen(xe as f64, y as f64)], stroke);
        }
    }
    if project.view.show_grid {
        let spacing = 100.0;
        let stroke = Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 160, 255, 70));
        let mut x = spacing;
        while x < doc.width as f64 {
            painter.line_segment([view.to_screen(x, 0.0), view.to_screen(x, doc.height as f64)], stroke);
            x += spacing;
        }
        let mut y = spacing;
        while y < doc.height as f64 {
            painter.line_segment([view.to_screen(0.0, y), view.to_screen(doc.width as f64, y)], stroke);
            y += spacing;
        }
    }
    if project.view.show_guides {
        let stroke = Stroke::new(1.0, Color32::from_rgb(0, 220, 255));
        for guide in &doc.guides {
            match guide.axis {
                GuideAxis::Horizontal => {
                    let y = view.to_screen(0.0, guide.position).y;
                    painter.line_segment([Pos2::new(clip.min.x, y), Pos2::new(clip.max.x, y)], stroke);
                }
                GuideAxis::Vertical => {
                    let x = view.to_screen(guide.position, 0.0).x;
                    painter.line_segment([Pos2::new(x, clip.min.y), Pos2::new(x, clip.max.y)], stroke);
                }
            }
        }
    }
    if project.view.show_selection {
        if let Some(selection) = &doc.selection {
            let key = Arc::as_ptr(&selection.mask) as usize;
            let outline = match &project.view.selection_outline {
                Some((k, o)) if *k == key => o.clone(),
                _ => {
                    let o = Arc::new(outline_segments(&selection.mask));
                    project.view.selection_outline = Some((key, o.clone()));
                    o
                }
            };
            draw_marching_ants(painter, &outline, view, painter.ctx().input(|i| i.time));
            painter.ctx().request_repaint_after(Duration::from_millis(120));
        }
    }
}

/// Boundary edges between selected (≥ 128) and unselected pixels, in document coordinates.
pub fn outline_segments(mask: &GrayImage) -> Vec<[(f32, f32); 2]> {
    let (w, h) = mask.dimensions();
    let on = |x: i64, y: i64| x >= 0 && y >= 0 && x < w as i64 && y < h as i64 && mask.get_pixel(x as u32, y as u32)[0] >= 128;
    let mut segments = Vec::new();
    for y in 0..=h as i64 {
        let mut run: Option<i64> = None;
        for x in 0..=w as i64 {
            let edge = x < w as i64 && on(x, y) != on(x, y - 1);
            match (edge, run) {
                (true, None) => run = Some(x),
                (false, Some(start)) => {
                    segments.push([(start as f32, y as f32), (x as f32, y as f32)]);
                    run = None;
                }
                _ => {}
            }
        }
    }
    for x in 0..=w as i64 {
        let mut run: Option<i64> = None;
        for y in 0..=h as i64 {
            let edge = y < h as i64 && on(x, y) != on(x - 1, y);
            match (edge, run) {
                (true, None) => run = Some(y),
                (false, Some(start)) => {
                    segments.push([(x as f32, start as f32), (x as f32, y as f32)]);
                    run = None;
                }
                _ => {}
            }
        }
    }
    segments
}

fn draw_marching_ants(painter: &egui::Painter, segments: &[[(f32, f32); 2]], view: &ViewTransform, time: f64) {
    let white = Stroke::new(1.0, Color32::WHITE);
    let black = Stroke::new(1.0, Color32::BLACK);
    let offset = ((time * 8.0) % 8.0) as f32;
    let clip = painter.clip_rect();
    for [a, b] in segments {
        let pa = view.to_screen(a.0 as f64, a.1 as f64);
        let pb = view.to_screen(b.0 as f64, b.1 as f64);
        if !clip.intersects(Rect::from_two_pos(pa, pb).expand(1.0)) {
            continue;
        }
        painter.line_segment([pa, pb], white);
        let shapes = egui::Shape::dashed_line_with_offset(&[pa, pb], black, &[4.0], &[4.0], offset);
        painter.extend(shapes);
    }
}
