//! The player screen: picture, subtitles and transport controls.

use super::motion::{self, WheelAxis};
use crate::player::{Menu, Status, SubChoice};
use crate::player::engine::MAX_VOLUME;
use crate::state::App;
use crate::store::{SubtitleEdge, SubtitleStyle};
use crate::theme::*;
use crate::util::{format_bytes, format_speed, format_time, language_name};
use incular::controls::{Button, ButtonStyle, StateColor};
use incular::prelude::*;
use incular::widgets::{HitTestBehavior, StackFit, CompositedTransformTarget, CompositedTransformFollower, LayerLink, OverlayPortal};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

const OVERLAY: Color = Color::rgba(10, 10, 10, 205);
/// YouTube's modern control surface: black at 30%, with a 16px backdrop blur.
const CONTROL_TINT: Color = Color::rgba(0, 0, 0, 77);
// Liner MiniPlayer: 16px primary glyph, 18px secondary glyphs, 8px gaps.
const PRIMARY_ICON: f32 = 16.0;
const CONTROL_ICON: f32 = 18.0;
const PRIMARY_BUTTON: f32 = 34.0;
const CONTROL_BUTTON: f32 = 30.0;
const CONTROL_GAP: f32 = 8.0;

fn control_surface(child: impl Into<Widget>, padding: EdgeInsets) -> Widget {
    let surface = Container::new()
        .radius(24.0)
        .color(CONTROL_TINT)
        .padding(padding)
        .child(child);
    frosted_glass(surface, 24.0, 16.0)
}

fn floating_button(child: Widget, size: f32, on_click: impl Fn() + 'static) -> Widget {
    control_surface(icon_button(child, size, on_click), EdgeInsets::ZERO)
}

/// A horizontal track; `change(fraction, committed)` fires on click and
/// while dragging, with `committed` true on click and on release.
fn slider(width: f32, fraction: f32, change: Rc<dyn Fn(f32, bool)>) -> Widget {
    track(width, fraction, &[], change, Rail::THIN, None, SEEK_HOVER.with(Clone::clone))
}

/// The track's proportions.
#[derive(Clone, Copy)]
struct Rail {
    /// Height of the black rail.
    height: f32,
    /// Small fixed corners keep the track rectangular at either height.
    radius: f32,
    /// Width of the rectangular thumb.
    thumb: f32,
}

impl Rail {
    const THICK: Rail = Rail { height: 12.0, radius: 2.0, thumb: 7.0 };
    const THIN: Rail = Rail { height: 8.0, radius: 1.5, thumb: 6.0 };
}

/// Played: near-white. Downloaded but not played: grey. The rest: the black
/// rail showing through.
const PLAYED: Color = Color::rgba(244, 244, 244, 255);
const LOADED: Color = Color::rgba(184, 184, 184, 255);
const RAIL: Color = CONTROL_TINT;
/// How far above the rail the time bubble floats (outside the bar's box).
const BUBBLE_WIDTH: f32 = 78.0;
const BUBBLE_HEIGHT: f32 = 24.0;
const POINTER_WIDTH: f32 = 14.0;
const POINTER_HEIGHT: f32 = 7.0;
const BUBBLE_GAP: f32 = 4.0;

thread_local! {
    /// Where the pointer is over the seek bar (a fraction), for its bubble.
    /// Kept outside the bar, which rebuilds on every position tick.
    static SEEK_HOVER: Signal<Option<f32>> = Signal::new(None);
    static VOLUME_HOVER: Signal<bool> = Signal::new(false);
    static VOLUME_DRAGGING: Signal<bool> = Signal::new(false);
    static VOLUME_POSITION: Signal<Option<f32>> = Signal::new(None);
    static VOLUME_LINK: LayerLink = LayerLink::default();
    static BUTTON_TIPS: std::cell::RefCell<std::collections::HashMap<String, Signal<bool>>> = Default::default();
}

/// A borderless, boxy rail with grey `buffered` (start, end) stretches,
/// a white played part and a softly squared white thumb.
/// With `label`, a bubble over the pointer (or the thumb, while
/// dragging) shows `label(fraction)`.
fn track(
    width: f32,
    fraction: f32,
    buffered: &[(f32, f32)],
    change: Rc<dyn Fn(f32, bool)>,
    rail: Rail,
    label: Option<Rc<dyn Fn(f32) -> String>>,
    hover: Signal<Option<f32>>,
) -> Widget {
    let fraction = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
    let width = width.max(0.0);
    let thumb_height = rail.height + 4.0;
    // Keep an easy-to-grab hit area without adding a visible frame.
    let hit_height = 28.0;
    let top = (hit_height - thumb_height) / 2.0;
    let rail_top = (hit_height - rail.height) / 2.0;
    let segment = |start: f32, end: f32, color: Color| -> Option<Widget> {
        let (start, end) = (start.clamp(0.0, 1.0), end.clamp(0.0, 1.0));
        let span = (end - start) * width;
        (span >= 1.0).then(|| {
            Positioned::new(
                Container::new()
                    .width(span)
                    .height(rail.height)
                    .radius(rail.radius.min(span / 2.0))
                    .color(color),
            )
            .left(start * width)
            .top(rail_top)
            .into()
        })
    };

    let mut parts: Vec<Widget> = vec![
        Positioned::new(
            frosted_glass(
                Container::new().width(width).height(rail.height).radius(rail.radius).color(RAIL),
                rail.radius,
                16.0,
            ),
        )
        .left(0.0)
        .top(rail_top)
        .into(),
    ];
    // Separate downloaded stretches keep a hairline of rail between them.
    for &(start, end) in buffered {
        let gap = 1.0 / width.max(1.0);
        parts.extend(segment(
            if start > 0.0 { start + gap } else { 0.0 },
            if end < 1.0 { end - gap } else { 1.0 },
            LOADED,
        ));
    }
    parts.extend(segment(0.0, fraction, PLAYED));
    let thumb_x = (fraction * width - rail.thumb / 2.0).clamp(0.0, (width - rail.thumb).max(0.0));
    parts.push(
        Positioned::new(
            Container::new()
                .width(rail.thumb)
                .height(thumb_height)
                .radius(2.0)
                .color(Color::WHITE),
        )
        .left(thumb_x)
        .top(top)
        .into(),
    );
    if let Some(label) = label.clone() {
        parts.push(bubble(width, rail_top, label, hover.clone()));
    }

    let visual = SizedBox::new()
        .width(width)
        .height(hit_height)
        .child(layers(parts).clip_behavior(incular::config::Clip::None));
    let at = move |x: f32| (x / width.max(1.0)).clamp(0.0, 1.0);
    let hovering = label.is_some();
    let set_hover = move |value: Option<f32>| {
        if hovering {
            hover.set(value);
        }
    };
    let last = Rc::new(std::cell::Cell::new(fraction));
    let (tap, drag, release, cancel) = (change.clone(), change.clone(), change.clone(), change);
    let (drag_last, release_last, cancel_last) = (last.clone(), last.clone(), last);
    let drag_hover = set_hover.clone();
    let gestures = GestureDetector::new(visual)
        .behavior(HitTestBehavior::Opaque)
        .on_tap_down(move |details| tap(at(details.local_position.x), true))
        .on_horizontal_drag_update(move |details| {
            let value = at(details.local_position.x);
            drag_last.set(value);
            drag_hover(Some(value));
            drag(value, false);
        })
        .on_horizontal_drag_end(move |_| release(release_last.get(), true))
        .on_horizontal_drag_cancel(move || cancel(cancel_last.get(), true));
    if !hovering {
        return gestures.into();
    }
    let exit_hover = set_hover.clone();
    MouseRegion::new(gestures)
        .on_hover(move |event| set_hover(Some(at(event.local_position.x))))
        .on_exit(move |_| exit_hover(None))
        .into()
}

/// The time bubble: a white rounded label with a small pointer, above the
/// rail at the hovered position. Rebuilds alone as the pointer moves.
fn bubble_geometry(width: f32, fraction: f32) -> (f32, f32) {
    let centre = fraction.clamp(0.0, 1.0) * width;
    let left = (centre - BUBBLE_WIDTH / 2.0).clamp(0.0, (width - BUBBLE_WIDTH).max(0.0));
    // Clamp the label, never its arrow: the tip follows the actual pointer
    // even at the beginning and end of the timeline.
    (left, centre - POINTER_WIDTH / 2.0)
}

#[cfg(test)]
mod bubble_tests {
    use super::*;

    #[test]
    fn arrow_tracks_the_pointer_even_when_the_label_is_clamped() {
        for fraction in [0.0, 0.01, 0.5, 0.99, 1.0] {
            let (label, arrow) = bubble_geometry(800.0, fraction);
            assert_eq!(arrow + POINTER_WIDTH / 2.0, fraction * 800.0);
            assert!(label >= 0.0 && label + BUBBLE_WIDTH <= 800.0);
        }
    }

    #[test]
    fn volume_popup_keeps_its_label_inside_the_short_track() {
        assert_eq!(bubble_geometry(84.0, 0.0).0, 0.0);
        assert_eq!(bubble_geometry(84.0, 1.0).0, 84.0 - BUBBLE_WIDTH);
    }
}

fn bubble(width: f32, rail_top: f32, label: Rc<dyn Fn(f32) -> String>, hover: Signal<Option<f32>>) -> Widget {
    Positioned::new(IgnorePointer::new(Builder::new(move |_| {
        let Some(at) = hover.get() else {
            return SizedBox::shrink().into();
        };
        let (left, pointer) = bubble_geometry(width, at);
        SizedBox::new().width(width).height(BUBBLE_HEIGHT + POINTER_HEIGHT)
            .child(layers(vec![
                Positioned::new(Container::new()
                    .width(BUBBLE_WIDTH)
                    .height(BUBBLE_HEIGHT)
                    .alignment(Alignment::CENTER)
                    .radius(4.0)
                    .color(Color::WHITE)
                    .child(text(label(at), 12.5, FontWeight::W600, ON_SELECTED)))
                    .left(left).top(0.0).into(),
                Positioned::new(CustomPaint::from_painter(Size::new(POINTER_WIDTH, POINTER_HEIGHT), Pointer(false)))
                    .left(pointer).top(BUBBLE_HEIGHT).into(),
            ]).clip_behavior(incular::config::Clip::None))
        .into()
    })))
    .left(0.0)
    .top(rail_top - BUBBLE_HEIGHT - POINTER_HEIGHT - BUBBLE_GAP)
    .into()
}

