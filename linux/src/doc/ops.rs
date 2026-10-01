//! Layer-stack edits shared by the Layers panel and the menus.

use std::sync::Arc;

use image::{GrayImage, Luma, RgbaImage};

use super::{Document, Id, Layer, LayerMask, LayerTransform, MaskPixels};
use crate::render::{self, Region, RenderCache};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Above,
    Below,
    /// At the top of a folder.
    Into,
}

/// Moves `ids` (with their contents) next to `target`. Refuses to move a folder into itself.
pub fn move_layers(doc: &mut Document, ids: &[Id], target: Id, placement: Placement) -> bool {
    if ids.contains(&target) {
        return false;
    }
    for id in ids {
        if doc.subtree(*id).contains(&target) {
            return false;
        }
    }
    let Some(target_layer) = doc.layer(target) else { return false };
    let new_parent = match placement {
        Placement::Into if target_layer.is_group => Some(target),
        Placement::Into => target_layer.parent,
        _ => target_layer.parent,
    };
    // Keep the moved layers' stacking order.
    let mut moving: Vec<Id> = ids.iter().copied().filter(|id| !ids.iter().any(|o| doc.ancestors(*id).contains(o))).collect();
    moving.sort_by_key(|id| doc.index_of(*id));
    let mut records = Vec::new();
    for id in &moving {
        let index = doc.index_of(*id).unwrap();
        let mut layer = doc.layers.remove(index);
        layer.parent = new_parent;
        records.push(layer);
    }
    let position = match placement {
        Placement::Above => doc.index_of(target).map(|i| i + 1),
        Placement::Into if doc.layer(target).is_some_and(|l| l.is_group) => doc.index_of(target),
        Placement::Into => doc.index_of(target).map(|i| i + 1),
        Placement::Below => {
            let subtree = doc.subtree(target);
            subtree.iter().filter_map(|id| doc.index_of(*id)).min()
        }
    };
    let Some(position) = position else { return false };
    for (offset, layer) in records.into_iter().enumerate() {
        doc.layers.insert(position + offset, layer);
    }
    release_broken_clips(doc);
    doc.normalize_order();
    true
}

/// A clipped layer has to sit directly above its base, among the base's siblings; anything
/// else lets go of the base.
pub fn release_broken_clips(doc: &mut Document) {
    let snapshot: Vec<(Id, Option<Id>, Option<Id>)> = doc.layers.iter().map(|l| (l.id, l.parent, l.clip_source)).collect();
    for (id, parent, source) in snapshot {
        let Some(source) = source else { continue };
        let valid = doc.layer(source).is_some_and(|s| s.parent == parent && !s.is_group);
        if !valid {
            if let Some(layer) = doc.layer_mut(id) {
                layer.clip_source = None;
            }
        }
    }
}

/// Moves the selection one step up (`up`) or down among its siblings.
pub fn shift_layers(doc: &mut Document, up: bool) -> bool {
    let ids = doc.selected_ids();
    let Some(&anchor) = (if up { ids.last() } else { ids.first() }) else { return false };
    let Some(layer) = doc.layer(anchor) else { return false };
    let siblings = doc.children(layer.parent);
    let Some(pos) = siblings.iter().position(|s| *s == anchor) else { return false };
    let neighbor = if up { siblings.get(pos + 1) } else { pos.checked_sub(1).and_then(|p| siblings.get(p)) };
    match neighbor {
        Some(n) => move_layers(doc, &ids, *n, if up { Placement::Above } else { Placement::Below }),
        None => false,
    }
}

