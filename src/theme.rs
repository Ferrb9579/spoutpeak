//! Palette, typography and the small set of styled building blocks every
//! screen shares.

use incular::controls::{Button, ButtonStyle, StateColor, TextField, TextFieldStyle};
use incular::prelude::*;
use std::sync::Arc;

pub const PANEL: Color = Color::rgba(23, 23, 23, 255);

pub const BG: Color = Color::rgba(8, 8, 8, 255);
pub const SIDEBAR: Color = Color::rgba(20, 20, 20, 255);
pub const SURFACE: Color = Color::rgba(30, 30, 30, 255);
pub const SURFACE_HI: Color = Color::rgba(42, 42, 42, 255);
pub const SURFACE_HOVER: Color = Color::rgba(53, 53, 53, 255);
pub const LINE: Color = Color::rgba(51, 51, 51, 255);
pub const TEXT: Color = Color::rgba(241, 241, 241, 255);
pub const MUTED: Color = Color::rgba(159, 159, 159, 255);
pub const FAINT: Color = Color::rgba(109, 109, 109, 255);
// Monochrome: the interface is black and white; colour comes only from the
// artwork on screen (the ambient backdrop, posters, the video itself).
/// Primary actions and highlights: near-white, with black text on it.
pub const ACCENT: Color = Color::rgba(245, 245, 245, 255);
pub const ACCENT_HI: Color = Color::rgba(255, 255, 255, 255);
/// Selected rows and toggles: lit glass.
pub const ACCENT_DIM: Color = Color::rgba(255, 255, 255, 38);
pub const ON_ACCENT: Color = Color::rgba(10, 10, 10, 255);
pub const DANGER: Color = Color::rgba(240, 96, 104, 255);
pub const GOOD: Color = Color::rgba(92, 204, 150, 255);
pub const SCRIM: Color = Color::rgba(7, 7, 7, 190);

// Glass: translucent fills over the ambient backdrop (`ui::ambient`).
/// A faint lift for tiles and inactive controls.
pub const GLASS: Color = Color::rgba(255, 255, 255, 12);
/// Neutral buttons, fields.
pub const GLASS_HI: Color = Color::rgba(255, 255, 255, 20);
pub const GLASS_HOVER: Color = Color::rgba(255, 255, 255, 30);
/// The dark tint inside panels, so text stays readable over bright art.
pub const GLASS_TINT: Color = Color::rgba(0, 0, 0, 110);
/// Selected pills: near-white with dark text.
pub const SELECTED: Color = Color::rgba(243, 243, 243, 255);
pub const ON_SELECTED: Color = Color::rgba(21, 21, 21, 255);

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
    /// See-through white, for buttons over pictures and video.
    Glass,
}

fn button_style(tone: Tone, height: f32) -> ButtonStyle {
    let (background, hover, foreground) = match tone {
        Tone::Primary => (ACCENT, ACCENT_HI, ON_ACCENT),
        Tone::Neutral => (GLASS_HI, GLASS_HOVER, TEXT),
        Tone::Ghost => (Color::TRANSPARENT, GLASS_HI, MUTED),
        Tone::Danger => (Color::rgba(240, 96, 104, 34), Color::rgba(240, 96, 104, 60), DANGER),
        Tone::Glass => (Color::rgba(255, 255, 255, 34), Color::rgba(255, 255, 255, 60), TEXT),
    };
    ButtonStyle {
        background: Some(background),
        background_states: Some(StateColor::new(background).hovered(hover)),
        foreground: Some(foreground),
        border: Some(Border::new(0.0, Color::TRANSPARENT)),
        border_radius: Some(height / 2.0),
        padding: Some(EdgeInsets::symmetric(16.0, 0.0)),
        height: Some(height),
        alignment: Some(Alignment::CENTER),
        text_style: Some(style(13.5, FontWeight::W600, foreground)),
        overlay_color: Some(Color::TRANSPARENT.into()),
        animation_duration: Some(std::time::Duration::ZERO),
        ..ButtonStyle::default()
    }
}

