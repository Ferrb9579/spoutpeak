//! Palette, typography and the small set of styled building blocks every
//! screen shares.

use incular::controls::{Button, ButtonStyle, StateColor, TextField, TextFieldStyle};
use incular::prelude::*;
use std::sync::Arc;

pub const PANEL: Color = Color::rgba(17, 20, 31, 255);

pub const BG: Color = Color::rgba(11, 13, 20, 255);
pub const SIDEBAR: Color = Color::rgba(15, 18, 28, 255);
pub const SURFACE: Color = Color::rgba(23, 27, 40, 255);
pub const SURFACE_HI: Color = Color::rgba(33, 38, 56, 255);
pub const SURFACE_HOVER: Color = Color::rgba(42, 48, 70, 255);
pub const LINE: Color = Color::rgba(40, 46, 66, 255);
pub const TEXT: Color = Color::rgba(236, 239, 247, 255);
pub const MUTED: Color = Color::rgba(146, 154, 178, 255);
pub const FAINT: Color = Color::rgba(96, 104, 128, 255);
pub const ACCENT: Color = Color::rgba(255, 138, 76, 255);
pub const ACCENT_HI: Color = Color::rgba(255, 160, 108, 255);
pub const ACCENT_DIM: Color = Color::rgba(62, 40, 30, 255);
pub const ON_ACCENT: Color = Color::rgba(26, 14, 6, 255);
pub const DANGER: Color = Color::rgba(240, 96, 104, 255);
pub const GOOD: Color = Color::rgba(92, 204, 150, 255);
pub const SCRIM: Color = Color::rgba(5, 6, 10, 190);

pub fn style(size: f32, weight: FontWeight, color: Color) -> TextStyle {
    TextStyle::new()
        .font_size(size)
        .font_weight(weight)
        .color(color)
}

pub fn text(value: impl Into<String>, size: f32, weight: FontWeight, color: Color) -> Text {
    Text::new(value).style(style(size, weight, color))
}

/// Single-line text that ellipsizes instead of wrapping.
pub fn line(value: impl Into<String>, size: f32, weight: FontWeight, color: Color) -> Widget {
    text(value, size, weight, color)
        .max_lines(Some(1))
        .soft_wrap(false)
        .overflow(TextOverflow::Ellipsis)
        .into()
}

pub fn body(value: impl Into<String>) -> Widget {
    text(value, 14.0, FontWeight::W400, TEXT).into()
}

pub fn muted(value: impl Into<String>) -> Widget {
    text(value, 13.0, FontWeight::W400, MUTED).into()
}

pub fn heading(value: impl Into<String>) -> Widget {
    text(value, 26.0, FontWeight::W700, TEXT).into()
}

pub fn section(value: impl Into<String>) -> Widget {
    text(value, 17.0, FontWeight::W600, TEXT).into()
}

pub fn gap(height: f32) -> Widget {
    SizedBox::new().height(height).into()
}

pub fn hgap(width: f32) -> Widget {
    SizedBox::new().width(width).into()
}

pub fn col(children: Vec<Widget>) -> Column {
    Column::new(children)
        .main_axis_size(MainAxisSize::Min)
        .cross_axis_alignment(CrossAxisAlignment::Start)
}

pub fn row(children: Vec<Widget>) -> Row {
    Row::new(children)
        .main_axis_size(MainAxisSize::Min)
        .cross_axis_alignment(CrossAxisAlignment::Center)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Primary,
    Neutral,
    Ghost,
    Danger,
}

fn button_style(tone: Tone, height: f32) -> ButtonStyle {
    let (background, hover, foreground) = match tone {
        Tone::Primary => (ACCENT, ACCENT_HI, ON_ACCENT),
        Tone::Neutral => (SURFACE_HI, SURFACE_HOVER, TEXT),
        Tone::Ghost => (Color::TRANSPARENT, SURFACE_HI, MUTED),
        Tone::Danger => (Color::rgba(70, 30, 36, 255), Color::rgba(98, 38, 46, 255), DANGER),
    };
    ButtonStyle {
        background: Some(background),
        background_states: Some(StateColor::new(background).hovered(hover)),
        foreground: Some(foreground),
        border: Some(Border::new(0.0, Color::TRANSPARENT)),
        border_radius: Some(height / 2.0),
        padding: Some(EdgeInsets::symmetric(16.0, 0.0)),
        height: Some(height),
        text_style: Some(style(13.5, FontWeight::W600, foreground)),
        overlay_color: Some(Color::TRANSPARENT.into()),
        animation_duration: Some(std::time::Duration::ZERO),
        ..ButtonStyle::default()
    }
}