pub fn duplicate_layers(doc: &mut Document, ids: &[Id]) -> Vec<Id> {
    let mut new_ids = Vec::new();
    let mut ordered = ids.to_vec();
    ordered.sort_by_key(|id| doc.index_of(*id));
    for id in ordered {
        if ids.iter().any(|o| doc.ancestors(id).contains(o)) {
            continue;
        }
        let members = doc.subtree(id);
        let mut map = std::collections::HashMap::new();
        for m in &members {
            map.insert(*m, Id::new());
        }
        let mut copies: Vec<Layer> = Vec::new();
        for m in &members {
            let mut copy = doc.layer(*m).unwrap().clone();
            copy.id = map[m];
            if let Some(p) = copy.parent {
                if let Some(np) = map.get(&p) {
                    copy.parent = Some(*np);
                }
            }
            if let Some(s) = copy.clip_source {
                if let Some(ns) = map.get(&s) {
                    copy.clip_source = Some(*ns);
                }
            }
            if *m == id {
                copy.name = copy_name(&copy.name);
            }
            copies.push(copy);
        }
        let insert_at = doc.index_of(id).map(|i| i + 1).unwrap_or(doc.layers.len());
        for (offset, copy) in copies.into_iter().enumerate() {
            doc.layers.insert(insert_at + offset, copy);
        }
        new_ids.push(map[&id]);
    }
    doc.normalize_order();
    if let Some(last) = new_ids.last() {
        doc.active = Some(*last);
        doc.selected = new_ids.clone();
    }
    new_ids
}

fn copy_name(name: &str) -> String {
    if name.ends_with(" copy") {
        format!("{name} 2")
    } else if let Some((base, n)) = name.rsplit_once(" copy ").and_then(|(b, n)| n.parse::<u32>().ok().map(|n| (b, n))) {
        format!("{base} copy {}", n + 1)
    } else {
        format!("{name} copy")
    }
}

/// Puts the selected layers into a new folder where the topmost of them was.
pub fn group_layers(doc: &mut Document, ids: &[Id]) -> Option<Id> {
    let mut ids: Vec<Id> = ids.iter().copied().filter(|id| !ids.iter().any(|o| doc.ancestors(*id).contains(o))).collect();
    if ids.is_empty() {
        return None;
    }
    ids.sort_by_key(|id| doc.index_of(*id));
    let top = *ids.last().unwrap();
    let parent = doc.layer(top)?.parent;
    let mut group = Layer::new_group(doc.unique_name("Group"), doc.full_canvas_transform());
    group.parent = parent;
    let group_id = group.id;
    let index = doc.index_of(top)?;
    doc.layers.insert(index + 1, group);
    for id in &ids {
        if let Some(layer) = doc.layer_mut(*id) {
            layer.parent = Some(group_id);
        }
    }
    release_broken_clips(doc);
    doc.normalize_order();
    doc.active = Some(group_id);
    doc.selected.clear();
    Some(group_id)
}

/// Dissolves a folder, keeping its contents where they were. A folder's opacity is folded into
/// its children so nothing changes on screen.
pub fn ungroup(doc: &mut Document, id: Id) -> bool {
    let Some(group) = doc.layer(id).filter(|l| l.is_group).cloned() else { return false };
    for layer in &mut doc.layers {
        if layer.parent == Some(id) {
            layer.parent = group.parent;
            layer.opacity *= group.opacity;
            if !group.visible {
                layer.visible = false;
            }
        }
    }
    let children = doc.children(group.parent);
    let _ = children;
    doc.layers.retain(|l| l.id != id);
    doc.active = doc.layers.iter().rev().find(|l| l.parent == group.parent).map(|l| l.id).or(doc.active);
    if doc.active == Some(id) {
        doc.active = doc.layers.last().map(|l| l.id);
    }
    doc.normalize_order();
    true
}

/// Renders some layers of `doc` (with their folder context) into a canvas-sized image.
pub fn render_layers(doc: &Document, ids: &[Id], cache: &RenderCache) -> RgbaImage {
    let mut temp = doc.clone();
    let keep: Vec<Id> = ids.iter().flat_map(|id| doc.subtree(*id)).collect();
    for layer in &mut temp.layers {
        if !keep.contains(&layer.id) && !layer.is_group {
            layer.visible = false;
        }
    }
    render::composite(&temp, Region::full(&temp), cache).to_rgba8()
}

/// The smallest rectangle holding every non-transparent pixel, or `None` if there are none.
pub fn alpha_bounds(image: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = image.dimensions();
    let mut b = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if image.get_pixel(x, y)[3] > 0 {
                b.0 = b.0.min(x);
                b.1 = b.1.min(y);
                b.2 = b.2.max(x + 1);
                b.3 = b.3.max(y + 1);
            }
        }
    }
    (b.0 != u32::MAX).then_some(b)
}

