//! Font coverage engine built on Pango (re-exported by GTK4).
//!
//! Two quality tiers:
//!
//! * Full mode: real Pango itemization of the cue text. Every character
//!   is attributed to the font Pango actually selects (including fallback
//!   fonts), and coverage is confirmed with `pango_font_has_glyph`.
//! * Degraded mode (resource-pressure path): a single probe font is
//!   loaded once and each character is checked against it directly.
//!   No shaping, no fallback resolution; results are conservative and
//!   every affected cue is marked `DegradedCheck` in the report.

use gtk4::pango;
use gtk4::pango::prelude::*;

use crate::model::CoverageStatus;

#[derive(Debug, Clone)]
pub struct GlyphOutcome {
    pub status: CoverageStatus,
    pub font: Option<String>,
}

pub struct CoverageEngine {
    fontmap: pango::FontMap,
    context: pango::Context,
}

impl Default for CoverageEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl CoverageEngine {
    pub fn new() -> Self {
        let fontmap = pangocairo::FontMap::new();
        let context = fontmap.create_context();
        Self { fontmap, context }
    }

    /// Full analysis: itemize `text`, attribute each non-whitespace char
    /// to the font Pango picked, and verify glyph presence.
    pub fn analyze_full(&self, text: &str) -> Vec<(char, GlyphOutcome)> {
        let items = pango::itemize(&self.context, text, 0, text.len() as i32, &pango::AttrList::new(), None);
        let primary_family = items
            .first()
            .map(|it| font_family(&it.analysis().font()))
            .unwrap_or_default();
        let mut out = Vec::new();
        for (byte_idx, ch) in text.char_indices() {
            if ch.is_whitespace() || ch.is_control() {
                continue;
            }
            let item = items.iter().find(|it| {
                let start = it.offset() as usize;
                let end = start + it.length() as usize;
                byte_idx >= start && byte_idx < end
            });
            let outcome = match item {
                Some(it) => {
                    let font = it.analysis().font();
                    let family = font_family(&font);
                    if !font.has_char(ch) {
                        GlyphOutcome { status: CoverageStatus::Missing, font: Some(family) }
                    } else if family != primary_family {
                        GlyphOutcome {
                            status: CoverageStatus::CoveredByFallback,
                            font: Some(family),
                        }
                    } else {
                        GlyphOutcome { status: CoverageStatus::Covered, font: Some(family) }
                    }
                }
                None => GlyphOutcome { status: CoverageStatus::Missing, font: None },
            };
            out.push((ch, outcome));
        }
        out
    }

    /// Degraded analysis: probe one cheap font, no shaping or fallback.
    pub fn analyze_degraded(&self, text: &str) -> Vec<(char, GlyphOutcome)> {
        let desc = pango::FontDescription::from_string("sans 12");
        let probe = self.fontmap.load_font(&self.context, &desc);
        let probe_family = probe.as_ref().map(font_family);
        let mut out = Vec::new();
        for ch in text.chars() {
            if ch.is_whitespace() || ch.is_control() {
                continue;
            }
            let covered = probe.as_ref().is_some_and(|f| f.has_char(ch));
            out.push((
                ch,
                GlyphOutcome {
                    status: if covered {
                        CoverageStatus::DegradedCheck
                    } else {
                        CoverageStatus::Missing
                    },
                    font: probe_family.clone(),
                },
            ));
        }
        out
    }
}

fn font_family(font: &pango::Font) -> String {
    font.describe()
        .family()
        .map(|f| f.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
