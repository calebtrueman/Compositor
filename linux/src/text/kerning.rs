//! Pair kerning. Current fonts keep it in the GPOS `kern` feature, which ab_glyph doesn't read;
//! older ones in the `kern` table, which it does.

use ab_glyph::{Font, FontArc, GlyphId};
use ttf_parser::gpos::{PairAdjustment, PositioningSubtable};
use ttf_parser::Tag;

pub struct Kerning<'a> {
    font: &'a FontArc,
    pairs: Vec<PairAdjustment<'a>>,
}

impl<'a> Kerning<'a> {
    pub fn new(font: &'a FontArc, index: u32) -> Self {
        Kerning { font, pairs: gpos_pairs(font.font_data(), index).unwrap_or_default() }
    }

    /// The adjustment between two glyphs, in font units.
    pub fn pair(&self, first: GlyphId, second: GlyphId) -> f32 {
        if self.pairs.is_empty() {
            return self.font.kern_unscaled(first, second);
        }
        let (a, b) = (ttf_parser::GlyphId(first.0), ttf_parser::GlyphId(second.0));
        for subtable in &self.pairs {
            match subtable {
                PairAdjustment::Format1 { coverage, sets } => {
                    let Some(set) = coverage.get(a).and_then(|i| sets.get(i)) else { continue };
                    if let Some((value, _)) = set.get(b) {
                        return value.x_advance as f32;
                    }
                }
                PairAdjustment::Format2 { coverage, classes, matrix } => {
                    if !coverage.contains(a) {
                        continue;
                    }
                    if let Some((value, _)) = matrix.get((classes.0.get(a), classes.1.get(b))) {
                        return value.x_advance as f32;
                    }
                }
            }
        }
        0.0
    }
}

fn gpos_pairs(data: &[u8], index: u32) -> Option<Vec<PairAdjustment<'_>>> {
    let face = ttf_parser::Face::parse(data, index).ok()?;
    let gpos = face.tables().gpos?;
    let kern = Tag::from_bytes(b"kern");
    let mut lookups: Vec<u16> = Vec::new();
    for i in 0..gpos.features.len() {
        let Some(feature) = gpos.features.get(i) else { continue };
        if feature.tag == kern {
            for lookup in feature.lookup_indices {
                if !lookups.contains(&lookup) {
                    lookups.push(lookup);
                }
            }
        }
    }
    lookups.sort_unstable();
    let mut pairs = Vec::new();
    for index in lookups {
        let Some(lookup) = gpos.lookups.get(index) else { continue };
        for subtable in lookup.subtables.into_iter::<PositioningSubtable>() {
            if let PositioningSubtable::Pair(pair) = subtable {
                pairs.push(pair);
            }
        }
    }
    (!pairs.is_empty()).then_some(pairs)
}
