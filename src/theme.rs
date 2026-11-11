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

/// Hand-drawn icons: Incular ships no icon set, so the handful the player
/// and shell need are authored here on a 24-unit grid.
pub mod icons {
    use super::*;
    use std::sync::OnceLock;

    fn polygons(shapes: &[&[(f32, f32)]]) -> Arc<Path> {
        let mut builder = Path::builder();
        for shape in shapes {
            let mut points = shape.iter();
            if let Some(&(x, y)) = points.next() {
                builder.move_to(Offset::new(x, y));
            }
            for &(x, y) in points {
                builder.line_to(Offset::new(x, y));
            }
            builder.close();
        }
        Arc::new(builder.build())
    }

    macro_rules! icon {
        ($name:ident, $shapes:expr) => {
            pub fn $name() -> Arc<Path> {
                static PATH: OnceLock<Arc<Path>> = OnceLock::new();
                PATH.get_or_init(|| polygons($shapes)).clone()
            }
        };
    }

    /// A regular polygon ring (outer clockwise, inner counter-clockwise) so
    /// the non-zero fill rule leaves the middle open.
    fn ring(builder: &mut PathBuilder, cx: f32, cy: f32, outer: f32, inner: f32) {
        const SIDES: usize = 20;
        for (radius, reverse) in [(outer, false), (inner, true)] {
            for step in 0..SIDES {
                let index = if reverse { SIDES - step } else { step };
                let angle = index as f32 / SIDES as f32 * std::f32::consts::TAU;
                let point = Offset::new(cx + radius * angle.cos(), cy + radius * angle.sin());
                if step == 0 {
                    builder.move_to(point);
                } else {
                    builder.line_to(point);
                }
            }
            builder.close();
        }
    }

    fn poly(builder: &mut PathBuilder, points: &[(f32, f32)]) {
        for (index, &(x, y)) in points.iter().enumerate() {
            if index == 0 {
                builder.move_to(Offset::new(x, y));
            } else {
                builder.line_to(Offset::new(x, y));
            }
        }
        builder.close();
    }

    macro_rules! drawn {
        ($name:ident, $build:expr) => {
            pub fn $name() -> Arc<Path> {
                static PATH: OnceLock<Arc<Path>> = OnceLock::new();
                PATH.get_or_init(|| {
                    let mut builder = Path::builder();
                    let draw: fn(&mut PathBuilder) = $build;
                    draw(&mut builder);
                    Arc::new(builder.build())
                })
                .clone()
            }
        };
    }

    drawn!(search, |b| {
        ring(b, 10.5, 10.5, 7.5, 5.3);
        poly(b, &[(15.2, 16.8), (16.8, 15.2), (21.6, 20.0), (20.0, 21.6)]);
    });
    drawn!(compass, |b| {
        ring(b, 12.0, 12.0, 10.0, 8.0);
        poly(b, &[(16.5, 7.5), (13.6, 13.6), (7.5, 16.5), (10.4, 10.4)]);
    });
    drawn!(settings, |b| {
        // Three sliders with knobs.
        poly(b, &[(3.0, 5.2), (21.0, 5.2), (21.0, 6.8), (3.0, 6.8)]);
        poly(b, &[(3.0, 11.2), (21.0, 11.2), (21.0, 12.8), (3.0, 12.8)]);
        poly(b, &[(3.0, 17.2), (21.0, 17.2), (21.0, 18.8), (3.0, 18.8)]);
        poly(b, &[(13.5, 3.2), (17.5, 3.2), (17.5, 8.8), (13.5, 8.8)]);
        poly(b, &[(6.0, 9.2), (10.0, 9.2), (10.0, 14.8), (6.0, 14.8)]);
        poly(b, &[(14.5, 15.2), (18.5, 15.2), (18.5, 20.8), (14.5, 20.8)]);
    });
    icon!(
        home,
        &[&[
            (12.0, 3.0),
            (21.5, 11.2),
            (19.0, 11.2),
            (19.0, 20.5),
            (14.2, 20.5),
            (14.2, 14.5),
            (9.8, 14.5),
            (9.8, 20.5),
            (5.0, 20.5),
            (5.0, 11.2),
            (2.5, 11.2)
        ]]
    );
    icon!(
        bookmark,
        &[&[(6.0, 3.0), (18.0, 3.0), (18.0, 21.0), (12.0, 16.4), (6.0, 21.0)]]
    );
    icon!(
        puzzle,
        &[
            &[(3.5, 3.5), (10.7, 3.5), (10.7, 10.7), (3.5, 10.7)],
            &[(13.3, 13.3), (20.5, 13.3), (20.5, 20.5), (13.3, 20.5)],
            &[(3.5, 13.3), (10.7, 13.3), (10.7, 20.5), (3.5, 20.5)],
            &[(17.0, 2.2), (21.8, 7.0), (17.0, 11.8), (12.2, 7.0)]
        ]
    );

