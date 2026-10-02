//! Lays out and draws text as the macOS app's TextKit setup does: lines a fixed `line_height`
//! apart with the baseline the font's descent up from each line's bottom, word wrapping inside a
//! paragraph box, alignment, tracking after every letter and pair kerning. Letters keep their
//! own face and color from the style's runs. Coordinates are layer pixels, with the text inset by
//! `PADDING` from the layer's edges.

use std::collections::HashMap;
use std::ops::Range;

use ab_glyph::{point, Font, FontArc, GlyphId};
use image::{Rgba, RgbaImage};

use super::fonts::{self, same_font};
use super::kerning::Kerning;
use super::{Alignment, TextStyle, PADDING};

/// Largest text image, as for any layer.
const MAX_SIDE: f64 = 30_000.0;
const MAX_PIXELS: f64 = 100_000_000.0;

#[derive(Clone)]
struct PlacedGlyph {
    font: FontArc,
    id: GlyphId,
    /// From the line's left edge.
    x: f32,
    color: [f64; 3],
}

#[derive(Clone)]
pub struct Line {
    /// UTF-16 range of the line's letters, without its line break.
    pub start: usize,
    pub end: usize,
    /// Left edge after alignment.
    pub x: f32,
    pub top: f32,
    pub baseline: f32,
    /// Caret positions: every letter boundary from `start` to `end`, with its x.
    pub stops: Vec<(usize, f32)>,
    /// Lines that don't fit a paragraph box aren't drawn, as in TextKit.
    pub visible: bool,
    glyphs: Vec<PlacedGlyph>,
}

#[derive(Clone)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub lines: Vec<Line>,
    pub line_height: f32,
    pub font_size: f32,
}

struct Item {
    unit: usize,
    font: FontArc,
    id: GlyphId,
    advance: f32,
    space: bool,
    color: [f64; 3],
}

/// Splits a paragraph into lines no wider than `max` (when given): after the last space that
/// fits, or between letters when one word alone is too wide. Spaces at the end of a line hang
/// past the edge and never force a break. Returns item ranges; an empty paragraph is one line.
pub fn break_lines(advances: &[f32], spaces: &[bool], max: Option<f32>) -> Vec<Range<usize>> {
    let n = advances.len();
    let Some(max) = max else { return vec![0..n] };
    let mut lines = Vec::new();
    let mut start = 0;
    let mut x = 0.0f32;
    let mut last_break: Option<usize> = None;
    let mut i = 0;
    while i < n {
        if spaces[i] {
            x += advances[i];
            i += 1;
            last_break = Some(i);
            continue;
        }
        if x + advances[i] > max + 0.01 && i > start {
            let end = match last_break {
                Some(b) if b > start => b,
                _ => i,
            };
            lines.push(start..end);
            start = end;
            x = advances[start..i].iter().sum();
            last_break = None;
            continue;
        }
        x += advances[i];
        i += 1;
    }
    lines.push(start..n);
    lines
}

