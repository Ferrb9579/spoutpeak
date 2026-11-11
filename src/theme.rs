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
            border: Some(Border::new(
                1.0,
                if selected { ACCENT } else { LINE },
            )),
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
        background: Some(SURFACE),
        foreground: Some(TEXT),
        placeholder_color: Some(FAINT),
        border: Some(Border::new(1.0, LINE)),
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
                .border(Border::new(1.0, LINE))
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
    use kurbo::{BezPath, PathEl};
    use std::sync::OnceLock;

    fn from_svg(paths: &[&str]) -> Arc<Path> {
        let point = |p: kurbo::Point| Offset::new(p.x as f32, p.y as f32);
        let mut builder = Path::builder();
        for data in paths {
            let parsed = BezPath::from_svg(data).expect("valid Lucide path data");
            for element in parsed.elements() {
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
        }
        Arc::new(builder.build())
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

        // Actions and status
        check => ["M20 6 9 17l-5-5"];
        x => ["M18 6 6 18", "m6 6 12 12"];
        plus => ["M5 12h14", "M12 5v14"];
        minus => ["M5 12h14"];
    }
}

/// Lucide's default stroke width on its 24-unit grid.
pub const ICON_STROKE: f32 = 2.0;

/// The 24-unit icon geometry scaled to `size`. Cached so a given icon at a
/// given size keeps one `PathId` (and one retained mesh) across rebuilds.
fn scaled_icon(path: &Arc<Path>, size: f32) -> Arc<Path> {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static SCALED: RefCell<HashMap<(PathId, u32), Arc<Path>>> = RefCell::new(HashMap::new());
    }
    SCALED.with(|cache| {
        cache
            .borrow_mut()
            .entry((path.id(), size.to_bits()))
            .or_insert_with(|| Arc::new(path.transformed(AffineTransform::scale(size / 24.0))))
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
    let scaled = scaled_icon(&path, size);
    let mut canvas = Canvas::default();
    if filled {
        canvas.fill_path(scaled.clone(), color, FillRule::NonZero);
    }
    canvas.stroke_path(
        scaled,
        color,
        Stroke {
            width: stroke * size / 24.0,
            cap: LineCap::Round,
            join: LineJoin::Round,
            ..Stroke::default()
        },
    );
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