pub fn button(label: impl Into<String>, tone: Tone, on_click: impl Fn() + 'static) -> Widget {
    Button::new(label)
        .style(button_style(tone, 36.0))
        .on_click(on_click)
        .into()
}

pub fn small_button(label: impl Into<String>, tone: Tone, on_click: impl Fn() + 'static) -> Widget {
    let mut style = button_style(tone, 28.0);
    style.padding = Some(EdgeInsets::symmetric(12.0, 0.0));
    style.text_style = Some(self::style(12.5, FontWeight::W600, style.foreground.unwrap_or(TEXT)));
    Button::new(label).style(style).on_click(on_click).into()
}

/// A selectable pill, used for filters and segmented choices.
pub fn chip(label: impl Into<String>, selected: bool, on_click: impl Fn() + 'static) -> Widget {
    let (background, hover, foreground) = if selected {
        (ACCENT_DIM, ACCENT_DIM, ACCENT)
    } else {
        (SURFACE, SURFACE_HOVER, MUTED)
    };
    Button::new(label)
        .style(ButtonStyle {
            background: Some(background),
            background_states: Some(StateColor::new(background).hovered(hover)),
            foreground: Some(foreground),
            border: Some(Border::new(0.0, Color::TRANSPARENT)),
            border_radius: Some(15.0),
            padding: Some(EdgeInsets::symmetric(13.0, 0.0)),
            height: Some(30.0),
            text_style: Some(style(12.5, FontWeight::W500, foreground)),
            overlay_color: Some(Color::TRANSPARENT.into()),
            animation_duration: Some(std::time::Duration::ZERO),
            ..ButtonStyle::default()
        })
        .on_click(on_click)
        .into()
}

/// A square button holding an arbitrary child (icons).
pub fn icon_button(child: impl Into<Widget>, size: f32, on_click: impl Fn() + 'static) -> Widget {
    Button::with_child(child)
        .style(ButtonStyle {
            background: Some(Color::TRANSPARENT),
            background_states: Some(
                StateColor::new(Color::TRANSPARENT).hovered(Color::rgba(255, 255, 255, 28)),
            ),
            foreground: Some(TEXT),
            border: Some(Border::new(0.0, Color::TRANSPARENT)),
            border_radius: Some(size / 2.0),
            padding: Some(EdgeInsets::ZERO),
            fixed_size: Some(Size::new(size, size)),
            alignment: Some(Alignment::CENTER),
            overlay_color: Some(Color::TRANSPARENT.into()),
            animation_duration: Some(std::time::Duration::ZERO),
            ..ButtonStyle::default()
        })
        .on_click(on_click)
        .into()
}

pub fn field_style() -> TextFieldStyle {
    TextFieldStyle {
        background: Some(SURFACE_HI),
        foreground: Some(TEXT),
        placeholder_color: Some(FAINT),
        border: Some(Border::new(0.0, Color::TRANSPARENT)),
        // Left unset: Incular 0.1.1 draws the focused border around the
        // inner editable area instead of the field's outer box.
        border_focused: None,
        border_radius: Some(10.0),
        padding: Some(EdgeInsets::symmetric(12.0, 0.0)),
    }
}

pub fn input(
    controller: &TextEditingController,
    placeholder: &str,
    width: f32,
    on_submit: impl Fn(String) + 'static,
) -> TextField {
    TextField::new(controller.clone())
        .placeholder(placeholder)
        .size(Size::new(width, 40.0))
        .style(field_style())
        .on_submit(on_submit)
}

/// A rounded panel.
pub fn card(child: impl Into<Widget>) -> Container {
    Container::new()
        .padding(EdgeInsets::all(18.0))
        .decoration(
            BoxDecoration::new()
                .color(SURFACE)
                .border_radius(BorderRadius::all(Radius::circular(14.0))),
        )
        .child(child)
}

pub fn divider() -> Widget {
    Container::new().height(1.0).color(LINE).into()
}

/// Lucide icons (<https://lucide.dev>, ISC licence), path data copied
/// verbatim from `lucide-static` 1.72.0. Each function returns the icon's
/// geometry on Lucide's 24x24 grid; [`icon`] scales and strokes it.
/// `<circle>` and `<rect>` elements are rewritten as equivalent path data.
pub mod icons {
    use super::*;
    use kurbo::BezPath;
    use std::sync::OnceLock;

    fn from_svg(paths: &[&str]) -> Arc<Path> {
        let mut all = BezPath::new();
        for data in paths {
            all.extend(BezPath::from_svg(data).expect("valid Lucide path data"));
        }
        Arc::new(to_path(&all))
    }