/// A pixel layer holding `image` cropped to its visible content, placed at its canvas position.
pub fn layer_from_canvas_image(name: &str, image: RgbaImage) -> Layer {
    match alpha_bounds(&image) {
        Some((x0, y0, x1, y1)) => {
            let cropped = image::imageops::crop_imm(&image, x0, y0, x1 - x0, y1 - y0).to_image();
            Layer::new_pixel(name, cropped, LayerTransform::rect(x0 as f64, y0 as f64, (x1 - x0) as f64, (y1 - y0) as f64))
        }
        None => Layer::blank(name, LayerTransform::rect(0.0, 0.0, image.width() as f64, image.height() as f64)),
    }
}

/// Merges the active layer into the one below it (or a folder into one layer).
pub fn merge_down(doc: &mut Document, cache: &RenderCache) -> bool {
    let Some(active) = doc.active_layer().cloned() else { return false };
    if active.is_group {
        return merge_group(doc, active.id, cache);
    }
    let siblings = doc.children(active.parent);
    let Some(pos) = siblings.iter().position(|s| *s == active.id) else { return false };
    let Some(&below_id) = pos.checked_sub(1).and_then(|p| siblings.get(p)) else { return false };
    let below = doc.layer(below_id).unwrap().clone();
    if below.is_group || below.adjustment.is_some() {
        return false;
    }
    let mut temp = doc.clone();
    temp.layers = vec![below.clone(), active.clone()];
    for l in &mut temp.layers {
        l.parent = None;
    }
    temp.layers[0].opacity = 1.0;
    temp.layers[0].blend = super::BlendMode::Normal;
    temp.layers[1].clip_source = active.clip_source.filter(|s| *s == below_id);
    let image = render::composite(&temp, Region::full(&temp), cache).to_rgba8();
    let mut merged = layer_from_canvas_image(&below.name, image);
    merged.id = below.id;
    merged.parent = below.parent;
    merged.opacity = below.opacity;
    merged.blend = below.blend;
    merged.visible = below.visible;
    merged.clip_source = below.clip_source;
    let index = doc.index_of(below_id).unwrap();
    doc.layers[index] = merged;
    doc.remove_layers(&[active.id]);
    doc.active = Some(below_id);
    true
}

pub fn merge_group(doc: &mut Document, id: Id, cache: &RenderCache) -> bool {
    let Some(group) = doc.layer(id).cloned() else { return false };
    let mut temp = doc.clone();
    let members = doc.subtree(id);
    temp.layers.retain(|l| members.contains(&l.id));
    if let Some(g) = temp.layer_mut(id) {
        g.parent = None;
        g.visible = true;
    }
    let image = render::composite(&temp, Region::full(&temp), cache).to_rgba8();
    let mut merged = layer_from_canvas_image(&group.name, image);
    merged.parent = group.parent;
    let new_id = merged.id;
    let index = doc.index_of(id).unwrap();
    doc.layers.insert(index + 1, merged);
    doc.remove_layers(&[id]);
    doc.normalize_order();
    doc.active = Some(new_id);
    true
}

/// Merges every visible layer into one, keeping hidden layers.
pub fn merge_visible(doc: &mut Document, cache: &RenderCache) {
    let image = render::composite(doc, Region::full(doc), cache).to_rgba8();
    let visible: Vec<Id> =
        doc.layers.iter().filter(|l| !l.is_group && doc.is_effectively_visible(l.id)).map(|l| l.id).collect();
    doc.remove_layers(&visible);
    let empty_groups: Vec<Id> = doc
        .layers
        .iter()
        .filter(|l| l.is_group && doc.subtree(l.id).len() == 1)
        .map(|l| l.id)
        .collect();
    doc.remove_layers(&empty_groups);
    let merged = layer_from_canvas_image("Merged", image);
    let id = merged.id;
    doc.layers.push(merged);
    doc.active = Some(id);
}

pub fn flatten(doc: &mut Document, cache: &RenderCache) {
    let image = render::composite(doc, Region::full(doc), cache).to_rgba8();
    let mut opaque = RgbaImage::from_pixel(doc.width, doc.height, image::Rgba([255, 255, 255, 255]));
    image::imageops::overlay(&mut opaque, &image, 0, 0);
    let layer = Layer::new_pixel("Background", opaque, doc.full_canvas_transform());
    doc.active = Some(layer.id);
    doc.selected.clear();
    doc.layers = vec![layer];
}