/// Lays out `style`, or `None` when no font is installed or the text would be too large.
pub fn layout(style: &TextStyle) -> Option<Layout> {
    let library = fonts::library();
    let size = style.font_size as f32;
    let primary = library.resolve(&style.font_name)?;
    let descent = -primary.descent_unscaled() * size / primary.units_per_em().unwrap_or(1000.0);
    let line_height = style.line_height() as f32;
    let tracking = style.tracking as f32;

    // Letters with their faces, advances and colors; `None` marks a line break.
    let mut faces: HashMap<String, Option<FontArc>> = HashMap::new();
    let mut items: Vec<Option<Item>> = Vec::new();
    let mut breaks: Vec<usize> = Vec::new();
    let mut unit = 0;
    for c in style.content.chars() {
        if c == '\n' {
            items.push(None);
            breaks.push(unit);
            unit += 1;
            continue;
        }
        let name = style.font_name_at(unit);
        let face = faces
            .entry(name.to_string())
            .or_insert_with(|| library.resolve(name))
            .clone()
            .unwrap_or_else(|| primary.clone());
        let mut font = face;
        let mut id = font.glyph_id(c);
        if id.0 == 0 && !c.is_whitespace() {
            if let Some(fallback) = library.fallback_for(c) {
                id = fallback.glyph_id(c);
                font = fallback;
            }
        }
        let scale = size / font.units_per_em().unwrap_or(1000.0);
        let mut advance = font.h_advance_unscaled(id) * scale;
        if c == '\t' {
            advance = font.h_advance_unscaled(font.glyph_id(' ')) * scale * 4.0;
        }
        items.push(Some(Item { unit, font, id, advance: advance + tracking, space: c.is_whitespace(), color: style.color_at(unit) }));
        unit += c.len_utf16();
    }
    let total = unit;
    // Pair kerning between neighbors set in the same face.
    let mut distinct: Vec<FontArc> = Vec::new();
    for item in items.iter().flatten() {
        if !distinct.iter().any(|f| same_font(f, &item.font)) {
            distinct.push(item.font.clone());
        }
    }
    let kernings: Vec<Kerning> = distinct.iter().map(|f| Kerning::new(f, library.face_index(f))).collect();
    for i in 1..items.len() {
        let (before, after) = items.split_at_mut(i);
        if let (Some(a), Some(b)) = (before[i - 1].as_mut(), after[0].as_ref()) {
            if same_font(&a.font, &b.font) {
                let Some(k) = distinct.iter().position(|f| same_font(f, &a.font)) else { continue };
                let scale = size / a.font.units_per_em().unwrap_or(1000.0);
                a.advance += kernings[k].pair(a.id, b.id) * scale;
            }
        }
    }

    let max_width = style.box_size.map(|b| (b[0] - 2.0 * PADDING).max(1.0) as f32);
    // Paragraphs, then lines within them.
    let mut lines: Lines = Vec::new();
    let mut paragraph: Vec<&Item> = Vec::new();
    let mut paragraph_start = 0;
    type Lines<'a> = Vec<(Vec<&'a Item>, usize, usize)>;
    fn flush<'a>(paragraph: &mut Vec<&'a Item>, start: usize, end: usize, max_width: Option<f32>, lines: &mut Lines<'a>) {
        let advances: Vec<f32> = paragraph.iter().map(|i| i.advance).collect();
        let spaces: Vec<bool> = paragraph.iter().map(|i| i.space).collect();
        for range in break_lines(&advances, &spaces, max_width) {
            let line_start = paragraph.get(range.start).map_or(start, |i| i.unit);
            let line_end = paragraph.get(range.end).map_or(end, |i| i.unit);
            lines.push((paragraph[range].to_vec(), line_start, line_end));
        }
        paragraph.clear();
    }
    let mut break_index = 0;
    for item in &items {
        match item {
            Some(item) => paragraph.push(item),
            None => {
                let end = breaks[break_index];
                flush(&mut paragraph, paragraph_start, end, max_width, &mut lines);
                paragraph_start = end + 1;
                break_index += 1;
            }
        }
    }
    flush(&mut paragraph, paragraph_start, total, max_width, &mut lines);

    let widths: Vec<f32> = lines
        .iter()
        .map(|(items, _, _)| {
            let letters = items.iter().rposition(|i| !i.space).map_or(0, |p| p + 1);
            items[..letters].iter().map(|i| i.advance).sum()
        })
        .collect();
    let (width, height) = match style.box_size {
        Some(b) => (b[0].ceil(), b[1].ceil()),
        None => {
            let widest = widths.iter().cloned().fold(0.0f32, f32::max) as f64;
            let block = (lines.len() as f64 * line_height as f64).max(line_height as f64);
            (
                (widest + PADDING * 2.0 + style.font_size * 0.1).ceil().max(16.0),
                (block + PADDING * 2.0).ceil().max(16.0),
            )
        }
    };
    if !(1.0..=MAX_SIDE).contains(&width) || !(1.0..=MAX_SIDE).contains(&height) || width * height > MAX_PIXELS {
        return None;
    }
    let available = (width - 2.0 * PADDING).max(1.0) as f32;
    let fits = (height - 2.0 * PADDING) as f32 + 0.01;
    let mut out = Vec::with_capacity(lines.len());
    for (index, ((items, start, end), width)) in lines.into_iter().zip(widths).enumerate() {
        let x = PADDING as f32
            + match style.alignment {
                Alignment::Left => 0.0,
                Alignment::Center => (available - width) / 2.0,
                Alignment::Right => available - width,
            };
        let top = PADDING as f32 + index as f32 * line_height;
        let mut stops = Vec::with_capacity(items.len() + 1);
        let mut glyphs = Vec::with_capacity(items.len());
        let mut pen = 0.0;
        for item in &items {
            stops.push((item.unit, x + pen));
            glyphs.push(PlacedGlyph { font: item.font.clone(), id: item.id, x: pen, color: item.color });
            pen += item.advance;
        }
        stops.push((end, x + pen));
        out.push(Line {
            start,
            end,
            x,
            top,
            baseline: top + line_height - descent,
            stops,
            visible: style.box_size.is_none() || (index + 1) as f32 * line_height <= fits,
            glyphs,
        });
    }
    Some(Layout { width: width as u32, height: height as u32, lines: out, line_height, font_size: size })
}

