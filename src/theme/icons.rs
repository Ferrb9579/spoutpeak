//! Original MingCute and Solar geometry; see assets/icons/README.md for sources.

use super::{Glyph, IconGlyph, IconLayer, to_path};
use incular::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

#[derive(Deserialize)]
struct SourceGlyph {
    paths: Vec<SourcePath>,
}

#[derive(Deserialize)]
struct SourcePath {
    d: String,
    even_odd: bool,
    #[serde(default)]
    stroke_width: Option<f32>,
    #[serde(default)]
    round_cap: bool,
    #[serde(default)]
    round_join: bool,
}

fn glyph(name: &str) -> Glyph {
    static GLYPHS: OnceLock<HashMap<String, Glyph>> = OnceLock::new();
    GLYPHS.get_or_init(|| {
        let source: HashMap<String, SourceGlyph> =
            serde_json::from_str(include_str!("../../assets/icons/glyphs.json"))
                .expect("valid bundled icon data");
        source.into_iter().map(|(name, glyph)| {
            let layers = glyph.paths.into_iter().map(|part| {
                let bez = kurbo::BezPath::from_svg(&part.d).expect("valid library icon path");
                let stroke = part.stroke_width.map(|width| {
                    let mut stroke = Stroke { width, ..Stroke::default() };
                    if part.round_cap {
                        stroke.cap = LineCap::Round;
                    }
                    if part.round_join {
                        stroke.join = LineJoin::Round;
                    }
                    stroke
                });
                IconLayer {
                    path: Arc::new(to_path(&bez)),
                    fill_rule: if part.even_odd { FillRule::EvenOdd } else { FillRule::NonZero },
                    stroke,
                }
            }).collect();
            (name, Arc::new(IconGlyph { layers }))
        }).collect()
    }).get(name).expect("known bundled icon").clone()
}

macro_rules! glyphs {
    ($($name:ident),+ $(,)?) => {
        $(pub fn $name() -> Glyph { glyph(stringify!($name)) })+
    };
}

glyphs! {
    house, compass, bookmark, users, search, puzzle, settings,
    arrow_left, chevron_left, chevron_right, chevron_up, chevron_down,
    play, pause, rewind, fast_forward, skip_forward, volume_2, volume_x,
    maximize, minimize, audio_lines, captions, gauge, sun, type_,
    download, trash_2, folder_open, rotate_ccw, check, x, plus, minus,
    log_in, message_square, window_maximize, window_restore,
}