    macro_rules! lucide {
        ($($name:ident => [$($d:literal),+ $(,)?];)+) => {
            $(
                pub fn $name() -> Arc<Path> {
                    static PATH: OnceLock<Arc<Path>> = OnceLock::new();
                    PATH.get_or_init(|| from_svg(&[$($d),+])).clone()
                }
            )+
        };
    }

    lucide! {
        // Navigation
        house => [
            "M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8",
            "M3 10a2 2 0 0 1 .709-1.528l7-6a2 2 0 0 1 2.582 0l7 6A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
        ];
        compass => [
            "M2 12a10 10 0 1 0 20 0a10 10 0 1 0-20 0z",
            "m16.24 7.76-1.804 5.411a2 2 0 0 1-1.265 1.265L7.76 16.24l1.804-5.411a2 2 0 0 1 1.265-1.265z",
        ];
        bookmark => [
            "M17 3a2 2 0 0 1 2 2v15a1 1 0 0 1-1.496.868l-4.512-2.578a2 2 0 0 0-1.984 0l-4.512 2.578A1 1 0 0 1 5 20V5a2 2 0 0 1 2-2z",
        ];
        users => [
            "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2",
            "M5 7a4 4 0 1 0 8 0a4 4 0 1 0-8 0z",
            "M22 21v-2a4 4 0 0 0-3-3.87",
            "M16 3.13a4 4 0 0 1 0 7.75",
        ];
        search => [
            "m21 21-4.34-4.34",
            "M3 11a8 8 0 1 0 16 0a8 8 0 1 0-16 0z",
        ];
        puzzle => [
            "M15.39 4.39a1 1 0 0 0 1.68-.474 2.5 2.5 0 1 1 3.014 3.015 1 1 0 0 0-.474 1.68l1.683 1.682a2.414 2.414 0 0 1 0 3.414L19.61 15.39a1 1 0 0 1-1.68-.474 2.5 2.5 0 1 0-3.014 3.015 1 1 0 0 1 .474 1.68l-1.683 1.682a2.414 2.414 0 0 1-3.414 0L8.61 19.61a1 1 0 0 0-1.68.474 2.5 2.5 0 1 1-3.014-3.015 1 1 0 0 0 .474-1.68l-1.683-1.682a2.414 2.414 0 0 1 0-3.414L4.39 8.61a1 1 0 0 1 1.68.474 2.5 2.5 0 1 0 3.014-3.015 1 1 0 0 1-.474-1.68l1.683-1.682a2.414 2.414 0 0 1 3.414 0z",
        ];
        settings => [
            "M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915",
            "M9 12a3 3 0 1 0 6 0a3 3 0 1 0-6 0z",
        ];
        arrow_left => ["m12 19-7-7 7-7", "M19 12H5"];
        chevron_left => ["m15 18-6-6 6-6"];
        chevron_right => ["m9 18 6-6-6-6"];
        chevron_up => ["m18 15-6-6-6 6"];
        chevron_down => ["m6 9 6 6 6-6"];

        // Playback
        play => [
            "M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z",
        ];
        pause => [
            "M15 3h3a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z",
            "M6 3h3a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z",
        ];
        rewind => [
            "M12 6a2 2 0 0 0-3.414-1.414l-6 6a2 2 0 0 0 0 2.828l6 6A2 2 0 0 0 12 18z",
            "M22 6a2 2 0 0 0-3.414-1.414l-6 6a2 2 0 0 0 0 2.828l6 6A2 2 0 0 0 22 18z",
        ];
        fast_forward => [
            "M12 6a2 2 0 0 1 3.414-1.414l6 6a2 2 0 0 1 0 2.828l-6 6A2 2 0 0 1 12 18z",
            "M2 6a2 2 0 0 1 3.414-1.414l6 6a2 2 0 0 1 0 2.828l-6 6A2 2 0 0 1 2 18z",
        ];
        skip_forward => [
            "M21 4v16",
            "M6.029 4.285A2 2 0 0 0 3 6v12a2 2 0 0 0 3.029 1.715l9.997-5.998a2 2 0 0 0 .003-3.432z",
        ];
        volume_2 => [
            "M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z",
            "M16 9a5 5 0 0 1 0 6",
            "M19.364 18.364a9 9 0 0 0 0-12.728",
        ];
        volume_x => [
            "M11 4.702a.7.7 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.7.7 0 0 0 11 19.298z",
            "m16.5 14.5 5-5",
            "m16.5 9.5 5 5",
        ];
        maximize => [
            "M8 3H5a2 2 0 0 0-2 2v3",
            "M21 8V5a2 2 0 0 0-2-2h-3",
            "M3 16v3a2 2 0 0 0 2 2h3",
            "M16 21h3a2 2 0 0 0 2-2v-3",
        ];

        audio_lines => [
            "M2 10v3", "M6 6v11", "M10 3v18", "M14 8v7", "M18 5v13", "M22 10v3",
        ];
        captions => [
            "M5 5h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z",
            "M7 15h4M15 15h2M7 11h2M13 11h4",
        ];
        gauge => ["m12 14 4-4", "M3.34 19a10 10 0 1 1 17.32 0"];
        sun => [
            "M8 12a4 4 0 1 0 8 0a4 4 0 1 0-8 0z",
            "M12 2v2", "M12 20v2", "m4.93 4.93 1.41 1.41", "m17.66 17.66 1.41 1.41",
            "M2 12h2", "M20 12h2", "m6.34 17.66-1.41 1.41", "m19.07 4.93-1.41 1.41",
        ];
        type_ => ["M12 4v16", "M4 7V5a1 1 0 0 1 1-1h14a1 1 0 0 1 1 1v2", "M9 20h6"];

        // Actions and status
        download => ["M12 15V3", "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4", "m7 10 5 5 5-5"];
        trash_2 => [
            "M10 11v6", "M14 11v6", "M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6", "M3 6h18",
            "M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2",
        ];
        folder_open => [
            "m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2",
        ];
        rotate_ccw => ["M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8", "M3 3v5h5"];
        check => ["M20 6 9 17l-5-5"];
        x => ["M18 6 6 18", "m6 6 12 12"];
        plus => ["M5 12h14", "M12 5v14"];
        minus => ["M5 12h14"];
    }
}