/// Adds a mask: revealing everything, or showing only the selection when there is one.
pub fn add_mask(doc: &mut Document, id: Id, hide_all: bool) -> bool {
    let selection = doc.selection.clone();
    let Some(layer) = doc.layer(id) else { return false };
    if layer.mask.is_some() {
        return false;
    }
    let (w, h) = layer.pixel_size();
    let transform = layer.transform;
    let pixels = match selection {
        Some(sel) if !hide_all => {
            let Some(inverse) = transform.pixel_to_document(w, h).inverse() else { return false };
            let _ = inverse;
            let to_doc = transform.pixel_to_document(w, h);
            let mut mask = GrayImage::new(w, h);
            for (x, y, p) in mask.enumerate_pixels_mut() {
                let (dx, dy) = to_doc.apply(x as f64 + 0.5, y as f64 + 0.5);
                p[0] = sel.coverage(dx.floor() as i64, dy.floor() as i64);
            }
            MaskPixels::Pixels(Arc::new(mask))
        }
        _ => MaskPixels::Uniform(if hide_all { 0 } else { 255 }),
    };
    let layer = doc.layer_mut(id).unwrap();
    layer.mask = Some(LayerMask { pixels, enabled: true, linked: true, placement: None });
    true
}

/// Writes the mask into the layer's alpha and removes it.
pub fn apply_mask(doc: &mut Document, id: Id) -> bool {
    let Some(layer) = doc.layer_mut(id) else { return false };
    let Some(mask) = layer.mask.take() else { return false };
    let Some(image) = layer.image.as_mut() else { return true };
    let image = Arc::make_mut(image);
    let (w, h) = image.dimensions();
    let gray = expand_mask(&mask.pixels, w, h);
    for (p, m) in image.pixels_mut().zip(gray.pixels()) {
        p[3] = ((p[3] as u32 * m[0] as u32 + 127) / 255) as u8;
    }
    layer.rasterized();
    true
}

/// The mask as `w`×`h` pixels (resampling a mask of another size).
pub fn expand_mask(pixels: &MaskPixels, w: u32, h: u32) -> GrayImage {
    match pixels {
        MaskPixels::Uniform(v) => GrayImage::from_pixel(w, h, Luma([*v])),
        MaskPixels::Pixels(gray) if gray.dimensions() == (w, h) => (**gray).clone(),
        MaskPixels::Pixels(gray) => image::imageops::resize(&**gray, w, h, image::imageops::FilterType::Triangle),
    }
}

/// Toggles a clipping mask between the active layer and the sibling below it.
pub fn toggle_clip(doc: &mut Document, id: Id) -> bool {
    let Some(layer) = doc.layer(id) else { return false };
    if layer.is_group {
        return false;
    }
    if layer.clip_source.is_some() {
        doc.layer_mut(id).unwrap().clip_source = None;
        return true;
    }
    let siblings = doc.children(layer.parent);
    let Some(pos) = siblings.iter().position(|s| *s == id) else { return false };
    let Some(&below) = pos.checked_sub(1).and_then(|p| siblings.get(p)) else { return false };
    let Some(below_layer) = doc.layer(below) else { return false };
    if below_layer.is_group {
        return false;
    }
    // Clip to the same base as the layer below if it is itself clipped.
    let base = below_layer.clip_source.unwrap_or(below);
    doc.layer_mut(id).unwrap().clip_source = Some(base);
    true
}

/// Turns a text, shape, adjustment-free blank layer into plain pixels at canvas size.
pub fn rasterize(doc: &mut Document, id: Id) -> bool {
    let Some(layer) = doc.layer_mut(id) else { return false };
    if layer.is_group || layer.adjustment.is_some() {
        return false;
    }
    layer.rasterized();
    true
}

/// Copies the merged or layer pixels inside the selection, as a canvas-sized image.
pub fn copy_pixels(doc: &Document, merged: bool, cache: &RenderCache) -> Option<RgbaImage> {
    let region = Region::full(doc);
    let buffer = if merged {
        render::composite(doc, region, cache)
    } else {
        render::place_layer(doc.active_layer()?, region, cache)?
    };
    let mut image = buffer.to_rgba8();
    if let Some(selection) = &doc.selection {
        for (x, y, p) in image.enumerate_pixels_mut() {
            let c = selection.coverage(x as i64, y as i64) as u32;
            p[3] = ((p[3] as u32 * c + 127) / 255) as u8;
        }
    }
    Some(image)
}

