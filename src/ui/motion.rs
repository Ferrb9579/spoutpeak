//! Motion: spring-driven scrolling, the frame clock that drives it, and small
//! helpers for entrance animations.
//!
//! Incular 0.1.1 has no `ScrollController::animate_to` and no public ticker
//! (see INCULAR-GAPS-2.md). Instead, one unbounded `AnimationController` is
//! kept ticking by `Widget::animation_ticker` at the root of the tree while
//! anything moves. Its listener runs [`step`] once per frame, which moves
//! every gliding scroll view along a critically damped spring towards its
//! target with `jump_to`.
//!
//! The mouse wheel goes through the same springs. Scroll views that use
//! [`wheel`] must also set [`APP_SCROLLED`] physics, which turns off
//! Incular's instant per-event jump (a `Listener` cannot claim the event
//! from the viewport).

use incular::prelude::*;
use incular::widgets::internal::{OpacityController, TranslationController};
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

/// Physics for scroll views whose wheel input is handled by [`wheel`]:
/// Incular's own wheel handling is off, programmatic jumps and the
/// scrollbar thumb still work.
pub const APP_SCROLLED: ScrollPhysics = ScrollPhysics::NEVER_SCROLLABLE;

/// How a glide settles.
#[derive(Clone, Copy)]
pub enum Feel {
    /// Mouse wheel: quick, most of the travel happens in about 100 ms.
    Wheel,
    /// Buttons that page a row: a longer, softer glide (about half a second).
    Page,
}

impl Feel {
    /// Natural frequency of the critically damped spring, in rad/s.
    fn omega(self) -> f32 {
        match self {
            Feel::Wheel => 20.0,
            Feel::Page => 10.0,
        }
    }
}

struct Glide {
    controller: ScrollController,
    target: f32,
    position: f32,
    velocity: f32,
    omega: f32,
    /// The offset this glide last wrote; anything else means another input
    /// (a scrollbar drag, a programmatic jump) took over.
    written: f32,
}

type Start = Box<dyn FnOnce(Instant)>;

struct Motion {
    clock: AnimationController,
    glides: RefCell<Vec<Glide>>,
    /// Animations requested during a build, started on the next frame so a
    /// slow build doesn't use up their duration before anything is shown.
    starts: RefCell<Vec<Start>>,
    last_step: Cell<Option<Instant>>,
    /// The wheel event the innermost scroll view already used. Incular
    /// delivers one event to every `Listener` on the hit path, deepest
    /// first, with the same timestamp.
    claimed: Cell<Option<Instant>>,
}

thread_local! {
    static MOTION: Motion = Motion::new();
}

impl Motion {
    fn new() -> Self {
        let clock = AnimationController::unbounded();
        clock.add_listener(|_| step());
        Self {
            clock,
            glides: RefCell::new(Vec::new()),
            starts: RefCell::new(Vec::new()),
            last_step: Cell::new(None),
            claimed: Cell::new(None),
        }
    }
}

/// Keeps the clock running for as long as any glide is in flight. The value
/// is the elapsed time, so it changes (and notifies) on every frame.
struct WhileGliding;

impl Simulation for WhileGliding {
    fn position(&self, t: Duration) -> f32 {
        // Offset by one so the first frame of a run differs from where the
        // last run stopped (0 for the very first run), which makes the
        // controller notify on that frame too.
        1.0 + t.as_secs_f32()
    }
    fn velocity(&self, _: Duration) -> f32 {
        1.0
    }
    fn is_done(&self, _: Duration) -> bool {
        MOTION.with(|m| m.glides.borrow().is_empty() && m.starts.borrow().is_empty())
    }
}

/// Runs `start` with the time of the next frame.
fn on_next_frame(start: impl FnOnce(Instant) + 'static) {
    MOTION.with(|m| m.starts.borrow_mut().push(Box::new(start)));
    ensure_running();
}

/// Wraps the app so the motion clock ticks with the tree's frames.
pub fn clock(child: impl Into<Widget>) -> Widget {
    let clock = MOTION.with(|m| m.clock.clone());
    Widget::animation_ticker(clock, child)
}