/// Lucide's default stroke width on its 24-unit grid.
pub const ICON_STROKE: f32 = 2.0;

fn to_path(bez: &kurbo::BezPath) -> Path {
    use kurbo::PathEl;
    let point = |p: kurbo::Point| Offset::new(p.x as f32, p.y as f32);
    let mut builder = Path::builder();
    for element in bez.elements() {
        match *element {
            PathEl::MoveTo(p) => {
                builder.move_to(point(p));
            }
            PathEl::LineTo(p) => {
                builder.line_to(point(p));
            }
            PathEl::QuadTo(c, p) => {
                builder.quadratic_to(point(c), point(p));
            }
            PathEl::CurveTo(a, b, p) => {
                builder.cubic_to(point(a), point(b), point(p));
            }
            PathEl::ClosePath => {
                builder.close();
            }
        }
    }
    builder.build()
}

/// Filled geometry for one icon at one size: the stroke expanded into an
/// outline, plus the interior when the icon is drawn solid.
#[derive(Clone)]
struct IconMesh {
    outline: Arc<Path>,
    interior: Option<Arc<Path>>,
}

/// The 24-unit icon scaled to `size`, its stroke expanded on the CPU into a
/// filled outline. Expanding here (rather than asking the renderer to
/// stroke) matters for straight icons such as `minus`: their zero-height
/// bounds would otherwise be culled inside any clipped area. Cached so an
/// icon at a given size keeps one `PathId` and one GPU mesh across rebuilds.
fn icon_mesh(path: &Arc<Path>, size: f32, stroke: f32, filled: bool) -> IconMesh {
    use std::cell::RefCell;
    use std::collections::HashMap;
    type Key = (PathId, u32, u32, bool);
    thread_local! {
        static MESHES: RefCell<HashMap<Key, IconMesh>> = RefCell::new(HashMap::new());
    }
    let key = (path.id(), size.to_bits(), stroke.to_bits(), filled);
    MESHES.with(|cache| {
        cache
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| {
                let scale = f64::from(size) / 24.0;
                let mut scaled = path.bez_path().clone();
                scaled.apply_affine(kurbo::Affine::scale(scale));
                let style = kurbo::Stroke::new(f64::from(stroke) * scale)
                    .with_caps(kurbo::Cap::Round)
                    .with_join(kurbo::Join::Round);
                let outline =
                    kurbo::stroke(scaled.iter(), &style, &kurbo::StrokeOpts::default(), 0.01);
                IconMesh {
                    outline: Arc::new(to_path(&outline)),
                    interior: filled.then(|| Arc::new(to_path(&scaled))),
                }
            })
            .clone()
    })
}

/// A Lucide icon drawn the way Lucide intends: outlined, round caps and
/// joins, 2-unit stroke, on a square `size` box.
pub fn icon(path: Arc<Path>, size: f32, color: Color) -> Widget {
    icon_with(path, size, color, ICON_STROKE, false)
}

