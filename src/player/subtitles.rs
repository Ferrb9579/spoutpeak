//! SRT / WebVTT parsing.

#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
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