pub fn button(label: impl Into<String>, tone: Tone, on_click: impl Fn() + 'static) -> Widget {
    let label = label.into();
    let style = button_style(tone, 36.0);
    let color = style.foreground.unwrap_or(TEXT);
    frosted(
        tone,
        36.0,
        Widget::from(Button::with_child(button_label(label.clone(), 13.5, color)).style(style).on_click(on_click))
            .accessibility_label(label),
    )
}

/// Glass buttons blur what lies behind them (a pill-shaped frosted glass).
fn frosted(tone: Tone, height: f32, button: Widget) -> Widget {
    if tone != Tone::Glass {
        return button;
    }
    ClipRRect::new(height / 2.0, incular::widgets::BackdropFilter::blur(12.0, button)).into()
}

pub fn small_button(label: impl Into<String>, tone: Tone, on_click: impl Fn() + 'static) -> Widget {
    let label = label.into();
    let mut style = button_style(tone, 28.0);
    style.padding = Some(EdgeInsets::symmetric(12.0, 0.0));
    style.text_style = Some(self::style(12.5, FontWeight::W600, style.foreground.unwrap_or(TEXT)));
    let color = style.foreground.unwrap_or(TEXT);
    frosted(tone, 28.0,
        Widget::from(Button::with_child(button_label(label.clone(), 12.5, color)).style(style).on_click(on_click))
            .accessibility_label(label))
}

/// A selectable pill, used for filters and segmented choices.
pub fn chip(label: impl Into<String>, selected: bool, on_click: impl Fn() + 'static) -> Widget {
    let (background, hover, foreground) = if selected {
        (SELECTED, SELECTED, ON_SELECTED)
    } else {
        (GLASS, GLASS_HOVER, MUTED)
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
            text_style: Some(style(
                12.5,
                if selected { FontWeight::W600 } else { FontWeight::W500 },
                foreground,
            )),
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
                StateColor::new(Color::TRANSPARENT).hovered(GLASS_HOVER),
            ),
            foreground: Some(TEXT),
            border: Some(Border::new(0.0, Color::TRANSPARENT)),
            border_radius: Some(size / 2.0),
            padding: Some(EdgeInsets::ZERO),
            fixed_size: Some(Size::new(size, size)),
            overlay_color: Some(Color::TRANSPARENT.into()),
            animation_duration: Some(std::time::Duration::ZERO),
            ..ButtonStyle::default()
        })
        .on_click(on_click)
        .into()
}

