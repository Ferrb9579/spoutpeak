# Incular gaps

Problems in Incular 0.1.3 that Spoutpeak works around. Each entry says what
goes wrong, what Flutter does, and what Spoutpeak does about it (and
where). When Incular fixes one, remove the workaround and the entry.

Everything listed for 0.1.1 was re-checked against 0.1.2; the fixed entries
and their workarounds are gone (icons, text shadows, rounded clips,
centred text, wheel routing, layers over scroll views, focus hand-back,
the `TextField` focus border, growing buttons, `Ticker`, post-frame
callbacks, public transition controllers, tooltips over later siblings,
transparent layers, infinite constraints, blur clipping, button centring,
signal writes during a build, panics in `Simulation`).

Re-checked against 0.1.3: none of the entries below is fixed. `Tooltip`,
the controls `Button` and `Simulation` are unchanged, and 0.1.3 still has no
video widget (its new `ImageStream` is the loading state of one image).

## Overlays and gestures

### `Tooltip` takes the taps meant for its child
`incular::material::Tooltip` wraps its child in an `ActionSurface`, which
wins every tap on the child. A clickable tile inside a `Tooltip` no longer
reacts to clicks.
- Repro: `Tooltip::new(text, GestureDetector::new(tile).on_tap(..))`;
  click the tile: `on_tap` never fires.
- Flutter: `Tooltip` listens for hover and long press only; the child's
  tap recognizer still wins taps.
- Workaround: our own tooltip, a `MouseRegion` for hover plus an
  `OverlayPortal` placed at the pointer (`ui/tooltip.rs`).

### An ancestor `GestureDetector` still fires for taps on non-button descendants
0.1.2 lets an enabled `Button` win the tap over enclosing detectors, but a
`GestureDetector` inside another `GestureDetector` still triggers both
`on_tap`s.
- Flutter: the innermost tap recognizer wins the arena.
- Workaround: none needed now; overlapping tap targets are siblings in a
  `Stack` (`ui/player.rs`, `popup_layer` and the click-to-pause surface).

## Controls

### `ButtonStyle::animation_duration` does not animate the background
Hover and pressed *overlays* now fade (ink highlights), but a change of the
state-resolved background still happens in one frame.
- Workaround: none. Spoutpeak's buttons use background states with a
  transparent overlay, so their hover colours change instantly.

## Media

### No video surface
There is no widget that plays video or accepts decoded frames efficiently.
- Workaround: ffmpeg child processes decode to raw RGBA, and every frame is
  uploaded as a new `ImageHandle` (`player/engine.rs`). This costs a full
  CPU copy and a texture upload per frame, and it limits resolution.

## Dev tooling

### `Simulation::click` cannot find poster cards by title
A poster card (a `Button` holding an image, a title and a year) is not
found by its title, even with a `Semantics` label around it; labels must
match exactly and the card's label is not its title alone.
- Workaround: scripts click by coordinate (`clickat`, logical pixels;
  captures are physical, 1.5x on this machine).