/// [`icon`] with an explicit stroke width (in 24-grid units) and an optional
/// solid fill, for tiny marks or filled glyphs such as the brand's play.
pub fn icon_with(path: Arc<Path>, size: f32, color: Color, stroke: f32, filled: bool) -> Widget {
    let mesh = icon_mesh(&path, size, stroke, filled);
    let mut canvas = Canvas::default();
    if let Some(interior) = mesh.interior {
        canvas.fill_path(interior, color, FillRule::NonZero);
    }
    canvas.fill_path(mesh.outline, color, FillRule::NonZero);
    CustomPaint::new(Size::new(size, size), canvas.finish()).into()
}

fn icon_label(glyph: Arc<Path>, label: Option<String>, size: f32, text_size: f32, color: Color) -> Widget {
    let mut children = vec![icon(glyph, size, color)];
    if let Some(label) = label {
        children.push(hgap(7.0));
        children.push(text(label, text_size, FontWeight::W600, color).into());
    }
    row(children).into()
}

/// [`button`] with a leading Lucide icon.
pub fn icon_text_button(
    glyph: Arc<Path>,
    label: impl Into<String>,
    tone: Tone,
    on_click: impl Fn() + 'static,
) -> Widget {
    let mut style = button_style(tone, 36.0);
    style.padding = Some(EdgeInsets::only(13.0, 0.0, 16.0, 0.0));
    style.alignment = Some(Alignment::CENTER);
    let color = style.foreground.unwrap_or(TEXT);
    Button::with_child(icon_label(glyph, Some(label.into()), 16.0, 13.5, color))
        .style(style)
        .on_click(on_click)
        .into()
}

/// [`small_button`] holding a Lucide icon, optionally followed by a label.
/// Without a label the button is a 28px square.
pub fn small_icon_button(
    glyph: Arc<Path>,
    label: Option<&str>,
    tone: Tone,
    on_click: impl Fn() + 'static,
) -> Widget {
    let mut style = button_style(tone, 28.0);
    style.alignment = Some(Alignment::CENTER);
    if label.is_some() {
        style.padding = Some(EdgeInsets::only(9.0, 0.0, 12.0, 0.0));
    } else {
        style.padding = Some(EdgeInsets::ZERO);
        style.fixed_size = Some(Size::new(28.0, 28.0));
    }
    let color = style.foreground.unwrap_or(TEXT);
    Button::with_child(icon_label(glyph, label.map(str::to_owned), 15.0, 12.5, color))
        .style(style)
        .on_click(on_click)
        .into()
}

/// `Column::new` over an explicitly typed child list (keeps `.into()`
/// children unambiguous).
pub fn vcol(children: Vec<Widget>) -> Column {
    Column::new(children)
}

/// `Row::new` over an explicitly typed child list.
pub fn hrow(children: Vec<Widget>) -> Row {
    Row::new(children)
}

/// `Stack::new` over an explicitly typed child list.
pub fn layers(children: Vec<Widget>) -> Stack {
    Stack::new(children)
}

/// The app logo (`assets/logo@2x.png`, 128x100), embedded in the binary.
const LOGO_PNG: &[u8] = include_bytes!("../assets/logo@2x.png");

fn logo_pixels() -> Option<&'static (u32, u32, Vec<u8>)> {
    static PIXELS: std::sync::OnceLock<Option<(u32, u32, Vec<u8>)>> = std::sync::OnceLock::new();
    PIXELS
        .get_or_init(|| {
            let rgba = image::load_from_memory(LOGO_PNG).ok()?.into_rgba8();
            Some((rgba.width(), rgba.height(), rgba.into_raw()))
        })
        .as_ref()
}

/// Logo pixels for the window/taskbar icon.
pub fn logo_rgba() -> Option<(u32, u32, Vec<u8>)> {
    logo_pixels().cloned()
}

/// The logo at `height` logical pixels, keeping its aspect ratio.
pub fn logo(height: f32) -> Widget {
    thread_local! {
        static HANDLE: Option<ImageHandle> = logo_pixels().and_then(|(w, h, px)| {
            ImageHandle::from_rgba8(*w, *h, px.clone()).ok()
        });
    }
    let Some((w, h, _)) = logo_pixels() else {
        return SizedBox::new().width(height).height(height).into();
    };
    let width = height * *w as f32 / *h as f32;
    match HANDLE.with(Clone::clone) {
        Some(handle) => Image::new(handle)
            .width(width)
            .height(height)
            .fit(BoxFit::Contain)
            .into(),
        None => SizedBox::new().width(width).height(height).into(),
    }
}
