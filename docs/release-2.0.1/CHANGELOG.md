# QBZ 2.0.1

Maintenance release on top of the 2.0 native rebuild — performance, resilience
and text/rendering fidelity, plus a few features that missed the 2.0 cutoff.

## Performance
- Viewport/element windowing for Home, Discover, Favorites, Search, Labels,
  Artist/Award grids and the shared carousels (build + decode only what's on
  screen).
- Equalizer bars and animated surfaces driven from a coarse clock; near-zero
  idle/paused CPU.

## Rendering & resilience
- Autonomous renderer degradation ladder with a UI-loop watchdog and an
  auto-revert sentinel.
- Startup crash-chain watchdog (reset bad last-view, bypass queue restore after
  repeated early deaths).
- Single-instance guard on Linux via the session-bus name (#544, #559).
- High-performance GPU adapter by default on hybrid desktops (#542); wgpu
  surface-acquire timeout treated as a skipped frame.

## Text & internationalization
- CJK glyphs render at non-regular weights on macOS (#543); Korean TTC faces
  loaded where the Skia renderer refused them.
- First line always drawn under `overflow: elide`.
- HTML entities decoded in API prose, including malformed bare forms.
- Lenient per-item parsing everywhere a single bad item blanked a list.
- Dutch locale — 8th bundled language.

## Themes
- Auto theme derived from desktop accent, wallpaper or a chosen image.
- Custom theme editor panel with a ColorPicker primitive, persistence and
  startup wiring.

## Playback & audio
- Auto-skip unavailable tracks instead of parking on them.
- Stream errors surfaced as toasts, sandbox-aware for ALSA under Flatpak/Snap.
- DLNA/cast: strict-renderer-compatible discovery, DIDL and Play self-heal;
  no more full-track buffer hoarding in the media server.
- Forced PipeWire clock released on app quit (#521).
- Dead-key compose commits delivered to text fields; zbus D-Bus executor no
  longer forced onto tokio.

## Interface
- Queue per-track menu: "Remove all after" and "Add to playlist" now work.
- Home renders all 13 configurable sections; Recently Played "View all" page
  with its own album history.
- Album favorite action is a real toggle and syncs app-wide.
- Guest local-only profile — offline mode without a prior login.
- "Date added" playlist sort restored; Library Tracks tab sort control.
- Log viewer opens expanded (Copy all + Upload) with an always-visible Share
  logs entry.
- macOS title bar restart-to-apply toggle; genre-filter rows fully clickable
  in the advanced tree; Search artists carousel sized for the taller cards.

## Qobuz Connect
- Device-name setting restored from the Tauri build.
- Startup behavior setting (remember state / on / off).

## Packaging
- Nix: libjack2 added to buildInputs.
- Snap/Gentoo CI recipe fixes.

**Full changelog:** https://github.com/vicrodh/qbz/compare/v2.0.0...v2.0.1