fn ensure_running() {
    MOTION.with(|m| {
        if !m.clock.is_active() {
            m.last_step.set(None);
            m.clock.animate_with(WhileGliding);
        }
    });
}

fn reduced_motion() -> bool {
    incular::animation::disable_animations()
}

/// One frame of every glide.
fn step() {
    let now = Instant::now();
    let starts = MOTION.with(|m| std::mem::take(&mut *m.starts.borrow_mut()));
    for start in starts {
        start(now);
    }
    let (mut glides, dt) = MOTION.with(|m| {
        let dt = m
            .last_step
            .replace(Some(now))
            .map_or(1.0 / 60.0, |last| now.duration_since(last).as_secs_f32());
        // The spring is solved exactly, so a long frame is fine: clamping it
        // made the glide run slower than real time on a busy screen.
        (std::mem::take(&mut *m.glides.borrow_mut()), dt.clamp(0.0, 0.25))
    });
    glides.retain_mut(|glide| {
        let controller = glide.controller.clone();
        if (controller.offset() - glide.written).abs() > 1.0 {
            return false;
        }
        let max = controller.max_offset();
        glide.target = glide.target.clamp(0.0, max);
        // Critically damped spring, solved exactly over `dt`:
        // x(t) = (x0 + (v0 + w x0) t) e^(-w t).
        let w = glide.omega;
        let x = glide.position - glide.target;
        let b = glide.velocity + w * x;
        let decay = (-w * dt).exp();
        let next = (x + b * dt) * decay;
        glide.velocity = (b - w * (x + b * dt)) * decay;
        glide.position = glide.target + next;
        if glide.position <= 0.0 || glide.position >= max {
            glide.position = glide.position.clamp(0.0, max);
            glide.velocity = 0.0;
        }
        let settled = (glide.position - glide.target).abs() < 0.5 && glide.velocity.abs() < 20.0;
        if settled {
            glide.position = glide.target;
        }
        controller.jump_to(glide.position);
        glide.written = controller.offset();
        !settled
    });
    MOTION.with(|m| {
        // Keep glides started while this step ran.
        let mut current = m.glides.borrow_mut();
        glides.append(&mut current);
        *current = glides;
    });
}

/// Glides `controller` by `delta` (added to any glide already in flight, so
/// repeated input builds speed). Returns false when the view is already at
/// that end, so the input can go to an outer view.
pub fn glide_by(controller: &ScrollController, delta: f32, feel: Feel) -> bool {
    if delta == 0.0 || !delta.is_finite() {
        return false;
    }
    if reduced_motion() {
        return controller.scroll_by(delta);
    }
    let max = controller.max_offset();
    let started = MOTION.with(|m| {
        let mut glides = m.glides.borrow_mut();
        let existing = glides.iter().position(|g| g.controller == *controller);
        let from = existing.map_or_else(|| controller.offset(), |i| glides[i].target);
        let target = (from + delta).clamp(0.0, max);
        if (target - from).abs() < 0.01 {
            return false;
        }
        match existing {
            Some(index) => {
                let glide = &mut glides[index];
                glide.target = target;
                glide.omega = feel.omega();
            }
            None => {
                let offset = controller.offset();
                glides.push(Glide {
                    controller: controller.clone(),
                    target,
                    position: offset,
                    velocity: 0.0,
                    omega: feel.omega(),
                    written: offset,
                });
            }
        }
        true
    });
    if started {
        ensure_running();
    }
    started
}

thread_local! {
    /// When the last wheel notch arrived, and the boost it got.
    static LAST_NOTCH: Cell<Option<(Instant, f32)>> = const { Cell::new(None) };
}

/// Wheel acceleration: notches in quick succession travel further (up to
/// 4x), so a fast flick covers the page while a single notch stays precise.
fn wheel_boost(at: Instant) -> f32 {
    LAST_NOTCH.with(|last| {
        let boost = match last.get() {
            // The same event seen by an outer view: same boost.
            Some((then, boost)) if then == at => return boost,
            Some((then, boost)) => {
                let gap = at.saturating_duration_since(then).as_secs_f32();
                if gap < 0.12 { (boost * 1.35).min(4.0) } else { 1.0 }
            }
            None => 1.0,
        };
        last.set(Some((at, boost)));
        boost
    })
}