impl Layout {
    /// The text drawn on a transparent layer-sized image.
    pub fn draw(&self) -> RgbaImage {
        let mut image = RgbaImage::new(self.width, self.height);
        let (w, h) = (self.width as i32, self.height as i32);
        for line in self.lines.iter().filter(|l| l.visible) {
            for glyph in &line.glyphs {
                // ab_glyph scales by the font's height, not its em.
                let em = glyph.font.units_per_em().unwrap_or(1000.0);
                let scale = ab_glyph::PxScale::from(self.font_size * glyph.font.height_unscaled() / em);
                let positioned = glyph.id.with_scale_and_position(scale, point(line.x + glyph.x, line.baseline));
                let Some(outline) = glyph.font.outline_glyph(positioned) else { continue };
                let bounds = outline.px_bounds();
                let color = glyph.color.map(|c| (c.clamp(0.0, 1.0) * 255.0) as f32);
                outline.draw(|px, py, coverage| {
                    let x = bounds.min.x as i32 + px as i32;
                    let y = bounds.min.y as i32 + py as i32;
                    if x < 0 || y < 0 || x >= w || y >= h || coverage <= 0.0 {
                        return;
                    }
                    let pixel = image.get_pixel_mut(x as u32, y as u32);
                    *pixel = over(*pixel, color, coverage.min(1.0));
                });
            }
        }
        image
    }

    /// The line showing the caret at `unit`: where a wrapped line ends, the next one.
    pub fn line_of(&self, unit: usize) -> usize {
        self.lines.iter().rposition(|l| l.start <= unit && unit <= l.end).unwrap_or(0)
    }

    /// The caret's x on line `line`.
    pub fn caret_x(&self, line: usize, unit: usize) -> f32 {
        let Some(line) = self.lines.get(line) else { return PADDING as f32 };
        line.stops.iter().rev().find(|(u, _)| *u <= unit).or(line.stops.first()).map_or(line.x, |s| s.1)
    }