/// The bubble's downward pointer: a small white triangle.
struct Pointer(bool);

impl CustomPainter for Pointer {
    fn paint(&self, size: Size) -> DisplayList {
        let mut builder = Path::builder();
        let (base, tip) = if self.0 { (size.height, 0.0) } else { (0.0, size.height) };
        builder.move_to(Offset::new(0.0, base));
        builder.line_to(Offset::new(size.width, base));
        builder.line_to(Offset::new(size.width / 2.0, tip));
        builder.close();
        let mut canvas = Canvas::default();
        canvas.fill_path(Arc::new(builder.build()), Color::WHITE, FillRule::NonZero);
        canvas.finish()
    }
    fn should_repaint(&self, _: &Self) -> bool {
        false
    }
}

/// The same white label and arrow used by the timeline, anchored to a control.
fn player_tip(label: impl Into<String>, child: Widget) -> Widget {
    player_tip_at(label, child, false)
}

fn player_tip_at(label: impl Into<String>, child: Widget, below: bool) -> Widget {
    let label = label.into();
    let child = child.accessibility_label(label.clone());
    let hover = BUTTON_TIPS.with(|tips| tips.borrow_mut()
        .entry(label.clone()).or_insert_with(|| Signal::new(false)).clone());
    Builder::new(move |_| {
        let visible = hover.get();
        let (enter, exit) = (hover.clone(), hover.clone());
        let anchor = MouseRegion::new(child.clone())
            .on_enter(move |_| { enter.set(true); })
            .on_exit(move |_| { exit.set(false); });
        let caption = label.clone();
        OverlayPortal::new(anchor).presentation(incular::config::TransientPresentation::Overlay)
            .role(incular::config::TransientRole::Tooltip).root_overlay(true)
            .overlay_child_layout_builder(move |_, info| {
                if info.overlay_size.width <= 16.0 { return SizedBox::shrink().into(); }
                let centre = info.anchor_rect.origin.x + info.anchor_rect.size.width / 2.0;
                let width = (caption.chars().count() as f32 * 7.0 + 18.0).min(250.0).min(info.overlay_size.width - 16.0);
                let left = (centre - width / 2.0).clamp(8.0, info.overlay_size.width - width - 8.0);
                let top = if below {
                    info.anchor_rect.origin.y + info.anchor_rect.size.height + BUBBLE_GAP
                } else {
                    info.anchor_rect.origin.y - BUBBLE_GAP - BUBBLE_HEIGHT - POINTER_HEIGHT
                };
                IgnorePointer::new(Stack::new(vec![
                    Positioned::new(Container::new().width(width).height(BUBBLE_HEIGHT)
                        .alignment(Alignment::CENTER).radius(4.0).color(Color::WHITE)
                        .child(text(caption.clone(), 12.5, FontWeight::W600, ON_SELECTED)))
                        .left(left).top(top + if below { POINTER_HEIGHT } else { 0.0 }).into(),
                    Positioned::new(CustomPaint::from_painter(Size::new(POINTER_WIDTH, POINTER_HEIGHT), Pointer(below)))
                        .left(centre - POINTER_WIDTH / 2.0).top(top + if below { 0.0 } else { BUBBLE_HEIGHT }).into(),
                ] as Vec<Widget>).fit(StackFit::Expand)).into()
            })
            .show(visible).into()
    }).into()
}

fn seek_bar(app: &App) -> Widget {
    let app = app.clone();
    LayoutBuilder::new(move |_, constraints| {
        let width = constraints.max_width();
        let position = app.player.position.get();
        let (duration, buffered) = app
            .player
            .state
            .peek(|s| (s.duration, s.torrent.as_ref().map(|t| t.ranges.clone()).unwrap_or_default()));
        let fraction = if duration > 0.0 { (position / duration) as f32 } else { 0.0 };
        let label: Option<Rc<dyn Fn(f32) -> String>> = (duration > 0.0)
            .then(|| Rc::new(move |at: f32| format_time(f64::from(at) * duration)) as Rc<dyn Fn(f32) -> String>);
        let app = app.clone();
        track(
            width,
            fraction,
            &buffered,
            Rc::new(move |value, committed| {
                if duration <= 0.0 {
                    return;
                }
                let target = value as f64 * duration;
                if committed {
                    app.player.scrub.set(None);
                    app.seek_to(target);
                } else {
                    app.player.scrub.set(Some(target));
                    app.player.position.set(target);
                    app.player_activity();
                }
            }),
            Rail::THICK,
            label,
            SEEK_HOVER.with(Clone::clone),
        )
    })
    .into()
}

fn clock(app: &App) -> Widget {
    let app = app.clone();
    Builder::new(move |_| {
        let position = app.player.position.get();
        let (duration, live) = app.player.state.peek(|s| (s.duration, s.live));
        if live {
            return row(vec![
                Container::new().width(8.0).height(8.0).radius(4.0).color(DANGER).into(),
                hgap(7.0),
                text("LIVE", 12.0, FontWeight::W700, TEXT).into(),
            ])
            .into();
        }
        let label = if duration > 0.0 {
            format!("{} / {}", format_time(position), format_time(duration))
        } else {
            format_time(position)
        };
        text(label, 12.5, FontWeight::W500, TEXT).into()
    })
    .into()
}

/// Rounded translucent button used inside the player panels.
fn pill_content(
    child: Widget,
    active: bool,
    fixed: Option<Size>,
    on_click: impl Fn() + 'static,
) -> Widget {
    let background = if active { ACCENT_DIM } else { Color::rgba(255, 255, 255, 22) };
    let foreground = if active { ACCENT } else { TEXT };
    Button::with_child(child)
        .style(ButtonStyle {
            background: Some(background),
            background_states: Some(
                StateColor::new(background).hovered(if active {
                    ACCENT_DIM
                } else {
                    Color::rgba(255, 255, 255, 48)
                }),
            ),
            foreground: Some(foreground),
            border: Some(Border::new(0.0, Color::TRANSPARENT)),
            border_radius: Some(15.0),
            padding: Some(if fixed.is_some() {
                EdgeInsets::ZERO
            } else {
                EdgeInsets::symmetric(12.0, 0.0)
            }),
            height: Some(30.0),
            fixed_size: fixed,
            overlay_color: Some(Color::TRANSPARENT.into()),
            animation_duration: Some(std::time::Duration::ZERO),
            ..ButtonStyle::default()
        })
        .on_click(on_click)
        .into()
}

fn pill(label: impl Into<String>, active: bool, on_click: impl Fn() + 'static) -> Widget {
    let color = if active { ACCENT } else { TEXT };
    pill_content(text(label, 12.5, FontWeight::W600, color).into(), active, None, on_click)
}

/// A round 30px pill holding a library icon.
fn icon_pill(label: &str, glyph: Glyph, on_click: impl Fn() + 'static) -> Widget {
    player_tip(label, pill_content(icon(glyph, 15.0, TEXT), false, Some(Size::new(30.0, 30.0)), on_click))
}

/// A pill with a leading icon and a label.
fn labeled_pill(glyph: Glyph, label: &str, on_click: impl Fn() + 'static) -> Widget {
    pill_content(
        row(vec![
            icon(glyph, 14.0, TEXT),
            hgap(6.0),
            text(label, 12.5, FontWeight::W600, TEXT).into(),
        ])
        .into(),
        false,
        None,
        on_click,
    )
}

/// Inner width of a menu panel.
const PANEL_INNER: f32 = 296.0;
const MENU_ROW: f32 = 34.0;

fn menu_item(label: String, selected: bool, on_click: impl Fn() + 'static) -> Widget {
    let background = if selected { ACCENT_DIM } else { Color::TRANSPARENT };
    let color = if selected { ACCENT } else { TEXT };
    let mut content: Vec<Widget> = vec![
        Expanded::new(line(
            label,
            13.0,
            if selected { FontWeight::W600 } else { FontWeight::W500 },
            color,
        ))
        .into(),
    ];
    if selected {
        content.push(hgap(8.0));
        content.push(icon(icons::check(), 16.0, ACCENT));
    }
    Button::with_child(
        SizedBox::new()
            .width(PANEL_INNER - 24.0)
            .child(hrow(content).cross_axis_alignment(CrossAxisAlignment::Center)),
    )
    .style(ButtonStyle {
        background: Some(background),
        background_states: Some(
            StateColor::new(background).hovered(if selected {
                ACCENT_DIM
            } else {
                Color::rgba(255, 255, 255, 30)
            }),
        ),
        foreground: Some(color),
        border: Some(Border::new(0.0, Color::TRANSPARENT)),
        border_radius: Some(8.0),
        padding: Some(EdgeInsets::symmetric(12.0, 0.0)),
        fixed_size: Some(Size::new(PANEL_INNER, MENU_ROW - 2.0)),
        alignment: Some(Alignment::CENTER_LEFT),
        overlay_color: Some(Color::TRANSPARENT.into()),
        animation_duration: Some(std::time::Duration::ZERO),
        ..ButtonStyle::default()
    })
    .on_click(on_click)
    .into()
}

fn track_label(language: &str, title: &str, fallback: usize) -> String {
    let language = if language.is_empty() {
        format!("Track {}", fallback + 1)
    } else {
        language_name(language)
    };
    if title.is_empty() {
        language
    } else {
        format!("{language} - {title}")
    }
}