/// Which wheel axis a scroll view follows.
#[derive(Clone, Copy)]
pub enum WheelAxis {
    Vertical,
    /// Sideways wheel or trackpad only, so a vertical wheel passes through to
    /// the page (poster rows).
    Horizontal,
    /// Sideways input, and the vertical wheel as well (chip bars).
    HorizontalOrVertical,
}

/// Routes the mouse wheel over `view` into a spring glide of `controller`.
/// `view` must use [`APP_SCROLLED`] physics. Nested views chain: the
/// innermost one that can still move in that direction takes the event.
pub fn wheel(controller: &ScrollController, axis: WheelAxis, view: impl Into<Widget>) -> Widget {
    let controller = controller.clone();
    Listener::new(view)
        .on_pointer_signal(move |event| {
            let (x, y) = (event.scroll_delta.x, event.scroll_delta.y);
            let delta = match axis {
                WheelAxis::Vertical => y,
                WheelAxis::Horizontal => x,
                WheelAxis::HorizontalOrVertical => {
                    if x != 0.0 {
                        x
                    } else {
                        y
                    }
                }
            };
            if delta == 0.0 {
                return;
            }
            let claimed = MOTION.with(|m| m.claimed.get() == Some(event.time));
            let delta = delta * wheel_boost(event.time);
            if !claimed && glide_by(&controller, delta, Feel::Wheel) {
                MOTION.with(|m| m.claimed.set(Some(event.time)));
            }
        })
        .into()
}

// ------------------------------------------------------------- entrances

/// A compositor-only fade and rise, replayed by [`Entrance::play`]. Nothing
/// below it rebuilds while it runs.
#[derive(Clone)]
pub struct Entrance {
    opacity: OpacityController,
    offset: TranslationController,
}

impl Entrance {
    pub fn new() -> Self {
        Self {
            opacity: OpacityController::new(),
            offset: TranslationController::new(),
        }
    }

    /// Starts the entrance from transparent and `rise` pixels lower.
    pub fn play(&self, duration: Duration, rise: f32) {
        if reduced_motion() {
            self.opacity.set_opacity(1.0);
            self.offset.set_offset(Offset::ZERO);
            return;
        }
        // Hidden from the first frame; moving from the frame after the build.
        self.opacity.set_opacity(0.0);
        self.offset.set_offset(Offset::new(0.0, rise));
        let this = self.clone();
        on_next_frame(move |now| {
            this.opacity.animate_to(1.0, duration, now);
            this.opacity.animation().set_curve(Curve::EaseOut);
            this.offset.animate_to(Offset::ZERO, duration, now);
            this.offset.animation().set_curve(Curve::EaseOutCubic);
        });
    }

    /// The opacity on screen right now.
    pub fn opacity(&self) -> f32 {
        self.opacity.opacity()
    }

    /// Fades to `opacity` from wherever it is now.
    pub fn fade_to(&self, opacity: f32, duration: Duration) {
        if reduced_motion() {
            self.opacity.set_opacity(opacity);
            return;
        }
        let this = self.clone();
        on_next_frame(move |now| {
            this.opacity.animate_to(opacity, duration, now);
            this.opacity.animation().set_curve(Curve::EaseOut);
        });
    }

    pub fn wrap(&self, child: impl Into<Widget>) -> Widget {
        FadeTransition::new(
            self.opacity.clone(),
            SlideTransition::new(self.offset.clone(), child),
        )
        .into()
    }
}

/// A short fade and rise for something that just appeared (menus, toasts).
/// Replays only when the widget is newly mounted.
pub fn pop_in(child: impl Into<Widget>, rise: f32) -> Widget {
    if reduced_motion() {
        return child.into();
    }
    child
        .animate()
        .duration(Duration::from_millis(160))
        .curve(Curve::EaseOutCubic)
        .fade_in()
        .move_from(Offset::new(0.0, rise))
        .into()
}