/// Clears the selected pixels of the active layer (or its mask).
pub fn clear_selection_pixels(doc: &mut Document) -> bool {
    let Some(selection) = doc.selection.clone() else { return false };
    let Some(layer) = doc.active_layer_mut() else { return false };
    let Some(image) = layer.image.as_mut() else { return false };
    let to_doc = layer.transform.pixel_to_document(image.width(), image.height());
    let image = Arc::make_mut(image);
    for (x, y, p) in image.enumerate_pixels_mut() {
        let (dx, dy) = to_doc.apply(x as f64 + 0.5, y as f64 + 0.5);
        let c = selection.coverage(dx.floor() as i64, dy.floor() as i64) as u32;
        p[3] = ((p[3] as u32 * (255 - c) + 127) / 255) as u8;
    }
    layer.rasterized();
    true
}

/// Fills the selection (or the whole layer) of the active layer with a color.
pub fn fill(doc: &mut Document, color: [u8; 4]) -> bool {
    let selection = doc.selection.clone();
    let (w, h) = (doc.width, doc.height);
    let canvas = doc.full_canvas_transform();
    let Some(layer) = doc.active_layer_mut() else { return false };
    if layer.is_group || layer.adjustment.is_some() {
        return false;
    }
    if layer.image.is_none() {
        layer.image = Some(Arc::new(RgbaImage::new(w, h)));
        layer.transform = canvas;
    }
    let image = layer.image.as_mut().unwrap();
    let to_doc = layer.transform.pixel_to_document(image.width(), image.height());
    let image = Arc::make_mut(image);
    for (x, y, p) in image.enumerate_pixels_mut() {
        let c = match &selection {
            Some(sel) => {
                let (dx, dy) = to_doc.apply(x as f64 + 0.5, y as f64 + 0.5);
                sel.coverage(dx.floor() as i64, dy.floor() as i64) as f32 / 255.0
            }
            None => 1.0,
        };
        if c <= 0.0 {
            continue;
        }
        let a = color[3] as f32 / 255.0 * c;
        let da = p[3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        for i in 0..3 {
            let v = (color[i] as f32 * a + p[i] as f32 * da * (1.0 - a)) / oa.max(1e-6);
            p[i] = v.round().clamp(0.0, 255.0) as u8;
        }
        p[3] = (oa * 255.0).round() as u8;
    }
    layer.rasterized();
    true
}

/// Sets the active layer's mask to `gray` inside the selection (everywhere without one).
pub fn fill_mask(doc: &mut Document, gray: u8) -> bool {
    let selection = doc.selection.clone();
    let Some(layer) = doc.active_layer_mut() else { return false };
    let transform = layer.transform;
    let (w, h) = layer.pixel_size();
    let Some(mask) = layer.mask.as_mut() else { return false };
    let Some(selection) = selection else {
        mask.pixels = MaskPixels::Uniform(gray);
        return true;
    };
    let placement = if mask.linked { transform } else { mask.placement.unwrap_or(transform) };
    let (mw, mh) = match &mask.pixels {
        MaskPixels::Uniform(_) => (w, h),
        MaskPixels::Pixels(p) => p.dimensions(),
    };
    let mut pixels = expand_mask(&mask.pixels, mw, mh);
    let to_doc = placement.pixel_to_document(mw, mh);
    for (x, y, p) in pixels.enumerate_pixels_mut() {
        let (dx, dy) = to_doc.apply(x as f64 + 0.5, y as f64 + 0.5);
        let c = selection.coverage(dx.floor() as i64, dy.floor() as i64) as u32;
        p[0] = ((p[0] as u32 * (255 - c) + gray as u32 * c + 127) / 255) as u8;
    }
    mask.pixels = MaskPixels::Pixels(Arc::new(pixels));
    true
}

/// Moves every layer, unlinked mask and guide by the affine `map` and sets a new canvas size:
/// rotating and flipping the canvas, and cropping (a translation).
pub fn transform_canvas(doc: &mut Document, map: &super::Affine, width: u32, height: u32) {
    let mirrored = map.a * map.d - map.b * map.c < 0.0;
    let place = |t: &LayerTransform| {
        let flip = if mirrored { !t.flip_x } else { t.flip_x };
        let placed = t.unit_to_document().then(map);
        let mut out = LayerTransform::from_affine(&placed, t.rotation, flip);
        out.sampling = t.sampling;
        out.origin = [round_near(out.origin[0]), round_near(out.origin[1])];
        out.size = [round_near(out.size[0]), round_near(out.size[1])];
        out.rotation = round_near(out.rotation);
        out
    };
    for layer in &mut doc.layers {
        layer.transform = place(&layer.transform);
        if let Some(mask) = layer.mask.as_mut() {
            if let Some(p) = mask.placement.as_mut() {
                *p = place(p);
            }
        }
    }
    for guide in &mut doc.guides {
        let (x, y) = match guide.axis {
            super::GuideAxis::Horizontal => map.apply(0.0, guide.position),
            super::GuideAxis::Vertical => map.apply(guide.position, 0.0),
        };
        let swapped = map.a.abs() < 0.5;
        let axis = if swapped {
            match guide.axis {
                super::GuideAxis::Horizontal => super::GuideAxis::Vertical,
                super::GuideAxis::Vertical => super::GuideAxis::Horizontal,
            }
        } else {
            guide.axis
        };
        guide.axis = axis;
        guide.position = match axis {
            super::GuideAxis::Horizontal => y,
            super::GuideAxis::Vertical => x,
        };
    }
    doc.width = width;
    doc.height = height;
    doc.selection = None;
}

/// Snaps values within a hair of a whole number (floating-point noise from rotations).
fn round_near(v: f64) -> f64 {
    if (v - v.round()).abs() < 1e-6 { v.round() } else { v }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasTurn {
    Clockwise,
    CounterClockwise,
    Half,
    FlipHorizontal,
    FlipVertical,
}

pub fn turn_canvas(doc: &mut Document, turn: CanvasTurn) {
    let (w, h) = (doc.width as f64, doc.height as f64);
    let (map, nw, nh) = match turn {
        CanvasTurn::Clockwise => (super::Affine { a: 0.0, b: 1.0, c: -1.0, d: 0.0, tx: h, ty: 0.0 }, doc.height, doc.width),
        CanvasTurn::CounterClockwise => (super::Affine { a: 0.0, b: -1.0, c: 1.0, d: 0.0, tx: 0.0, ty: w }, doc.height, doc.width),
        CanvasTurn::Half => (super::Affine { a: -1.0, b: 0.0, c: 0.0, d: -1.0, tx: w, ty: h }, doc.width, doc.height),
        CanvasTurn::FlipHorizontal => (super::Affine { a: -1.0, b: 0.0, c: 0.0, d: 1.0, tx: w, ty: 0.0 }, doc.width, doc.height),
        CanvasTurn::FlipVertical => (super::Affine { a: 1.0, b: 0.0, c: 0.0, d: -1.0, tx: 0.0, ty: h }, doc.width, doc.height),
    };
    transform_canvas(doc, &map, nw, nh);
}

/// Crops the canvas to a rectangle; layer pixels outside it are kept, as in the macOS app.
pub fn crop_canvas(doc: &mut Document, x: i64, y: i64, width: u32, height: u32) {
    let map = super::Affine::translate(-x as f64, -y as f64);
    let selection = doc.selection.take();
    transform_canvas(doc, &map, width.max(1), height.max(1));
    doc.selection = selection.and_then(|s| {
        let cropped = image::imageops::crop_imm(&*s.mask, x.max(0) as u32, y.max(0) as u32, width, height).to_image();
        let mut full = GrayImage::new(width.max(1), height.max(1));
        image::imageops::replace(&mut full, &cropped, (-x).max(0), (-y).max(0));
        super::Selection::from_mask(full)
    });
}

/// Mirrors the selected layers in place, around their own centers.
pub fn flip_layers(doc: &mut Document, horizontal: bool) {
    for id in crate::tools::move_tool::move_targets(doc) {
        if let Some(layer) = doc.layer_mut(id) {
            if horizontal {
                layer.transform.flip_x = !layer.transform.flip_x;
            } else {
                layer.transform.flip_y = !layer.transform.flip_y;
            }
        }
    }
}