/// Every selectable subtitle with the label the menu shows for it, in menu
/// order (off, embedded tracks, addon tracks sorted by language).
fn subtitle_entries(app: &App) -> Vec<(SubChoice, String)> {
    let (embedded, addon) = app
        .player
        .state
        .peek(|s| (s.embedded_subs.clone(), s.addon_subs.clone()));
    let mut entries = vec![(SubChoice::Off, "Off".to_owned())];
    for (index, track) in embedded.iter().enumerate() {
        if track.is_text_subtitle() {
            entries.push((
                SubChoice::Embedded(index),
                format!("{}  (embedded)", track_label(&track.language, &track.title, index)),
            ));
        }
    }
    let mut ordered: Vec<(usize, String)> = addon
        .iter()
        .enumerate()
        .map(|(i, s)| (i, language_name(s.lang.as_deref().unwrap_or(""))))
        .collect();
    ordered.sort_by(|a, b| a.1.cmp(&b.1));
    let mut seen: std::collections::HashMap<String, usize> = Default::default();
    for (index, language) in ordered {
        let number = seen.entry(language.clone()).or_insert(0);
        *number += 1;
        entries.push((SubChoice::Addon(index), format!("{language}  #{number}")));
    }
    entries
}

/// Children of a menu panel plus their total height, so the scroll view
/// can be offered exactly what it needs.
struct Panel {
    children: Vec<Widget>,
    height: f32,
}

impl Panel {
    fn new() -> Self {
        Self { children: Vec::new(), height: 0.0 }
    }
    fn push(&mut self, widget: impl Into<Widget>, height: f32) {
        self.children.push(widget.into());
        self.height += height;
    }
    fn gap(&mut self, height: f32) {
        self.push(gap(height), height);
    }
    fn label(&mut self, value: &str) {
        self.push(
            Padding::new(
                EdgeInsets::only(4.0, 0.0, 0.0, 0.0),
                text(value.to_uppercase(), 10.5, FontWeight::W700, FAINT),
            ),
            16.0,
        );
        self.gap(8.0);
    }
    fn row(&mut self, children: Vec<Widget>) {
        self.push(
            Padding::new(EdgeInsets::only(4.0, 0.0, 0.0, 0.0), row(children)),
            30.0,
        );
    }
}

/// Title row of a panel, with an optional way back to a parent panel.
fn panel_header(app: &App, title: &str, back: Option<Menu>, trailing: Vec<Widget>) -> Widget {
    let mut children: Vec<Widget> = Vec::new();
    if let Some(parent) = back {
        let app = app.clone();
        children.push(player_tip("Back to subtitles", icon_button(icon(icons::chevron_left(), 18.0, TEXT), 28.0, move || {
            app.set_player_menu(parent)
        })));
        children.push(hgap(4.0));
    } else {
        children.push(hgap(4.0));
    }
    children.push(text(title, 14.0, FontWeight::W600, TEXT).into());
    children.push(Spacer::new().into());
    children.extend(trailing);
    SizedBox::new()
        .width(PANEL_INNER)
        .height(30.0)
        .child(hrow(children).cross_axis_alignment(CrossAxisAlignment::Center))
        .into()
}

fn speed_label(speed: f64) -> String {
    if (speed - 1.0).abs() < 0.01 {
        "Normal".to_owned()
    } else {
        format!("{speed}×")
    }
}

fn speed_panel(app: &App) -> Panel {
    let current = app.player.state.peek(|s| s.speed);
    let mut panel = Panel::new();
    panel.push(panel_header(app, "Playback speed", None, Vec::new()), 30.0);
    panel.gap(10.0);
    for speed in [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0] {
        let app = app.clone();
        panel.push(
            menu_item(speed_label(speed), (current - speed).abs() < 0.01, move || {
                app.set_speed(speed)
            }),
            MENU_ROW,
        );
    }
    panel
}

fn audio_panel(app: &App) -> Panel {
    let (tracks, current) = app
        .player
        .state
        .peek(|s| (s.audio_tracks.clone(), s.audio_index));
    let mut panel = Panel::new();
    panel.push(panel_header(app, "Audio track", None, Vec::new()), 30.0);
    panel.gap(10.0);
    if tracks.is_empty() {
        panel.push(Padding::new(EdgeInsets::only(4.0, 0.0, 0.0, 0.0), muted("No audio tracks")), 20.0);
    }
    // The track in use first, then the rest in file order.
    let mut order: Vec<usize> = (0..tracks.len()).collect();
    order.sort_by_key(|&index| index != current);
    for index in order {
        let track = &tracks[index];
        let app = app.clone();
        panel.push(
            menu_item(
                track_label(&track.language, &track.title, index),
                index == current,
                move || app.select_audio(index),
            ),
            MENU_ROW,
        );
    }
    panel
}

/// How far the selected subtitle has loaded, for the menus. `None` when
/// subtitles are off.
fn subtitle_progress(app: &App) -> Option<(String, bool)> {
    let (choice, until, loaded, duration) = app
        .player
        .state
        .peek(|s| (s.sub_choice.clone(), s.sub_loaded_until, s.sub_loaded, s.duration));
    if choice == SubChoice::Off {
        return None;
    }
    let count = app.player.cue_count();
    if loaded {
        return Some((format!("All {count} lines loaded"), true));
    }
    if matches!(choice, SubChoice::Addon(_)) {
        return Some(("Downloading subtitle…".to_owned(), false));
    }
    let label = if duration > 0.0 && until > 0.0 {
        let percent = (until / duration * 100.0).clamp(0.0, 99.0);
        format!(
            "Loading lines: {count} so far, up to {} of {} ({percent:.0}%)",
            format_time(until),
            format_time(duration)
        )
    } else if count > 0 {
        format!("Loading lines: {count} so far")
    } else {
        "Loading lines…".to_owned()
    };
    Some((label, false))
}

fn subtitles_panel(app: &App) -> Panel {
    let (choice, delay) = app.player.state.peek(|s| (s.sub_choice.clone(), s.sub_delay));
    let entries = subtitle_entries(app);
    let current = entries
        .iter()
        .find(|(c, _)| *c == choice)
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| "Off".to_owned());

    let mut panel = Panel::new();
    let style = app.clone();
    panel.push(
        panel_header(
            app,
            "Subtitles",
            None,
            vec![labeled_pill(icons::type_(), "Style", move || {
                style.set_player_menu(Menu::SubtitleStyle)
            })],
        ),
        30.0,
    );
    panel.gap(8.0);
    panel.push(
        Padding::new(
            EdgeInsets::only(4.0, 0.0, 0.0, 0.0),
            row(vec![
                text("Showing", 12.0, FontWeight::W500, MUTED).into(),
                hgap(8.0),
                SizedBox::new()
                    .width(PANEL_INNER - 70.0)
                    .child(line(current, 12.5, FontWeight::W600, ACCENT))
                    .into(),
            ]),
        ),
        18.0,
    );
    panel.gap(6.0);
    if let Some((label, done)) = subtitle_progress(app) {
        panel.push(
            Padding::new(
                EdgeInsets::only(4.0, 0.0, 0.0, 0.0),
                SizedBox::new()
                    .width(PANEL_INNER - 8.0)
                    .child(line(label, 11.5, FontWeight::W500, if done { FAINT } else { ACCENT })),
            ),
            16.0,
        );
    }
    panel.gap(10.0);

    if choice != SubChoice::Off {
        let (earlier, later, sync) = (app.clone(), app.clone(), app.clone());
        panel.row(vec![
            SizedBox::new()
                .width(110.0)
                .child(text(format!("Delay {delay:+.2}s"), 12.5, FontWeight::W500, MUTED))
                .into(),
            icon_pill("Subtitles earlier", icons::minus(), move || earlier.adjust_subtitle_delay(-0.25)),
            hgap(6.0),
            icon_pill("Subtitles later", icons::plus(), move || later.adjust_subtitle_delay(0.25)),
            hgap(6.0),
            pill("Sync…", false, move || sync.open_subtitle_sync()),
        ]);
        panel.gap(12.0);
    }

    panel.label("Tracks");
    // The track in use first, then everything else in menu order.
    let (selected, others): (Vec<_>, Vec<_>) =
        entries.into_iter().partition(|(c, _)| *c == choice);
    for (entry, label) in selected.into_iter().chain(others) {
        let app = app.clone();
        let is_current = entry == choice;
        panel.push(
            menu_item(label, is_current, move || app.select_subtitle(entry.clone())),
            MENU_ROW,
        );
    }
    panel
}

const PALETTE: [[u8; 3]; 8] = [
    [255, 255, 255],
    [255, 235, 59],
    [76, 230, 106],
    [0, 229, 255],
    [66, 133, 244],
    [234, 64, 251],
    [244, 67, 54],
    [0, 0, 0],
];