    /// The letter boundary nearest a layer-pixel point.
    pub fn hit(&self, x: f32, y: f32) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let index = (((y - PADDING as f32) / self.line_height).floor().max(0.0) as usize).min(self.lines.len() - 1);
        self.hit_in_line(index, x)
    }

    pub fn hit_in_line(&self, index: usize, x: f32) -> usize {
        let line = &self.lines[index];
        let mut best = line.stops.first().map_or(line.start, |s| s.0);
        for pair in line.stops.windows(2) {
            if x >= (pair[0].1 + pair[1].1) / 2.0 {
                best = pair[1].0;
            }
        }
        best
    }

    /// Highlight rectangles `(x0, y0, x1, y1)` in layer pixels for the letters in `range`.
    pub fn selection_rects(&self, range: Range<usize>) -> Vec<(f32, f32, f32, f32)> {
        let mut rects = Vec::new();
        if range.is_empty() {
            return rects;
        }
        for (index, line) in self.lines.iter().enumerate() {
            let start = range.start.max(line.start);
            let end = range.end.min(line.end);
            let through_break = range.end > line.end && range.start <= line.end;
            if start > end || (start == end && !through_break) {
                continue;
            }
            let x0 = self.caret_x(index, start);
            let mut x1 = self.caret_x(index, end);
            if through_break {
                x1 += self.font_size * 0.25;
            }
            rects.push((x0, line.top, x1, line.top + self.line_height));
        }
        rects
    }
}

/// Paints straight-alpha `color` at `coverage` over a straight-alpha pixel.
fn over(below: Rgba<u8>, color: [f32; 3], coverage: f32) -> Rgba<u8> {
    let da = below[3] as f32 / 255.0;
    let a = coverage + da * (1.0 - coverage);
    if a <= 0.0 {
        return below;
    }
    let mut out = [0u8; 4];
    for i in 0..3 {
        let v = (color[i] * coverage + below[i] as f32 * da * (1.0 - coverage)) / a;
        out[i] = v.round().clamp(0.0, 255.0) as u8;
    }
    out[3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgba(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaks_after_spaces_and_inside_long_words() {
        // "aa bb cc" with every letter 10 wide: "aa bb" just fits 50.
        let text = "aa bb cc";
        let advances = vec![10.0; text.len()];
        let spaces: Vec<bool> = text.chars().map(|c| c == ' ').collect();
        let lines = break_lines(&advances, &spaces, Some(50.0));
        assert_eq!(lines, vec![0..6, 6..8]);
        assert_eq!(break_lines(&advances, &spaces, Some(40.0)), vec![0..3, 3..6, 6..8]);
        // A word wider than the box breaks between letters.
        let lines = break_lines(&[10.0; 7], &[false; 7], Some(30.0));
        assert_eq!(lines, vec![0..3, 3..6, 6..7]);
        // No box: one line.
        assert_eq!(break_lines(&advances, &spaces, None), vec![0..8]);
        assert_eq!(break_lines(&[], &[], Some(10.0)), vec![0..0]);
    }

    #[test]
    fn wraps_inside_a_box() {
        if fonts::library().is_empty() {
            eprintln!("No fonts installed; skipping");
            return;
        }
        let mut style = TextStyle { content: "The quick brown fox jumps over the lazy dog".into(), font_size: 20.0, ..TextStyle::default() };
        style.font_name = fonts::library().default_post_script_name();
        let point = layout(&style).unwrap();
        assert_eq!(point.lines.len(), 1);
        style.box_size = Some([120.0, 400.0]);
        let boxed = layout(&style).unwrap();
        assert!(boxed.lines.len() > 2, "expected wrapping, got {} lines", boxed.lines.len());
        assert_eq!((boxed.width, boxed.height), (120, 400));
        for line in &boxed.lines {
            let letters = line.stops.iter().rev().find(|(u, _)| *u == line.end || !style.content[*u..].starts_with(' '));
            assert!(letters.unwrap().1 <= 120.0 - PADDING as f32 + 0.5);
        }
        // Every letter is on some line, in order.
        assert_eq!(boxed.lines.first().unwrap().start, 0);
        assert_eq!(boxed.lines.last().unwrap().end, style.content.len());
        let image = boxed.draw();
        assert!(image.pixels().any(|p| p[3] > 0));
        // Right alignment moves lines to the right edge.
        style.alignment = Alignment::Right;
        let right = layout(&style).unwrap();
        assert!(right.lines.iter().all(|l| l.x > PADDING as f32));
        assert!(right.lines.iter().zip(&boxed.lines).all(|(r, l)| r.x >= l.x));
    }
}
