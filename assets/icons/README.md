# Interface icons

The glyphs in `glyphs.json` are extracted from the original icon packages,
without redrawing or expanding their outlines. Each entry records the package,
version, component, and weight. SVG circles and ellipses are represented as
equivalent SVG arcs; invisible export bounds are omitted. Coordinates remain on
the original 24 by 24 grid. Individual path fill rules and stroke settings are
preserved by the native renderer.

- **Solar Icons by 480 Design**, supplied by `@solar-icons/react` **1.1.1**:
  [original collection](https://www.figma.com/community/file/1166831539721848736),
  [package source](https://github.com/saoudi-h/solar-icons),
  [CC BY 4.0 artwork license](https://creativecommons.org/licenses/by/4.0/).
  Package and third-party notices are in `SOLAR-LICENSE` and
  `SOLAR-LICENSE-THIRD-PARTY`.
- **MingCute Icons by MingCute Design**, supplied by `@mingcute/react` **1.4.1**:
  [original collection](https://www.mingcute.com/),
  [source](https://github.com/Richard9394/MingCute),
  Apache License 2.0, reproduced in `MINGCUTE-LICENSE`.

The navigation uses MingCute Home7Fill, Search3Fill, BookmarksFill and
Settings4Fill, matching Liner's navigation choices. Playback uses Solar Bold
Play, Pause, RewindBack, RewindForward and SkipNext. Secondary controls use the
corresponding Solar Bold or Outline variants. Only the icon libraries are
vendored; no Liner application code is included.
