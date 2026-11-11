# Incular gaps

Behaviour in Incular 0.1.1 that Spoutpeak works around. Each entry says what
happens, what we do instead, and where. When Incular fixes one, remove the
workaround and the entry.

## Rendering

### Paths are not antialiased
Filled and stroked paths are tessellated (lyon) and drawn with
`sample_count: 1` and no coverage AA, so icon edges look jagged. Rounded
rectangles and glyphs are fine (analytic / coverage masks).
- Workaround: none.
- Affects: every Lucide icon (`theme::icon`).

### Zero-area paths are culled before stroking
`render_plan::path_visible` tests the path's raw bounds against the clip. A
straight horizontal line (Lucide `minus`) has zero-height bounds, so it is
dropped inside any clipped area (scroll views, panels) even though its stroke
has area. The compositor adds a stroke margin; this check does not.
- Workaround: strokes are expanded to filled outlines on the CPU with
  `kurbo::stroke` (`theme::icon_mesh`).

### `PathView` is internal; `Icon` refits geometry to its bounds
`PathView` (fill + stroke) is only reachable through
`incular::widgets::internal`. `Icon` fills only, and scales the path to its
bounding box, which discards an icon set's built-in padding (Lucide's 24px
grid).
- Workaround: `CustomPaint` with our own scaled display list (`theme::icon_with`).

### Text shadows are not painted
`TextStyle::shadows` is stored and merged but never drawn.
- Workaround: subtitle edge styles draw offset copies of the text underneath
  (`ui/player.rs`, `edged_text`).

### `ClipRRect` clips to the bounding rectangle
Rounded corners are not clipped.
- Workaround: corners are baked into image alpha (`images::shaped`).

### `TextAlign::Center` clips shrink-wrapped text
- Workaround: one `Text` per line, centred by a `Column` (subtitle captions).

## Input and hit testing

### Horizontal scroll views ignore a vertical mouse wheel
`scroll_delta_for_axis` reads only `delta.x` for horizontal viewports, so a
regular mouse wheel cannot scroll them (Flutter maps vertical wheel to
horizontal scroll when there is no vertical scrollable).
- Workaround: `widgets::horizontal_scroller` wraps the view in a `Listener`
  and forwards `scroll_delta.y` to the controller.

### A vertical wheel over a horizontal row does not scroll the page
`scroll_viewports_at` takes the axis from the innermost viewport, so over a
horizontal list the vertical delta becomes 0 and the outer vertical page
never receives it. Flutter passes an unused delta on to the nearest
ancestor that scrolls on that axis.
- Workaround: `widgets::page_wheel` wraps poster rows in a `Listener` and
  forwards `scroll_delta.y` to the page controller (Home, Search).

### Scrollbar thumb is drawn vertically on horizontal scroll views
A horizontal `SingleChildScrollView` shows a vertical thumb at its right edge.
- Workaround: none (season chips were replaced by a dropdown).

### A layer stacked over a scroll view swallows its clicks
A `Stack` sibling above a `SingleChildScrollView` blocks taps on the list
below, even when that layer is `IgnorePointer::new(SizedBox::shrink())` or
contains only a small `Positioned` child.
- Workaround: the season dropdown layer is added only while it is open
  (`ui/detail.rs`, `episodes_pane`).

### Focus cannot be handed back programmatically
Clicking a button moves keyboard focus to it, and a focused button swallows
Space, which breaks the player's play/pause key. There is no way to give
focus back to a chosen node after the click.
- Workaround: the player wraps its controls in `ExcludeFocus` and re-requests
  `player_focus` on every tick (`ui/player.rs`, `player/mod.rs`,
  `keep_player_focus`).

## Controls

### `TextField` focus outline is drawn in the wrong place
With `TextFieldStyle::border_focused` set, the focus outline is drawn
around the inner editable text area instead of the field's outer rounded
box. It sits inset from the edges and ignores the field's corner radius
and padding.
- Workaround: `border_focused: None` (`theme::field_style`). Fields
  therefore have no focus indicator at all; only the caret shows which one
  is active.
- Affects: every text input (search, addon URL, sign-in, settings, the
  season search).

### `Button` has a fixed height
A controls `Button` cannot grow with multi-line content.
- Workaround: hand-built list tiles from `MouseRegion` + `GestureDetector` +
  `Container` with their own hover state (`ui/detail.rs`, `list_button`).

## Media

### No video surface
There is no widget that plays video or accepts decoded frames efficiently.
- Workaround: ffmpeg child processes decode to raw RGBA, and every frame is
  uploaded as a new `ImageHandle` (`player/engine.rs`). This costs a full
  CPU copy and a texture upload per frame, and it limits resolution.

## Dev tooling

### `Simulation::type_text` needs a focused field
Typed text is dropped unless a field already has focus, and there is no
simulation step that focuses a field by label.
- Workaround: none (scripts use recent-search chips or clicks instead).

### `Simulation::click` cannot find cards without a semantic label
Labels come only from text inside buttons, so poster cards (an image plus
text in a `GestureDetector`) can't be clicked by title.
- Workaround: scripts click by coordinate.

### Simulation coordinates are logical, captures are physical
`Simulation::click_at` takes logical pixels while `capture()` returns
physical pixels (1.5x on this machine), so coordinates read off a screenshot
must be divided by the scale factor. Not a bug, but worth documenting.