pub fn field_style() -> TextFieldStyle {
    TextFieldStyle {
        // Mid-grey glass reads on dark panels and over bright artwork alike.
        background: Some(Color::rgba(128, 128, 128, 46)),
        foreground: Some(TEXT),
        placeholder_color: Some(MUTED),
        border: Some(Border::new(0.0, Color::TRANSPARENT)),
        // The one outline left: shows which field has keyboard focus.
        border_focused: Some(Border::new(1.0, Color::rgba(255, 255, 255, 80))),
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

/// A rounded glass panel: a dark translucent tint over the ambient
/// backdrop. No outline; the fill alone sets it apart.
pub fn card(child: impl Into<Widget>) -> Container {
    glass(18.0).padding(EdgeInsets::all(20.0)).child(child)
}

/// An empty glass panel with corner `radius`, to add padding and a child to.
pub fn glass(radius: f32) -> Container {
    Container::new().decoration(
        BoxDecoration::new()
            .color(GLASS_TINT)
            .border_radius(BorderRadius::all(Radius::circular(radius))),
    )
}

/// Frosted glass: what lies behind `child` is blurred (by `sigma`), then
/// `child`, normally a [`glass`] panel, is drawn over it.
pub fn frosted_glass(child: impl Into<Widget>, radius: f32, sigma: f32) -> Widget {
    ClipRRect::new(radius, incular::widgets::BackdropFilter::blur(sigma, child)).into()
}

pub fn divider() -> Widget {
    Container::new().height(1.0).color(LINE).into()
}

/// A cached library glyph. Each source path retains its own fill rule and stroke.
pub type Glyph = Arc<IconGlyph>;

pub struct IconGlyph {
    layers: Vec<IconLayer>,
}

struct IconLayer {
    path: Arc<Path>,
    fill_rule: FillRule,
    stroke: Option<Stroke>,
}

pub mod icons;

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

/// Render the original library geometry without thickening or reshaping it.
pub fn icon(glyph: Glyph, size: f32, color: Color) -> Widget {
    struct LibraryIcon {
        glyph: Glyph,
        color: Color,
    }
    impl CustomPainter for LibraryIcon {
        fn paint(&self, size: Size) -> DisplayList {
            let mut list = Canvas::default();
            list.save_transform(AffineTransform::scale(size.width / 24.0));
            for layer in &self.glyph.layers {
                let paint = match &layer.stroke {
                    Some(stroke) => Paint::new().color(self.color).stroke().stroke_settings(stroke.clone()),
                    None => Paint::new().color(self.color).fill(),
                };
                list.draw_path(layer.path.clone(), &paint, layer.fill_rule);
            }
            list.restore();
            list.finish()
        }
        fn should_repaint(&self, old: &Self) -> bool {
            !Arc::ptr_eq(&self.glyph, &old.glyph) || self.color != old.color
        }
    }
    CustomPaint::from_painter(Size::new(size, size), LibraryIcon { glyph, color }).into()
}

/// Centre the visible glyphs, compensating for the system font's line-box
/// leading. Icons keep their geometrically centred square; labels share this
/// optical alignment in both text-only and icon-plus-text buttons.
fn button_label(label: String, size: f32, color: Color) -> Widget {
    let child: Widget = Transform::translation(
        Offset::new(0.0, -1.5),
        text(label.clone(), size, FontWeight::W600, color).max_lines(Some(1)).soft_wrap(false),
    ).into();
    child.accessibility_label(label)
}

fn icon_label(glyph: Glyph, label: Option<String>, size: f32, text_size: f32, color: Color) -> Widget {
    let mut children = vec![icon(glyph, size, color)];
    if let Some(label) = label {
        children.push(hgap(7.0));
        children.push(button_label(label, text_size, color));
    }
    row(children).into()
}

/// [`button`] with a leading icon.
pub fn icon_text_button(
    glyph: Glyph,
    label: impl Into<String>,
    tone: Tone,
    on_click: impl Fn() + 'static,
) -> Widget {
    let mut style = button_style(tone, 36.0);
    style.padding = Some(EdgeInsets::only(13.0, 0.0, 16.0, 0.0));
    let color = style.foreground.unwrap_or(TEXT);
    let button = Button::with_child(icon_label(glyph, Some(label.into()), 16.0, 13.5, color))
        .style(style)
        .on_click(on_click);
    frosted(tone, 36.0, button.into())
}

/// [`small_button`] holding an icon, optionally followed by a label.
/// Without a label the button is a 28px square.
pub fn small_icon_button(
    glyph: Glyph,
    label: Option<&str>,
    tone: Tone,
    on_click: impl Fn() + 'static,
) -> Widget {
    let mut style = button_style(tone, 28.0);
    if label.is_some() {
        style.padding = Some(EdgeInsets::only(9.0, 0.0, 12.0, 0.0));
    } else {
        style.padding = Some(EdgeInsets::ZERO);
        style.fixed_size = Some(Size::new(28.0, 28.0));
    }
    let color = style.foreground.unwrap_or(TEXT);
    let button = Button::with_child(icon_label(glyph, label.map(str::to_owned), 15.0, 12.5, color))
        .style(style)
        .on_click(on_click);
    frosted(tone, 28.0, button.into())
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

fn logo_pixels() -> Option<&'static (u32, u32, Vec<u8>)> {
    static PIXELS: std::sync::OnceLock<Option<(u32, u32, Vec<u8>)>> = std::sync::OnceLock::new();
    PIXELS
        .get_or_init(|| crate::branding::render_logo(256, false))
        .as_ref()
}

/// Logo pixels for the window/taskbar icon.
pub fn logo_rgba() -> Option<(u32, u32, Vec<u8>)> {
    crate::branding::render_logo(256, true)
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
