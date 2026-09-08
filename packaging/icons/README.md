# Canonical app icons

Canonical icon source for every Linux/macOS packager (nfpm deb/rpm, AppImage,
tarball, flatpak, snap, ebuilds, PKGBUILDs). This directory is the single
source of truth — the old `src-tauri/icons/` tree was removed in 2.0.2.

- `NNxNN.png` — hicolor sizes (32/48/64/128/256/512, plus `128x128@2x`)
- `QBZ.icon/` — macOS 26+ Icon Composer source for the main application icon.
  Apple renders its layered source as Default, Dark, and Mono appearances;
  Clear and Tinted are presentations of the monochrome appearance. The primary
  app-icon name is `QBZ`.
- `icon.icns` — legacy macOS bundle fallback. Keep it for builds that do not
  compile the Icon Composer source.

`QBZ.icon` is the unchanged attachment contributed by @LuckyTheCoder in
[issue #712](https://github.com/vicrodh/qbz/issues/712) (attachment SHA-256
`bef9e7bd8d0a1c340d73e3fe1b7bc410d43848b96907880727e993af38e96056`).
An Xcode 26+ macOS packager must compile this source as the primary app icon
and merge the generated asset-catalog/Info.plist output into `QBZ.app`; copying
the raw directory into `Resources` is not sufficient.

This directory controls the application/Finder/Dock icon only. The macOS menu
bar (tray) templates are separate runtime assets and must not be replaced by
the artwork here.