    icon!(play, &[&[(6.0, 3.0), (21.0, 12.0), (6.0, 21.0)]]);
    icon!(
        pause,
        &[
            &[(5.0, 3.0), (10.0, 3.0), (10.0, 21.0), (5.0, 21.0)],
            &[(14.0, 3.0), (19.0, 3.0), (19.0, 21.0), (14.0, 21.0)]
        ]
    );
    icon!(
        back,
        &[&[
            (3.0, 12.0),
            (11.0, 4.0),
            (12.8, 5.8),
            (7.9, 10.7),
            (21.0, 10.7),
            (21.0, 13.3),
            (7.9, 13.3),
            (12.8, 18.2),
            (11.0, 20.0)
        ]]
    );
    icon!(
        rewind,
        &[
            &[(12.0, 4.0), (12.0, 20.0), (2.0, 12.0)],
            &[(22.0, 4.0), (22.0, 20.0), (12.0, 12.0)]
        ]
    );
    icon!(
        forward,
        &[
            &[(2.0, 4.0), (12.0, 12.0), (2.0, 20.0)],
            &[(12.0, 4.0), (22.0, 12.0), (12.0, 20.0)]
        ]
    );
    icon!(
        next,
        &[
            &[(3.0, 4.0), (15.0, 12.0), (3.0, 20.0)],
            &[(17.0, 4.0), (21.0, 4.0), (21.0, 20.0), (17.0, 20.0)]
        ]
    );
    icon!(
        volume,
        &[
            &[
                (2.0, 9.0),
                (7.0, 9.0),
                (13.0, 4.0),
                (13.0, 20.0),
                (7.0, 15.0),
                (2.0, 15.0)
            ],
            &[(16.0, 8.0), (18.0, 8.0), (18.0, 16.0), (16.0, 16.0)],
            &[(20.0, 5.0), (22.0, 5.0), (22.0, 19.0), (20.0, 19.0)]
        ]
    );
    icon!(
        muted,
        &[
            &[
                (2.0, 9.0),
                (7.0, 9.0),
                (13.0, 4.0),
                (13.0, 20.0),
                (7.0, 15.0),
                (2.0, 15.0)
            ],
            &[(15.0, 11.0), (22.0, 11.0), (22.0, 13.0), (15.0, 13.0)]
        ]
    );
    icon!(
        fullscreen,
        &[
            &[(3.0, 3.0), (10.0, 3.0), (10.0, 5.5), (5.5, 5.5), (5.5, 10.0), (3.0, 10.0)],
            &[(14.0, 3.0), (21.0, 3.0), (21.0, 10.0), (18.5, 10.0), (18.5, 5.5), (14.0, 5.5)],
            &[(3.0, 14.0), (5.5, 14.0), (5.5, 18.5), (10.0, 18.5), (10.0, 21.0), (3.0, 21.0)],
            &[(18.5, 14.0), (21.0, 14.0), (21.0, 21.0), (14.0, 21.0), (14.0, 18.5), (18.5, 18.5)]
        ]
    );
    icon!(
        close,
        &[&[
            (5.0, 3.2),
            (12.0, 10.2),
            (19.0, 3.2),
            (20.8, 5.0),
            (13.8, 12.0),
            (20.8, 19.0),
            (19.0, 20.8),
            (12.0, 13.8),
            (5.0, 20.8),
            (3.2, 19.0),
            (10.2, 12.0),
            (3.2, 5.0)
        ]]
    );
    icon!(
        check,
        &[&[
            (3.0, 12.5),
            (5.0, 10.5),
            (9.5, 15.0),
            (19.0, 4.5),
            (21.0, 6.3),
            (9.6, 19.0)
        ]]
    );
    icon!(
        plus,
        &[&[
            (10.7, 3.0),
            (13.3, 3.0),
            (13.3, 10.7),
            (21.0, 10.7),
            (21.0, 13.3),
            (13.3, 13.3),
            (13.3, 21.0),
            (10.7, 21.0),
            (10.7, 13.3),
            (3.0, 13.3),
            (3.0, 10.7),
            (10.7, 10.7)
        ]]
    );
}

pub fn icon(path: Arc<Path>, size: f32, color: Color) -> Widget {
    Icon::new(path).size(size).brush(color).into()
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
