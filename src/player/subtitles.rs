//! SRT / WebVTT parsing.

#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// A playing line, or the insertion point for a marker between sorted cues.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CuePosition {
    pub index: usize,
    pub in_gap: bool,
}

pub fn position_at(cues: &[Cue], seconds: f64) -> CuePosition {
    if let Some(index) = cues.iter().position(|cue| cue.start <= seconds && seconds < cue.end) {
        CuePosition { index, in_gap: false }
    } else {
        CuePosition { index: cues.partition_point(|cue| cue.start <= seconds), in_gap: true }
    }
}

/// Parses a whole SRT or VTT document.
pub fn parse(document: &str) -> Vec<Cue> {
    let normalized = document.replace("\r\n", "\n").replace('\r', "\n");
    let mut cues: Vec<Cue> = normalized
        .split("\n\n")
        .filter_map(|block| parse_block(block, 0.0))
        .collect();
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    cues
}

/// Parses one cue block; `offset` seconds are added to both timestamps.
pub fn parse_block(block: &str, offset: f64) -> Option<Cue> {
    let mut lines = block.lines().map(str::trim_end);
    let timing = lines.by_ref().find(|line| line.contains("-->"))?;
    let (start, rest) = timing.split_once("-->")?;
    let end = rest.split_whitespace().next()?;
    let start = parse_timestamp(start.trim())?;
    let end = parse_timestamp(end)?;
    let text = lines
        .map(strip_markup)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return None;
    }
    Some(Cue {
        start: start + offset,
        end: end + offset,
        text,
    })
}

fn parse_timestamp(value: &str) -> Option<f64> {
    let value = value.replace(',', ".");
    let parts: Vec<&str> = value.split(':').collect();
    let (h, m, s) = match parts.as_slice() {
        [h, m, s] => (h.parse::<f64>().ok()?, m.parse::<f64>().ok()?, *s),
        [m, s] => (0.0, m.parse::<f64>().ok()?, *s),
        _ => return None,
    };
    Some(h * 3600.0 + m * 60.0 + s.parse::<f64>().ok()?)
}

/// Removes `<i>`-style tags and `{\an8}`-style ASS overrides.
fn strip_markup(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut depth_angle = 0;
    let mut depth_brace = 0;
    for ch in line.chars() {
        match ch {
            '<' => depth_angle += 1,
            '>' if depth_angle > 0 => depth_angle -= 1,
            '{' => depth_brace += 1,
            '}' if depth_brace > 0 => depth_brace -= 1,
            _ if depth_angle == 0 && depth_brace == 0 => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .trim()
        .to_owned()
}

/// Text of every cue active at `time`, newest last.
pub fn active_text(cues: &[Cue], time: f64) -> String {
    cues.iter()
        .filter(|cue| cue.start <= time && time < cue.end)
        .map(|cue| cue.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod position_tests {
    use super::*;

    fn cue(start: f64, end: f64) -> Cue {
        Cue { start, end, text: String::new() }
    }

    #[test]
    fn gaps_have_a_marker_before_between_and_after_lines() {
        let cues = [cue(10.0, 12.0), cue(20.0, 22.0)];
        for (seconds, index) in [(0.0, 0), (12.0, 1), (19.9, 1), (22.0, 2), (30.0, 2)] {
            assert_eq!(position_at(&cues, seconds), CuePosition { index, in_gap: true });
        }
        for (seconds, index) in [(10.0, 0), (11.9, 0), (20.0, 1), (21.9, 1)] {
            assert_eq!(position_at(&cues, seconds), CuePosition { index, in_gap: false });
        }
        assert_eq!(position_at(&[], 5.0), CuePosition { index: 0, in_gap: true });
    }

    #[test]
    fn overlapping_lines_stay_active_until_every_line_has_ended() {
        let cues = [cue(10.0, 30.0), cue(15.0, 17.0), cue(35.0, 40.0)];
        assert_eq!(position_at(&cues, 20.0), CuePosition { index: 0, in_gap: false });
        assert_eq!(position_at(&cues, 30.0), CuePosition { index: 2, in_gap: true });
        // A positive subtitle delay moves the marker back on the cue timeline.
        assert_eq!(position_at(&cues, 36.0 - 5.0), CuePosition { index: 2, in_gap: true });
        assert_eq!(position_at(&cues, 36.0), CuePosition { index: 2, in_gap: false });
    }
}
