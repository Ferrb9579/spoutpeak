# Incular gaps (2)

Things Flutter has that Incular 0.1.1 does not, found while adding smooth
scrolling and UI animations. Each entry says what Flutter offers, what
Incular has instead, what Spoutpeak does about it, and where. When Incular
adds one, remove the workaround and the entry. See `INCULAR-GAPS.md` for bugs
in features that already exist.

Most workarounds live in `src/ui/motion.rs`.

## Scrolling

### `ScrollController` has no `animateTo`
Flutter's `ScrollController.animateTo(offset, duration:, curve:)` scrolls to
an offset over time. Incular's `ScrollController` only has `jump_to`,
`deferred_jump_to` and `scroll_by`, which all move instantly
(`incular-scroll/src/controller.rs`).
- Workaround: `motion::glide_by` keeps a target per controller and moves it
  along a critically damped spring, calling `jump_to` once per frame. New
  input adds to the target and keeps the current velocity. A glide stops
  when something else moves the controller (scrollbar drag, programmatic
  `jump_to`).
- Used by: the row chevrons (`widgets::row_header`) and the mouse wheel.

### A wheel event cannot be claimed away from a scroll view
Flutter's `PointerSignalResolver` lets a `Listener` register for a
`PointerScrollEvent` so only one handler acts on it. Incular has none:
`WidgetTree::scroll_at` delivers the signal to every `Listener` on the hit
path and then always applies it to the viewports under the pointer
(`incular-widgets/src/tree/interaction.rs`, `scroll_at`,
`scroll_viewports_at`).
- Workaround: views with smooth wheel scrolling set
  `ScrollPhysics::NEVER_SCROLLABLE` (`motion::APP_SCROLLED`) so the
  viewport ignores the wheel, and `motion::wheel` handles it instead.
  Incular calls listeners deepest first with the same event timestamp, so
  the innermost view that can still move claims the event by timestamp and
  outer ones skip it. When a view is at its end, the next one out takes
  the event, like Flutter's scroll chaining.
- Cost: `NEVER_SCROLLABLE` also turns off touch-drag scrolling in those
  views. The scrollbar thumb still works because it uses `jump_to`.
- This also replaces the earlier `page_wheel` workaround (see
  `INCULAR-GAPS.md`).

## Animation

### No public `Ticker` or `TickerProvider`
Flutter lets app code get a callback on every frame (`Ticker`,
`createTicker`), and `AnimationController(vsync:)` ticks itself. Incular's
`AnimationController` has no clock; the tree calls `tick(now)` only for a
controller wrapped in `Widget::animation_ticker`, which is
`#[doc(hidden)]` (`incular-widgets/src/tree/widget/constructors.rs`), or
one owned by a built-in transition widget.
- Workaround: `motion::clock` wraps the root in
  `Widget::animation_ticker` with one unbounded controller. While a glide
  or a deferred start is pending, the controller follows a simulation
  that never finishes, and its listener runs `motion::step` each frame.
  The simulation reports done when nothing is moving, so idle frames stop.

### No `AnimatedBuilder` / `ListenableBuilder`
Flutter's `AnimatedBuilder(animation:, builder:)` rebuilds a subtree when a
controller changes. Incular has this internally as `animated_widget`, but
it is `pub(crate)` (`incular-widgets/src/animation_transitions.rs`).
- Workaround: none needed so far. Entrances use compositor controllers
  (below) or `Animate` (`.animate()`), which is public.

### Transition controllers are only in `internal`
`FadeTransition`, `SlideTransition` and `ScaleTransition` are public, but
the controllers they take (`OpacityController`, `TranslationController`,
`ScaleController`) are exported only from `incular::widgets::internal`. In
Flutter the same widgets take an ordinary `Animation`.
- Workaround: `motion::Entrance` imports them from `internal`.
- Used by: page entrances (`ui/mod.rs`), image fade-in
  (`widgets::net_image`), the player controls' fade (`ui/player.rs`).

### Animations started during a build lose time to the build
`OpacityController::animate_to` and `TranslationController::animate_to`
take `now` and start right away. Flutter starts a ticker on the next frame.
If a slow build follows, as when a screen opens, the animation is mostly
over before its first frame. (`AnimationController::forward_on_next_tick`
exists, but these controllers make their own controller internally.)
- Workaround: `motion::Entrance` sets the start values during the build and
  calls `animate_to` from the motion clock on the next frame.

### No post-frame callback
Flutter's `SchedulerBinding.addPostFrameCallback` runs code after a frame.
Incular has none; `Runtime::request_frame` takes no callback
(`incular-runtime/src/frame.rs`).
- Workaround: the motion clock covers "run on the next frame" for
  animations. When the player controls fade out, they leave the tree after
  a timer (`ui/player.rs`), not a frame callback.

### `ButtonStyle::animation_duration` does not animate the background
In Flutter, a button's background, shape and elevation changes animate
(`ButtonStyle.animationDuration`, through `Material`). Incular's `Button`
passes the duration only to the `ActionSurface` splash and overlay. The
state-resolved background is a plain container that changes in one frame
(`incular-controls/src/button.rs`).
- Workaround: none. Hover and selection colours still change instantly.