fn rgb(color: [u8; 3], opacity: f32) -> Color {
    Color::rgba(
        color[0],
        color[1],
        color[2],
        (opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// A colour dot; the chosen one carries a check.
fn swatch(color: [u8; 3], selected: bool, on_click: impl Fn() + 'static) -> Widget {
    let fill = rgb(color, 1.0);
    let light = u32::from(color[0]) * 3 + u32::from(color[1]) * 6 + u32::from(color[2]) > 1100;
    let child: Widget = if selected {
        icon(icons::check(), 16.0, if light { BG } else { Color::WHITE })
    } else {
        SizedBox::shrink().into()
    };
    Button::with_child(child)
        .style(ButtonStyle {
            background: Some(fill),
            background_states: Some(StateColor::new(fill)),
            foreground: Some(TEXT),
            border: Some(Border::new(0.0, Color::TRANSPARENT)),
            border_radius: Some(14.0),
            padding: Some(EdgeInsets::ZERO),
            fixed_size: Some(Size::new(28.0, 28.0)),
            overlay_color: Some(Color::TRANSPARENT.into()),
            animation_duration: Some(std::time::Duration::ZERO),
            ..ButtonStyle::default()
        })
        .on_click(on_click)
        .into()
}

fn swatches(
    app: &App,
    current: [u8; 3],
    set: fn(&mut SubtitleStyle, [u8; 3]),
) -> Vec<Widget> {
    let mut children = Vec::new();
    for color in PALETTE {
        let app = app.clone();
        children.push(swatch(color, color == current, move || {
            app.update_subtitle_style(|style| set(style, color))
        }));
        children.push(hgap(8.0));
    }
    children
}

fn opacity_pills(
    app: &App,
    current: f32,
    choices: &[f32],
    set: fn(&mut SubtitleStyle, f32),
) -> Vec<Widget> {
    let mut children = Vec::new();
    for &value in choices {
        let app = app.clone();
        children.push(pill(
            format!("{:.0}%", value * 100.0),
            (current - value).abs() < 0.01,
            move || app.update_subtitle_style(|style| set(style, value)),
        ));
        children.push(hgap(6.0));
    }
    children
}

fn subtitle_style_panel(app: &App) -> Panel {
    let (style, size) = app.player.state.peek(|s| (s.sub_style.clone(), s.sub_size));
    let mut panel = Panel::new();
    let reset = app.clone();
    panel.push(
        panel_header(
            app,
            "Subtitle style",
            Some(Menu::Subtitles),
            vec![labeled_pill(icons::rotate_ccw(), "Reset", move || {
                reset.reset_subtitle_style()
            })],
        ),
        30.0,
    );
    panel.gap(12.0);

    // Live preview on a picture-like backdrop.
    panel.push(
        Container::new()
            .width(PANEL_INNER)
            .height(72.0)
            .alignment(Alignment::CENTER)
            .decoration(
                BoxDecoration::new()
                    .color(Color::rgba(72, 72, 72, 255))
                    .border_radius(BorderRadius::all(Radius::circular(10.0))),
            )
            .child(caption_block(&["Subtitles look like this"], size.min(22.0), &style)),
        72.0,
    );
    panel.gap(14.0);

    let (smaller, bigger) = (app.clone(), app.clone());
    panel.label("Font size");
    panel.row(vec![
        icon_pill("Smaller subtitles", icons::minus(), move || smaller.adjust_subtitle_size(-2.0)),
        SizedBox::new()
            .width(56.0)
            .child(
                Column::new(vec![text(format!("{size:.0}"), 13.0, FontWeight::W600, TEXT).into()]
                    as Vec<Widget>)
                .cross_axis_alignment(CrossAxisAlignment::Center),
            )
            .into(),
        icon_pill("Larger subtitles", icons::plus(), move || bigger.adjust_subtitle_size(2.0)),
        hgap(12.0),
        {
            let app = app.clone();
            pill("Bold", style.bold, move || {
                app.update_subtitle_style(|s| s.bold = !s.bold)
            })
        },
    ]);
    panel.gap(14.0);

    panel.label("Text colour");
    panel.push(
        Padding::new(
            EdgeInsets::only(4.0, 0.0, 0.0, 0.0),
            row(swatches(app, style.text_color, |s, c| s.text_color = c)),
        ),
        28.0,
    );
    panel.gap(8.0);
    panel.row(opacity_pills(app, style.text_opacity, &[0.25, 0.5, 0.75, 1.0], |s, v| {
        s.text_opacity = v
    }));
    panel.gap(14.0);

    panel.label("Background");
    panel.push(
        Padding::new(
            EdgeInsets::only(4.0, 0.0, 0.0, 0.0),
            row(swatches(app, style.background_color, |s, c| s.background_color = c)),
        ),
        28.0,
    );
    panel.gap(8.0);
    panel.row(opacity_pills(
        app,
        style.background_opacity,
        &[0.0, 0.25, 0.5, 0.75, 1.0],
        |s, v| s.background_opacity = v,
    ));
    panel.gap(14.0);

    panel.label("Window");
    panel.push(
        Padding::new(
            EdgeInsets::only(4.0, 0.0, 0.0, 0.0),
            row(swatches(app, style.window_color, |s, c| s.window_color = c)),
        ),
        28.0,
    );
    panel.gap(8.0);
    panel.row(opacity_pills(
        app,
        style.window_opacity,
        &[0.0, 0.25, 0.5, 0.75, 1.0],
        |s, v| s.window_opacity = v,
    ));
    panel.gap(14.0);

    panel.label("Character edge");
    for edges in SubtitleEdge::ALL.chunks(3) {
        let mut children = Vec::new();
        for &edge in edges {
            let app = app.clone();
            children.push(pill(edge.label(), style.edge == edge, move || {
                app.update_subtitle_style(|s| s.edge = edge)
            }));
            children.push(hgap(6.0));
        }
        panel.row(children);
        panel.gap(6.0);
    }
    panel
}

fn picture_panel(app: &App) -> Panel {
    let brightness = app.player.state.peek(|s| s.brightness);
    let mut panel = Panel::new();
    let reset = app.clone();
    panel.push(
        panel_header(
            app,
            "Brightness",
            None,
            vec![
                text(format!("{:.0}%", brightness * 100.0), 12.5, FontWeight::W600, ACCENT).into(),
                hgap(8.0),
                labeled_pill(icons::rotate_ccw(), "Reset", move || reset.set_brightness(1.0)),
            ],
        ),
        30.0,
    );
    panel.gap(14.0);
    let (darker, brighter, drag) = (app.clone(), app.clone(), app.clone());
    panel.row(vec![
        icon_pill("Darker", icons::minus(), move || darker.adjust_brightness(-0.01)),
        hgap(10.0),
        slider(
            PANEL_INNER - 98.0,
            (brightness - 0.3) / 1.4,
            Rc::new(move |value, _| drag.set_brightness(0.3 + value * 1.4)),
        ),
        hgap(10.0),
        icon_pill("Brighter", icons::plus(), move || brighter.adjust_brightness(0.01)),
    ]);
    panel.gap(12.0);
    let mut presets = Vec::new();
    for value in [0.5f32, 0.75, 1.0, 1.25, 1.5] {
        let app = app.clone();
        presets.push(pill(
            format!("{:.0}%", value * 100.0),
            (brightness - value).abs() < 0.005,
            move || app.set_brightness(value),
        ));
        presets.push(hgap(5.0));
    }
    panel.row(presets);
    panel.gap(8.0);
    panel.push(
        Padding::new(
            EdgeInsets::only(4.0, 0.0, 0.0, 0.0),
            text("Steps of 1%. Keys: B brighter, Shift+B darker.", 11.5, FontWeight::W400, FAINT),
        ),
        16.0,
    );
    panel.gap(6.0);
    panel
}

fn more_panel(app: &App) -> Panel {
    let mut panel = Panel::new();
    panel.push(panel_header(app, "Video actions", None, Vec::new()), 30.0);
    panel.gap(8.0);
    let download = app.clone();
    panel.push(menu_item("Download Video".into(), false, move || {
        download.set_player_menu(Menu::None);
        download.download_current_video();
    }), MENU_ROW);
    let title = app.player.state.peek(|s| s.title.clone());
    let mut copies: Vec<(&str, String)> = Vec::new();
    if let Some((stream, _, _)) = app.player.current_media() {
        if let Some(url) = crate::player::source::magnet_url(&stream, &title) {
            copies.push(("Copy magnet URL", url));
            if let crate::addons::types::StreamSource::Torrent { info_hash, .. } = stream.source() {
                copies.push(("Copy torrent hash", info_hash));
            }
        } else if let Some(source) = crate::player::source::source_copy(&stream) {
            copies.push(source);
        }
        let details = stream.details();
        if !details.is_empty() { copies.push(("Copy source details", details)); }
    }
    if !title.is_empty() { copies.push(("Copy title", title)); }
    for (label, value) in copies {
        let app = app.clone();
        panel.push(menu_item(label.to_owned(), false, move || {
            app.set_player_menu(Menu::None);
            crate::state::copy_text(value.clone(), format!("{} copied", label.trim_start_matches("Copy ")));
        }), MENU_ROW);
    }
    panel
}

fn menu_panel(app: &App, menu: Menu) -> Option<Widget> {
    let panel = match menu {
        Menu::None => return None,
        Menu::Sync => return Some(sync_panel(app)),
        Menu::Speed => speed_panel(app),
        Menu::Audio => audio_panel(app),
        Menu::Subtitles => subtitles_panel(app),
        Menu::SubtitleStyle => subtitle_style_panel(app),
        Menu::Picture => picture_panel(app),
        Menu::More => more_panel(app),
        // Drawn outside the focus-excluded controls (it has a text field).
        Menu::Together | Menu::Peers => return None,
    };
    let height = panel.height.min(460.0);
    Some(
        Container::new()
            .width(320.0)
            .constraints(Constraints::tight_for(Some(320.0), None))
            .padding(EdgeInsets::all(12.0))
            .decoration(
                BoxDecoration::new()
                    .color(Color::rgba(24, 24, 24, 245))
                    .border_radius(BorderRadius::all(Radius::circular(14.0))),
            )
            .child(ConstrainedBox::new(
                Constraints::tight(Size::new(PANEL_INNER, height)),
                // A shared controller: the panel is rebuilt whenever player
                // state changes, and a fresh controller would jump to the top.
                motion::wheel(
                    &app.controllers.player_menu,
                    WheelAxis::Vertical,
                    SingleChildScrollView::new(col(panel.children))
                        .physics(motion::APP_SCROLLED)
                        .controller(app.controllers.player_menu.clone()),
                ),
            ))
            .into(),
    )
}

/// One caption line drawn with the chosen character edge, as hard text
/// shadows.
fn edged_text(value: &str, size: f32, style: &SubtitleStyle) -> Widget {
    let weight = if style.bold { FontWeight::W700 } else { FontWeight::W500 };
    let alpha = style.text_opacity;
    let text_color = rgb(style.text_color, alpha);
    let glyphs = |color: Color| Text::new(value.to_owned()).style(self::style(size, weight, color));
    let dark = |strength: f32| Color::rgba(0, 0, 0, (strength * alpha * 255.0) as u8);
    let light = |strength: f32| Color::rgba(255, 255, 255, (strength * alpha * 255.0) as u8);
    let d = (size * 0.06).clamp(1.0, 2.5);
    let copies: Vec<(f32, f32, Color)> = match style.edge {
        SubtitleEdge::None => Vec::new(),
        SubtitleEdge::DropShadow => vec![(d, d, dark(0.75)), (d * 1.6, d * 1.6, dark(0.35))],
        SubtitleEdge::Outline => [
            (-1.0, -1.0),
            (0.0, -1.0),
            (1.0, -1.0),
            (-1.0, 0.0),
            (1.0, 0.0),
            (-1.0, 1.0),
            (0.0, 1.0),
            (1.0, 1.0),
        ]
        .into_iter()
        .map(|(x, y)| (x * d, y * d, dark(1.0)))
        .collect(),
        SubtitleEdge::Raised => vec![(d * 0.6, d * 0.6, dark(0.9)), (d * 1.2, d * 1.2, dark(0.5))],
        SubtitleEdge::Depressed => vec![(-d * 0.6, -d * 0.6, dark(0.9)), (d * 0.6, d * 0.6, light(0.45))],
    };
    if copies.is_empty() {
        return glyphs(text_color).into();
    }
    let shadows = copies.into_iter().map(|(x, y, color)| TextShadow {
        color,
        offset: Offset::new(x, y),
        blur_radius: 0.0,
    });
    let text = Text::new(value.to_owned())
        .style(self::style(size, weight, text_color).shadows(shadows));
    // Room for the shadows, so they are not clipped by neighbours.
    Padding::new(EdgeInsets::all(d * 2.0), text).into()
}

/// A subtitle cue: optional window behind the block, a background behind
/// each line, edged text on top.
fn caption_block(lines: &[&str], size: f32, style: &SubtitleStyle) -> Widget {
    let background = rgb(style.background_color, style.background_opacity);
    let rows: Vec<Widget> = lines
        .iter()
        .map(|line| {
            Container::new()
                .padding(EdgeInsets::symmetric(size * 0.3, size * 0.06))
                .decoration(
                    BoxDecoration::new()
                        .color(background)
                        .border_radius(BorderRadius::all(Radius::circular(4.0))),
                )
                .child(edged_text(line, size, style))
                .into()
        })
        .collect();
    let block = Column::new(rows)
        .main_axis_size(MainAxisSize::Min)
        .cross_axis_alignment(CrossAxisAlignment::Center);
    if style.window_opacity <= 0.0 {
        return block.into();
    }
    Container::new()
        .padding(EdgeInsets::symmetric(size * 0.5, size * 0.3))
        .decoration(
            BoxDecoration::new()
                .color(rgb(style.window_color, style.window_opacity))
                .border_radius(BorderRadius::all(Radius::circular(8.0))),
        )
        .child(block)
        .into()
}

/// Every subtitle line with its time. Clicking the line being spoken right
/// now re-times the whole track, instead of guessing a delay.
fn sync_panel(app: &App) -> Widget {
    const WIDTH: f32 = 470.0;
    let cues = Rc::new(app.player.cue_list());
    let (delay, paused) = app.player.state.peek(|s| (s.sub_delay, s.paused));
    let position = app.player.position.get_untracked();
    let current = cues
        .iter()
        .position(|cue| cue.start <= position - delay && position - delay < cue.end);

    let (back, reset, pause) = (app.clone(), app.clone(), app.clone());
    let header = hrow(vec![
        text("Sync subtitles", 14.0, FontWeight::W600, TEXT).into(),
        hgap(10.0),
        text(format!("{delay:+.2}s"), 12.5, FontWeight::W600, ACCENT).into(),
        Spacer::new().into(),
        pill(if paused { "Play" } else { "Pause" }, false, move || pause.toggle_pause()),
        hgap(6.0),
        pill("Reset", false, move || reset.reset_subtitle_delay()),
        hgap(6.0),
        pill("Done", true, move || back.set_player_menu(Menu::Sync)),
    ])
    .cross_axis_alignment(CrossAxisAlignment::Center);

    let progress = subtitle_progress(app);
    let embedded = app.player.state.peek(|s| matches!(s.sub_choice, SubChoice::Embedded(_)));
    let status: Widget = match &progress {
        Some((label, done)) => {
            let mut parts: Vec<Widget> = vec![
                if *done {
                    icon(icons::check(), 13.0, FAINT)
                } else {
                    spinner(13.0)
                },
                hgap(7.0),
                Expanded::new(line(label.clone(), 12.0, FontWeight::W500, if *done { FAINT } else { ACCENT }))
                    .into(),
            ];
            if !done && embedded {
                let all = app.clone();
                parts.push(hgap(6.0));
                parts.push(pill("Load all now", false, move || all.load_all_subtitles()));
            }
            hrow(parts).cross_axis_alignment(CrossAxisAlignment::Center).into()
        }
        None => SizedBox::shrink().into(),
    };

    let list: Widget = if cues.is_empty() {
        Padding::new(
            EdgeInsets::only(4.0, 20.0, 4.0, 0.0),
            muted("No lines yet. They appear here as the subtitle track is read."),
        )
        .into()
    } else {
        let controller = app.controllers.sync_list.clone();
        let wheel_controller = controller.clone();
        let app = app.clone();
        let count = cues.len();
        let list = ListView::builder(count, move |index| {
            let cue = &cues[index];
            let selected = current == Some(index);
            let start = cue.start;
            let app = app.clone();
            let background = if selected { ACCENT_DIM } else { Color::TRANSPARENT };
            Button::with_child(hrow(vec![
                SizedBox::new()
                    .width(62.0)
                    .child(text(
                        format_time(cue.start),
                        12.0,
                        FontWeight::W600,
                        if selected { ACCENT } else { FAINT },
                    ))
                    .into(),
                Expanded::new(line(cue.text.replace('\n', "  "), 13.0, FontWeight::W500, TEXT))
                    .into(),
            ]))
            .style(ButtonStyle {
                background: Some(background),
                background_states: Some(
                    StateColor::new(background).hovered(Color::rgba(255, 255, 255, 30)),
                ),
                foreground: Some(TEXT),
                border: Some(Border::new(0.0, Color::TRANSPARENT)),
                border_radius: Some(8.0),
                padding: Some(EdgeInsets::symmetric(10.0, 0.0)),
                fixed_size: Some(Size::new(WIDTH - 24.0, App::SYNC_ROW - 4.0)),
                alignment: Some(Alignment::CENTER_LEFT),
                overlay_color: Some(Color::TRANSPARENT.into()),
                animation_duration: Some(std::time::Duration::ZERO),
                ..ButtonStyle::default()
            })
            .on_click(move || app.sync_subtitle_to(start))
        })
        .item_extent(App::SYNC_ROW)
        .physics(motion::APP_SCROLLED)
        .controller(controller);
        motion::wheel(&wheel_controller, WheelAxis::Vertical, list)
    };

    Container::new()
        .width(WIDTH)
        .height(430.0)
        .padding(EdgeInsets::all(12.0))
        .decoration(
            BoxDecoration::new()
                .color(Color::rgba(24, 24, 24, 245))
                .border_radius(BorderRadius::all(Radius::circular(16.0))),
        )
        .child(
            vcol(vec![
                header.into(),
                gap(6.0),
                text(
                    "Pause right as someone starts a line, then click that line below.",
                    12.0,
                    FontWeight::W400,
                    MUTED,
                )
                .into(),
                gap(8.0),
                SizedBox::new().height(30.0).child(status).into(),
                gap(6.0),
                Expanded::new(list).into(),
            ])
            .cross_axis_alignment(CrossAxisAlignment::Stretch),
        )
        .into()
}

fn top_bar(app: &App, title: &str, subtitle: &str) -> Widget {
    let back = app.clone();
    let mut titles: Vec<Widget> = vec![line(title, 16.0, FontWeight::W600, TEXT)];
    if !subtitle.is_empty() {
        titles.push(gap(2.0));
        titles.push(line(subtitle, 12.5, FontWeight::W400, MUTED));
    }
    Container::new()
        .padding(EdgeInsets::symmetric(18.0, 12.0))
        .child(
            hrow(vec![
                player_tip_at("Back", floating_button(icon(icons::arrow_left(), CONTROL_ICON, TEXT), PRIMARY_BUTTON, move || back.back()), true),
                hgap(CONTROL_GAP),
                Flexible::new(control_surface(col(titles), EdgeInsets::symmetric(16.0, 10.0))).into(),
            ])
            .cross_axis_alignment(CrossAxisAlignment::Center),
        )
        .into()
}

/// A control-bar icon that opens a panel. Tinted while its panel is open;
/// a dot underneath marks a non-default setting (subtitles on, 1.5x, ...).
/// Gap under the popup panels: they sit just above the bottom bar.
const PANEL_BOTTOM: f32 = 104.0;

/// An open popup, bottom right, over a full-screen barrier: a click
/// anywhere outside the popup closes it (and does nothing else).
fn popup_layer(app: &App, panel: Widget) -> Widget {
    let close = app.clone();
    let barrier = GestureDetector::new(SizedBox::expand())
        .behavior(HitTestBehavior::Opaque)
        .on_tap(move || close.set_player_menu(Menu::None));
    Stack::new(vec![
        Widget::from(barrier),
        Align::new(
            Alignment::BOTTOM_RIGHT,
            Padding::new(EdgeInsets::only(0.0, 0.0, 20.0, PANEL_BOTTOM), panel),
        )
        .into(),
    ])
    .fit(StackFit::Expand)
    .into()
}

fn peers_dialog(app: &App) -> Widget {
    let app = app.clone();
    LayoutBuilder::new(move |_, constraints| {
        let stats = app.player.state.read(|s| s.torrent.clone()).unwrap_or_default();
        let peers = app.player.current_media().and_then(|(stream, _, _)| {
            match stream.source() {
                crate::addons::types::StreamSource::Torrent { info_hash, .. } => Some(crate::streaming::peers(&info_hash)),
                _ => None,
            }
        }).unwrap_or_default();
        let close = app.clone();
        let header = Row::new(vec![
            text("Peers", 20.0, FontWeight::W700, TEXT).into(),
            Spacer::new().into(),
            player_tip("Close", icon_button(icon(icons::x(), 18.0, TEXT), 30.0,
                move || close.set_player_menu(Menu::None))),
        ]).cross_axis_alignment(CrossAxisAlignment::Center);
        let summary = Row::new(vec![
            stat("Connected", stats.peers.to_string()),
            stat("Seeders", stats.seeders.to_string()),
            stat("Connecting", stats.connecting.to_string()),
            stat("Discovered", stats.discovered.to_string()),
        ]).main_axis_alignment(MainAxisAlignment::SpaceBetween);
        let list_height = (peers.len().max(1) as f32 * 90.0)
            .min((constraints.max_height() - 278.0).clamp(80.0, 340.0));
        let mut items: Vec<Widget> = Vec::new();
        for peer in peers {
            items.push(Container::new().padding(EdgeInsets::all(12.0)).radius(8.0)
                .color(Color::rgba(255, 255, 255, 9)).child(col(vec![
                    Row::new(vec![
                        Expanded::new(line(peer.address, 13.0, FontWeight::W600, TEXT)).into(),
                        hgap(8.0), text(peer.transport, 11.5, FontWeight::W600, MUTED).into(),
                    ]).cross_axis_alignment(CrossAxisAlignment::Center).into(),
                    gap(4.0),
                    line(peer.client, 12.0, FontWeight::W400, MUTED),
                    gap(6.0),
                    text(format!("Downloaded {}  ·  Uploaded {}", format_bytes(peer.downloaded), format_bytes(peer.uploaded)),
                        11.5, FontWeight::W400, FAINT).into(),
                ])).into());
            items.push(gap(8.0));
        }
        if items.is_empty() {
            items.push(Padding::new(EdgeInsets::symmetric(0.0, 22.0),
                text("No connected peers yet. Discovered peers will appear here when they connect.",
                    13.0, FontWeight::W400, MUTED)).into());
        }
        let dialog = Container::new().width((constraints.max_width() - 40.0).clamp(0.0, 560.0))
            .padding(EdgeInsets::all(22.0)).radius(16.0).color(Color::rgba(24, 24, 24, 250))
            .child(col(vec![
                header.into(), gap(18.0), summary.into(), gap(16.0),
                text(format!("TCP {}  ·  uTP {}  ·  SOCKS {}", stats.tcp_peers, stats.utp_peers, stats.proxy_peers),
                    12.0, FontWeight::W500, MUTED).into(), gap(14.0),
                text(format!("{} download  ·  {:.1}% of video buffered", format_speed(stats.download_speed), stats.progress * 100.0),
                    12.0, FontWeight::W500, MUTED).into(), gap(16.0),
                SizedBox::new().height(list_height).child(motion::wheel(&app.controllers.player_menu,
                    WheelAxis::Vertical, SingleChildScrollView::new(col(items).cross_axis_alignment(CrossAxisAlignment::Stretch))
                        .physics(motion::APP_SCROLLED).controller(app.controllers.player_menu.clone()))).into(),
            ]).cross_axis_alignment(CrossAxisAlignment::Stretch));
        let dismiss = app.clone();
        Stack::new(vec![
            GestureDetector::new(Container::new().color(Color::rgba(0, 0, 0, 100)).child(SizedBox::expand()))
                .behavior(HitTestBehavior::Opaque).on_tap(move || dismiss.set_player_menu(Menu::None)).into(),
            Center::new(dialog).into(),
        ] as Vec<Widget>).fit(StackFit::Expand).into()
    }).into()
}

fn menu_button(glyph: Glyph, open: bool, changed: bool, on_click: impl Fn() + 'static) -> Widget {
    let color = if open || changed { ACCENT } else { TEXT };
    let mark: Widget = Container::new()
        .width(4.0)
        .height(4.0)
        .radius(2.0)
        .color(if changed { ACCENT } else { Color::TRANSPARENT })
        .into();
    let content = Column::new(vec![icon(glyph, CONTROL_ICON, color), gap(2.0), mark])
        .main_axis_size(MainAxisSize::Min)
        .cross_axis_alignment(CrossAxisAlignment::Center);
    let background = if open { Color::rgba(255, 255, 255, 26) } else { Color::TRANSPARENT };
    Button::with_child(Padding::new(EdgeInsets::only(0.0, 6.0, 0.0, 0.0), content))
        .style(ButtonStyle {
            background: Some(background),
            background_states: Some(
                StateColor::new(background).hovered(Color::rgba(255, 255, 255, 28)),
            ),
            foreground: Some(TEXT),
            border: Some(Border::new(0.0, Color::TRANSPARENT)),
            border_radius: Some(CONTROL_BUTTON / 2.0),
            padding: Some(EdgeInsets::ZERO),
            fixed_size: Some(Size::new(CONTROL_BUTTON, CONTROL_BUTTON)),
            overlay_color: Some(Color::TRANSPARENT.into()),
            animation_duration: Some(std::time::Duration::ZERO),
            ..ButtonStyle::default()
        })
        .on_click(on_click)
        .into()
}

fn volume_control(app: &App) -> Widget {
    let app = app.clone();
    Builder::new(move |_| {
        let dragging = VOLUME_DRAGGING.with(|value| value.get());
        let open = VOLUME_HOVER.with(|value| value.get()) || dragging;
        let (volume, muted) = app.player.state.read(|s| (s.volume, s.muted));
        let shown = if muted { 0.0 } else { volume };
        let mute = app.clone();
        let label = if muted || volume <= 0.0 { "Unmute" } else { "Mute" };
        let button = player_tip(label, icon_button(icon(if muted || volume <= 0.0 { icons::volume_x() } else { icons::volume_2() },
            CONTROL_ICON, TEXT).accessibility_label(label), CONTROL_BUTTON,
            move || { mute.toggle_mute(); mute.player_activity(); }));
        let change = app.clone();
        let link = VOLUME_LINK.with(Clone::clone);
        let rail = track(84.0, shown / MAX_VOLUME, &[], Rc::new(move |value, committed| {
            VOLUME_DRAGGING.with(|dragging| dragging.set(!committed));
            change.set_volume(value * MAX_VOLUME);
        }), Rail::THIN, None, VOLUME_POSITION.with(Clone::clone));
        let rail = MouseRegion::new(CompositedTransformTarget::new(link.clone(), rail))
            .on_hover(|event| { VOLUME_POSITION.with(|value| value.set(Some((event.local_position.x / 84.0).clamp(0.0, 1.0)))); })
            .on_exit(|_| { VOLUME_POSITION.with(|value| value.set(None)); });
        let extra: Widget = row(vec![hgap(6.0), rail.into(), hgap(10.0)]).into();
        let hover = app.clone();
        let surface = MouseRegion::new(TweenAnimationBuilder::new(if open { 1.0f32 } else { 0.0 },
            std::time::Duration::from_millis(if incular::animation::disable_animations() { 0 } else { 180 }),
            move |_, amount, child| control_surface(row(vec![button.clone(),
                if amount >= 1.0 { child.expect("volume slider") } else {
                    ClipRect::new(Align::new(Alignment::CENTER_LEFT, child.expect("volume slider"))
                        .width_factor(amount)).into()
                },
            ]), EdgeInsets::all(2.0)))
            .curve(Curve::EaseOutCubic).child(extra))
            .on_enter(move |_| { VOLUME_HOVER.with(|value| value.set(true)); hover.player_activity(); })
            .on_exit(move |_| {
                VOLUME_HOVER.with(|value| value.set(false));
                VOLUME_POSITION.with(|value| value.set(None));
            });
        let at = VOLUME_POSITION.with(|value| value.get());
        let (left, pointer) = bubble_geometry(84.0, at.unwrap_or(shown / MAX_VOLUME));
        let popup = SizedBox::new().width(84.0).height(BUBBLE_HEIGHT + POINTER_HEIGHT)
            .child(layers(vec![
                Positioned::new(Container::new().width(BUBBLE_WIDTH).height(BUBBLE_HEIGHT)
                    .alignment(Alignment::CENTER).radius(4.0).color(Color::WHITE)
                    .child(text(format!("{:.0}%", shown * 100.0), 12.5, FontWeight::W600, ON_SELECTED)))
                    .left(left).top(0.0).into(),
                Positioned::new(CustomPaint::from_painter(Size::new(POINTER_WIDTH, POINTER_HEIGHT), Pointer(false)))
                    .left(pointer).top(BUBBLE_HEIGHT).into(),
            ]).clip_behavior(incular::config::Clip::None));
        OverlayPortal::new(surface).presentation(incular::config::TransientPresentation::Overlay)
            .role(incular::config::TransientRole::Tooltip).root_overlay(true)
            .overlay_child(IgnorePointer::new(CompositedTransformFollower::new(link, popup)
                .target_anchor(Alignment::TOP_LEFT).follower_anchor(Alignment::BOTTOM_LEFT)
                .offset(Offset::new(0.0, 10.0 - BUBBLE_GAP)).show_when_unlinked(false)))
            .show(open && (at.is_some() || dragging)).into()
    }).into()
}

fn bottom_bar(app: &App) -> Widget {
    let (live, paused, ended, speed, menu, has_next, torrent, has_subs, brightness) =
        app.player.state.peek(|s| {
            (
                s.live,
                s.paused,
                s.status == Status::Ended,
                s.speed,
                s.menu,
                s.next.is_some(),
                s.torrent.clone(),
                s.sub_choice != SubChoice::Off,
                s.brightness,
            )
        });
    let action = |call: fn(&App)| {
        let app = app.clone();
        move || call(&app)
    };
    let mut controls: Vec<Widget> = vec![
        player_tip(if paused || ended { "Play" } else { "Pause" }, floating_button(
            icon(
                if paused || ended { icons::play() } else { icons::pause() },
                PRIMARY_ICON,
                TEXT,
            ),
            PRIMARY_BUTTON,
            action(|app| app.toggle_pause()),
        )),
    ];
    if !live {
        controls.push(hgap(CONTROL_GAP));
        controls.push(player_tip("Rewind 10 seconds", floating_button(
            icon(icons::rewind(), CONTROL_ICON, TEXT),
            CONTROL_BUTTON,
            action(|app| app.seek_by(-10.0)),
        )));
        controls.push(hgap(CONTROL_GAP));
        controls.push(player_tip("Forward 10 seconds", floating_button(
            icon(icons::fast_forward(), CONTROL_ICON, TEXT),
            CONTROL_BUTTON,
            action(|app| app.seek_by(10.0)),
        )));
    }
    if has_next {
        controls.push(hgap(CONTROL_GAP));
        controls.push(player_tip("Next episode", floating_button(
            icon(icons::skip_forward(), CONTROL_ICON, TEXT),
            CONTROL_BUTTON,
            action(|app| {
                if let Some(next) = app.player.state.peek(|s| s.next.clone()) {
                    app.play_next(next);
                }
            }),
        )));
    }
    controls.push(hgap(CONTROL_GAP));
    controls.push(volume_control(app));
    controls.push(hgap(CONTROL_GAP));
    controls.push(control_surface(
        SizedBox::new().height(PRIMARY_BUTTON).child(Center::new(clock(app))),
        EdgeInsets::symmetric(14.0, 0.0),
    ));
    controls.push(Spacer::new().into());
    let (room, people) = app.together.state.peek(|s| {
        (
            (s.link == crate::together::Link::Connected).then(|| s.room.clone()),
            s.members.len(),
        )
    });
    if let Some(room) = room {
        controls.push(
            control_surface(SizedBox::new().height(PRIMARY_BUTTON).child(Center::new(row(vec![
                icon(icons::users(), 15.0, ACCENT),
                hgap(6.0),
                text(format!("{room} · {people}"), 12.0, FontWeight::W600, ACCENT).into(),
            ]))), EdgeInsets::symmetric(12.0, 0.0)),
        );
        controls.push(hgap(14.0));
    }
    if let Some(stats) = torrent {
        let peers = app.clone();
        controls.push(
            player_tip("Peers and torrent details", GestureDetector::new(control_surface(SizedBox::new().height(PRIMARY_BUTTON).child(Center::new(text(
                format!("{} peers · {}", stats.peers, format_speed(stats.download_speed)),
                12.0,
                FontWeight::W500,
                MUTED,
            ))), EdgeInsets::symmetric(12.0, 0.0)))
                .behavior(HitTestBehavior::Opaque)
                .on_tap(move || peers.set_player_menu(Menu::Peers)).into()),
        );
        controls.push(hgap(14.0));
    }
    let mut menu_controls = Vec::new();
    let (in_room, unread) = app.together.state.peek(|s| (s.link != crate::together::Link::Off, s.unread));
    if in_room {
        let app = app.clone();
        menu_controls.push(player_tip("Watch together", menu_button(icons::users(), menu == Menu::Together, unread > 0, move || {
            app.set_player_menu(Menu::Together)
        })));
        menu_controls.push(hgap(CONTROL_GAP));
    }
    let style_open = menu == Menu::SubtitleStyle || menu == Menu::Sync;
    for (label, glyph, target, changed) in [
        ("Picture", icons::sun(), Menu::Picture, (brightness - 1.0).abs() > 0.01),
        ("Playback speed", icons::gauge(), Menu::Speed, (speed - 1.0).abs() > 0.01),
        ("Audio", icons::audio_lines(), Menu::Audio, false),
        ("Subtitles", icons::captions(), Menu::Subtitles, has_subs),
    ] {
        if live && target == Menu::Speed {
            continue;
        }
        let open = menu == target || (target == Menu::Subtitles && style_open);
        let app = app.clone();
        menu_controls.push(player_tip(label, menu_button(glyph, open, changed, move || app.set_player_menu(target))));
        menu_controls.push(hgap(CONTROL_GAP));
    }
    let fullscreen = app.player.fullscreen.get();
    let more = app.clone();
    menu_controls.push(player_tip("More video actions", icon_button(icon(icons::more_horizontal(), CONTROL_ICON,
        if menu == Menu::More { ACCENT } else { TEXT }).accessibility_label("More video actions"),
        CONTROL_BUTTON, move || more.set_player_menu(Menu::More))));
    menu_controls.push(hgap(CONTROL_GAP));
    let fullscreen_label = if fullscreen { "Exit fullscreen" } else { "Enter fullscreen" };
    menu_controls.push(player_tip(fullscreen_label, icon_button(
        icon(
            if fullscreen { icons::minimize() } else { icons::maximize() },
            CONTROL_ICON,
            TEXT,
        ).accessibility_label(fullscreen_label),
        CONTROL_BUTTON,
        action(|app| app.toggle_fullscreen()),
    ).accessibility_label(fullscreen_label)));
    controls.push(control_surface(row(menu_controls), EdgeInsets::symmetric(4.0, 2.0)));

    Container::new()
        .padding(EdgeInsets::only(20.0, 8.0, 20.0, 12.0))
        .child(
            vcol(vec![
                if live { gap(6.0) } else { seek_bar(app) },
                gap(6.0),
                Row::new(controls)
                    .cross_axis_alignment(CrossAxisAlignment::Center)
                    .into(),
            ])
            .main_axis_size(MainAxisSize::Min)
            .cross_axis_alignment(CrossAxisAlignment::Stretch),
        )
        .into()
}

/// "Up next" countdown in the bottom-right corner, above the control bar
/// when it is showing.
fn up_next_card(app: &App, next: &crate::addons::types::Video, left: u32, lifted: bool) -> Widget {
    let (play, cancel, video) = (app.clone(), app.clone(), next.clone());
    let card = Container::new()
        .width(340.0)
        .padding(EdgeInsets::all(16.0))
        .decoration(
            BoxDecoration::new()
                .color(Color::rgba(24, 24, 24, 240))
                .border_radius(BorderRadius::all(Radius::circular(14.0))),
        )
        .child(col(vec![
            row(vec![
                icon(icons::skip_forward(), 14.0, ACCENT),
                hgap(6.0),
                text(format!("UP NEXT IN {left}s"), 11.0, FontWeight::W700, ACCENT).into(),
            ])
            .into(),
            gap(6.0),
            SizedBox::new()
                .width(308.0)
                .child(line(next.label(), 14.5, FontWeight::W600, TEXT))
                .into(),
            gap(12.0),
            row(vec![
                icon_text_button(icons::play(), "Play now", Tone::Primary, move || {
                    play.play_next(video.clone())
                }),
                hgap(8.0),
                button("Cancel", Tone::Glass, move || cancel.cancel_up_next()),
            ])
            .into(),
        ]));
    Align::new(
        Alignment::BOTTOM_RIGHT,
        Padding::new(
            EdgeInsets::only(0.0, 0.0, 24.0, if lifted { 120.0 } else { 28.0 }),
            card,
        ),
    )
    .into()
}

fn stat(label: &str, value: String) -> Widget {
    Column::new(vec![
        text(value, 15.0, FontWeight::W700, TEXT).into(),
        gap(2.0),
        text(label, 11.0, FontWeight::W500, FAINT).into(),
    ] as Vec<Widget>)
    .main_axis_size(MainAxisSize::Min)
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .into()
}

fn status_card(children: Vec<Widget>) -> Widget {
    Center::new(
        Container::new()
            .padding(EdgeInsets::symmetric(26.0, 20.0))
            .decoration(
                BoxDecoration::new()
                    .color(OVERLAY)
                    .border_radius(BorderRadius::all(Radius::circular(16.0))),
            )
            .child(
                Column::new(children)
                    .main_axis_size(MainAxisSize::Min)
                    .cross_axis_alignment(CrossAxisAlignment::Center),
            ),
    )
    .into()
}

/// A spinning ring.
/// Seconds of video downloaded without a gap from the playhead on.
fn buffered_ahead(ranges: &[(f32, f32)], position: f64, duration: f64) -> Option<f64> {
    if duration <= 0.0 {
        return None;
    }
    let at = (position / duration) as f32;
    let range = ranges.iter().find(|(start, end)| *start <= at + 0.0005 && at < *end);
    Some(range.map_or(0.0, |(_, end)| ((*end - at) as f64 * duration).max(0.0)))
}

fn connection_details(stats: &crate::streaming::TorrentStats) -> Widget {
    let mut transports = format!("TCP {}  ·  uTP {}", stats.tcp_peers, stats.utp_peers);
    if stats.proxy_peers > 0 {
        transports.push_str(&format!("  ·  SOCKS {}", stats.proxy_peers));
    }
    col(vec![
        text(transports, 12.0, FontWeight::W600, TEXT).into(),
        gap(5.0),
        text(format!("{} discovered  ·  {} connecting  ·  {} seeders",
            stats.discovered, stats.connecting, stats.seeders),
            11.5, FontWeight::W400, MUTED).into(),
    ]).cross_axis_alignment(CrossAxisAlignment::Center).into()
}

/// Shown while playback waits for data: what it is waiting on and how
/// the download is going.
fn buffering_card(app: &App, torrent: Option<&crate::streaming::TorrentStats>) -> Widget {
    let position = app.player.position.get_untracked();
    let duration = app.player.state.peek(|s| s.duration);
    let mut lines: Vec<Widget> = vec![
        spinner(34.0),
        gap(14.0),
        text("Buffering", 16.0, FontWeight::W700, TEXT).into(),
        gap(4.0),
    ];
    match torrent {
        Some(stats) => {
            let hint = if stats.peers == 0 {
                "Looking for peers…"
            } else if stats.download_speed < 50_000.0 {
                "Peers are slow to send this part"
            } else {
                "Fetching the next part of the video"
            };
            lines.push(text(hint, 12.5, FontWeight::W400, MUTED).into());
            lines.push(gap(16.0));
            let ahead = buffered_ahead(&stats.ranges, position, duration)
                .map_or_else(|| "–".to_owned(), |s| format!("{s:.0}s"));
            lines.push(
                row(vec![
                    stat("Connected peers", stats.peers.to_string()),
                    hgap(26.0),
                    stat("Download", format_speed(stats.download_speed)),
                    hgap(26.0),
                    stat("Ready ahead", ahead),
                    hgap(26.0),
                    stat("Downloaded", format!("{:.1}%", stats.progress * 100.0)),
                ])
                .into(),
            );
            lines.push(gap(12.0));
            lines.push(connection_details(stats));
            if duration > 0.0 {
                lines.push(gap(16.0));
                lines.push(
                    IgnorePointer::new(track(
                        300.0,
                        (position / duration) as f32,
                        &stats.ranges,
                        Rc::new(|_, _| {}),
                        Rail::THIN,
                        None,
                        SEEK_HOVER.with(Clone::clone),
                    ))
                    .into(),
                );
                lines.push(
                    text(
                        format!("At {} of {}", format_time(position), format_time(duration)),
                        11.0,
                        FontWeight::W500,
                        FAINT,
                    )
                    .into(),
                );
            }
        }
        None => {
            lines.push(text("Waiting for the stream to send more data", 12.5, FontWeight::W400, MUTED).into());
            if duration > 0.0 {
                lines.push(gap(10.0));
                lines.push(
                    text(
                        format!("At {} of {}", format_time(position), format_time(duration)),
                        11.0,
                        FontWeight::W500,
                        FAINT,
                    )
                    .into(),
                );
            }
        }
    }
    status_card(lines)
}

// YouTube's control chrome enters over 250ms and leaves over 100ms.
const CHROME_FADE_IN: Duration = Duration::from_millis(250);
const CHROME_FADE_OUT: Duration = Duration::from_millis(100);

thread_local! {
    /// The controls' fade, kept across rebuilds of the player screen.
    static CHROME: motion::Entrance = motion::Entrance::new();
    /// Whether the controls were showing at the last build.
    static CHROME_SHOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

pub fn screen(app: &App) -> Widget {
    let (stage, status, title, subtitle, visible, menu, stalled, has_video, torrent, next, brightness) =
        app.player.state.read(|s| {
            (
                s.stage.clone(),
                s.status.clone(),
                s.title.clone(),
                s.subtitle.clone(),
                s.controls_visible,
                s.menu,
                s.stalled,
                s.has_video,
                s.torrent.clone(),
                s.next.clone(),
                s.brightness,
            )
        });

    let picture: Widget = {
        let frame = app.player.frame.clone();
        let image = LayoutBuilder::new(move |_, constraints| {
            let size = constraints.biggest();
            match frame.get() {
                Some(handle) => Image::new(handle)
                    .width(size.width)
                    .height(size.height)
                    .fit(BoxFit::Contain)
                    .into(),
                None => SizedBox::shrink().into(),
            }
        });
        // Filtering costs an offscreen pass, so only when it changes anything.
        if (brightness - 1.0).abs() > 0.01 {
            ColorFiltered::new(ColorFilter::brightness(brightness), image).into()
        } else {
            image.into()
        }
    };

    let captions = {
        let app = app.clone();
        Builder::new(move |_| {
            let cue = app.player.cue.get();
            if cue.is_empty() {
                return SizedBox::shrink().into();
            }
            let (size, lifted, sub_style) = app
                .player
                .state
                .read(|s| (s.sub_size, s.controls_visible, s.sub_style.clone()));
            // One widget per line, centred by the column, so each line gets
            // its own background box.
            let lines: Vec<&str> = cue.lines().collect();
            // Captions move up with the controls, and back down when they hide.
            Align::new(
                Alignment::BOTTOM_CENTER,
                AnimatedPadding::new(
                    EdgeInsets::only(48.0, 0.0, 48.0, if lifted { 128.0 } else { 44.0 }),
                    if lifted { CHROME_FADE_IN } else { CHROME_FADE_OUT },
                    IgnorePointer::new(caption_block(&lines, size, &sub_style)),
                )
                .curve(Curve::EaseOutCubic),
            )
            .into()
        })
    };

    // The click-to-pause surface is a sibling layer underneath the controls.
    let (tap, double) = (app.clone(), app.clone());
    let surface = GestureDetector::new(Container::new().color(Color::TRANSPARENT))
        .behavior(HitTestBehavior::Opaque)
        .on_tap(move || {
            if tap.player.state.peek(|s| s.menu != Menu::None) {
                tap.set_player_menu(Menu::None);
            } else {
                tap.toggle_pause();
            }
        })
        .on_double_tap(move || double.toggle_fullscreen());

    let mut layers: Vec<Widget> = vec![
        Container::new().color(Color::BLACK).into(),
        picture,
        captions.into(),
        surface.into(),
    ];

    match &status {
        Status::Opening => {
            let mut lines: Vec<Widget> = vec![
                text("OPENING", 11.0, FontWeight::W700, ACCENT).into(),
                gap(8.0),
                SizedBox::new()
                    .width(380.0)
                    .child(
                        Column::new(vec![line(title.clone(), 19.0, FontWeight::W700, TEXT)])
                            .main_axis_size(MainAxisSize::Min)
                            .cross_axis_alignment(CrossAxisAlignment::Center),
                    )
                    .into(),
            ];
            if !subtitle.is_empty() {
                lines.push(gap(3.0));
                lines.push(text(subtitle.clone(), 13.0, FontWeight::W400, MUTED).into());
            }
            lines.push(gap(18.0));
            lines.push(
                incular::controls::Progress::new()
                    .indeterminate(true)
                    .width(380.0)
                    .height(4.0)
                    .into(),
            );
            lines.push(gap(14.0));
            lines.push(text(stage.clone(), 13.0, FontWeight::W500, TEXT).into());
            if let Some(stats) = &torrent {
                lines.push(gap(12.0));
                lines.push(
                    row(vec![
                        stat("Connected peers", stats.peers.to_string()),
                        hgap(26.0),
                        stat("Download", format_speed(stats.download_speed)),
                    ])
                    .into(),
                );
                lines.push(gap(12.0));
                lines.push(connection_details(stats));
            } else if stage == "Joining the swarm…" {
                lines.push(gap(10.0));
                lines.push(text("Discovering peers · TCP + uTP enabled", 12.0,
                    FontWeight::W400, MUTED).into());
            }
            layers.push(status_card(lines));
        }
        Status::Error(message) => {
            let back = app.clone();
            layers.push(status_card(vec![
                text("Playback failed", 17.0, FontWeight::W600, TEXT).into(),
                gap(8.0),
                SizedBox::new()
                    .width(460.0)
                    .child(
                        Text::new(message.clone()).style(style(13.5, FontWeight::W400, MUTED)),
                    )
                    .into(),
                gap(16.0),
                button("Back to streams", Tone::Primary, move || back.back()),
            ]));
        }
        Status::Ended => {
            let (replay, forward) = (app.clone(), app.clone());
            let mut actions = vec![button("Replay", Tone::Glass, move || replay.seek_to(0.0))];
            if let Some(next) = next.clone() {
                actions.push(hgap(10.0));
                actions.push(button("Next episode", Tone::Primary, move || {
                    forward.play_next(next.clone())
                }));
            }
            layers.push(status_card(vec![
                text("Finished", 17.0, FontWeight::W600, TEXT).into(),
                gap(14.0),
                row(actions).into(),
            ]));
        }
        Status::Playing if stalled => {
            layers.push(buffering_card(app, torrent.as_ref()));
        }
        Status::Playing if !stalled && !app.together_waiting_for().is_empty() => {
            let names = app.together_waiting_for().join(", ");
            layers.push(status_card(vec![
                spinner(30.0),
                gap(12.0),
                text("Waiting for the room", 15.0, FontWeight::W700, TEXT).into(),
                gap(4.0),
                text(format!("{names} is buffering. Playback resumes together."), 12.5, FontWeight::W400, MUTED)
                    .into(),
            ]));
        }
        Status::Playing if !has_video => {
            layers.push(status_card(vec![
                text("Audio only", 15.0, FontWeight::W600, MUTED).into(),
            ]));
        }
        _ => {}
    }

    if menu != Menu::Together {
        if let Some(bubbles) = super::together::bubbles(app) {
            layers.push(bubbles);
        }
    }

    if let (Some(left), Some(next_video)) = (app.player.state.read(|s| s.up_next), next.clone()) {
        layers.push(up_next_card(app, &next_video, left, visible));
    }

    // The controls fade in and out. While fading out they stay in the tree
    // but ignore the pointer; a rebuild after the fade removes them.
    let fade = CHROME.with(|c| c.clone());
    if CHROME_SHOWN.with(|shown| shown.replace(visible)) != visible {
        fade.fade_to(
            if visible { 1.0 } else { 0.0 },
            if visible { CHROME_FADE_IN } else { CHROME_FADE_OUT },
        );
        if !visible {
            crate::state::spawn(
                async { tokio::time::sleep(CHROME_FADE_OUT + Duration::from_millis(40)).await },
                |app, ()| app.player.state.write(|_| ()),
            );
        }
    }
    if visible || fade.opacity() > 0.0 {
        let mut chrome: Vec<Widget> = vec![
            top_bar(app, &title, &subtitle),
            Spacer::new().into(),
        ];
        chrome.push(bottom_bar(app));
        layers.push(fade.wrap(
            IgnorePointer::new(
                Column::new(chrome).cross_axis_alignment(CrossAxisAlignment::Stretch),
            )
            .ignoring(!visible),
        ));
        if let Some(panel) = menu_panel(app, menu) {
            layers.push(fade.wrap(
                IgnorePointer::new(popup_layer(app, motion::pop_in(panel, 8.0).into())).ignoring(!visible),
            ));
        }
        if menu == Menu::Peers {
            layers.push(peers_dialog(app));
        }
    }

    // The player owns keyboard focus while it is on screen, and its buttons
    // are kept out of the focus system: a focused button would swallow
    // Space.
    let (hover, keys) = (app.clone(), app.clone());
    let mut surface_layers: Vec<Widget> =
        vec![ExcludeFocus::new(Stack::new(layers).fit(StackFit::Expand)).into()];
    // The room panel takes focus for its chat field, so it sits outside the
    // focus exclusion.
    if menu == Menu::Together && (visible || fade.opacity() > 0.0) {
        surface_layers.push(popup_layer(app, motion::pop_in(super::together::player_panel(app), 8.0).into()));
    }
    KeyboardListener::new(
        MouseRegion::new(Stack::new(surface_layers).fit(StackFit::Expand))
            .on_hover(move |_| hover.player_activity()),
    )
    .focus_node(app.controllers.player_focus.clone())
    .autofocus(true)
    .on_key(move |event| super::handle_key(&keys, &event))
    .into()
}
