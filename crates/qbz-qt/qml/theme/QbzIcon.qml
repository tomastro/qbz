// Tinted SVG icon — the QML equivalent of Slint's QbzIcon (glyphs recolored
// to a theme tint).
//
// HOW TINTING WORKS HERE (and why it is not what Slint does)
//
// Slint's QbzIcon is `Image { colorize: <live Theme token> }`: the renderer
// recolours the glyph every frame, so an icon always matches the current
// theme, all 36 of them, custom accent included. Qt has no *free* equivalent,
// which is why this file used to point straight at PRE-BAKED variants,
// assets/icons/<tint>/<name>.svg, each with a fixed hex burned in.
//
// SUPERSEDED (2026-07-29): the reason recorded here used to be "ColorOverlay /
// MultiEffect / shaders draw NOTHING", from a POC probe. Effects need shaders,
// and this port runs on the GPU (OpenGL RHI, measured with QSG_INFO=1 in a real
// windowed session); the probe was taken under QT_QPA_PLATFORM=offscreen, which
// forces the software renderer by definition. Where a software path is
// genuinely possible, detect it with `GraphicsInfo.api` rather than assuming it
// — theme/RoundedImage.qml does exactly that.
// This does NOT re-open the design below. The runtime tint generator is cached,
// costs zero per frame, and serves all 36 themes; a per-glyph shader pass would
// be a rewrite, not a fix. Annotation only.
//
// Fixed hexes cannot serve 36 themes: the bakes were cut against the dark
// palette, so `secondary` (#cccccc) and `muted` (#888888) were near-white
// glyphs on near-white surfaces on every LIGHT theme, and `accent` was
// #4285f4 whatever the theme's accent actually was.
//
// So the tinted set is now GENERATED AT RUNTIME, into the user cache dir, by
// src/icon_tint_qt.rs: `QbzIconTint.dirFor(tint, themeJson)` returns the
// file:// directory holding this tint's glyphs for the live theme, and the
// Image loads a real .svg off disk — byte for byte the same load path as the
// qrc bakes, no effects involved. Passing `QbzShell.themeJson` is what makes
// the binding re-resolve on a live theme switch (the same dependency trick as
// `QbzSession.tr(msgid, trRev)`), and it is the colour source at the same
// time.
//
// ---------------------------------------------------------------------------
// THE TINT VOCABULARY — read this before adding a call site
// ---------------------------------------------------------------------------
//
// THEME-FOLLOWING (runtime-tinted, correct under all 36 themes):
//   "textPrimary"    Theme.text-primary — max contrast on a THEME surface
//   "secondary"      Theme.text-secondary   (alias: "textSecondary")
//   "muted"          Theme.text-muted       (alias: "textMuted")
//   "disabled"       Theme.text-disabled    (alias: "textDisabled")
//   "accent"         Theme.accent
//   "accentText"     the glyph colour ON an accent fill
//   "warning"        Theme.warning
//   "favorite"       Theme.favorite
//
// FIXED (theme-independent by design, served from the qrc bakes):
//   "white"          a light glyph on a host that is dark under EVERY theme:
//                    artwork scrims (#a6000000 / #b3000000), gradient discs,
//                    the close button's #e81123 hover
//   "primary"        LEGACY ALIAS OF "white" — a literal #ffffff, NOT
//                    Theme.text-primary. The app-wide migration is done: the
//                    fixed-host call sites now say "white" and the ones that
//                    always meant the theme token say "textPrimary". Only two
//                    call sites still spell it "primary"
//                    (controls/BrowseGenreButton.qml, views/LibraryView.qml —
//                    the glyph on an accent fill, where Slint hardcodes
//                    #ffffff but the sibling LABEL uses Theme.accent-text, so
//                    the intent is genuinely ambiguous and is the owner's
//                    call). The alias stays so those two keep working; do not
//                    add new ones.
//   "black"          a dark glyph on a host painted light under every theme
//   "amber"          #e0b341, the audio stamp's brand accent (one icon)
//   "green"          #22c55e, PmLocalBadge's "all local" state (one icon:
//                    wifi). A literal brand colour, not a theme token — the
//                    `amber` precedent, one partial dir for one glyph.
//   "orange"         #f59e0b, PmLocalBadge's "some local" state (one icon:
//                    cloud-download). Same precedent.
//
// Getting this pair wrong is how the bug this file fixes happened in reverse:
// resolving "primary" to Theme.text-primary would paint a DARK glyph on a
// dark scrim on light themes. Slint keeps the two apart explicitly
// (CircleAction.slint:70-74) and so does this.
//
// ---------------------------------------------------------------------------
// THE QRC BAKES REMAIN, AS THE FLOOR
// ---------------------------------------------------------------------------
//
// `dirFor` returns "" — and this file falls back to assets/icons/<dir>/ —
// whenever the tint is fixed, there is no writable cache dir, the bake
// failed, the theme document has not been published yet, or QbzIconTint is
// not registered at all (see the `typeof` guard: an unregistered singleton
// resolves LAZILY and would otherwise take every icon in the app down with
// it). It also falls back per-icon on Image.Error, so a name that exists in
// the qrc set but not in the runtime masters degrades to the old fixed colour
// instead of rendering nothing. Verified against src/icon_tint_qt.rs: every
// one of those paths is a real `QString::default()` return (`tint_root()`
// None, `bake()` false, empty/tokenless theme_json), plus one more that is
// not on the list — `prune()` sweeps the OLDEST directories past
// MAX_TINT_DIRS = 32, and a dir reused from a previous session keeps its old
// mtime because `bake()` early-returns on the `.complete` stamp, so the
// CURRENT theme's dir can be deleted under the running app and every Image
// 404s into `qrcFallback`. The floor is not a formality; it is reachable.
//
// AND THE FLOOR HAS TO CARRY THE POLARITY. Mapping a theme-following tint to
// a fixed dir is only harmless while the runtime bake is alive: send
// "textPrimary" to primary/ (a literal #ffffff) and the degraded path paints
// white-on-near-white across the 11 light themes, at ~35 call sites — the
// exact bug the runtime tinting exists to fix. So `qrcTint` below picks its
// dir from the LIVE token, with the same `hslLightness > 0.5` test the call
// sites carried before they were collapsed into this vocabulary.
//
// THE ONE WAY A BAKE STILL SILENTLY LIES — worth keeping in mind when adding
// an icon: NO VARIANT ON DISK -> the Image resolves to nothing and the glyph
// is simply ABSENT, with nothing logged. The runtime sets are always complete
// (every master, every tint); the qrc dirs were NOT, which is why the floor
// could not be trusted: `black/` and `warning/` were missing mp3 + translate,
// `secondary/` and `accent/` were missing mp3 (and `black/cd.svg` was a
// #cccccc glyph — a light-grey disc in the DARK-glyph directory). All six
// neutral/chromatic dirs are now 115 files, the same set as primary/ and as
// MASTERS; only `favorite/` (57), `amber/` (6) and the two one-glyph brand
// dirs `green/` / `orange/` stay deliberately partial, and none of them is
// reachable from the polarity mapping. A new icon goes into
// assets/icons/primary/, into the MASTERS table in src/icon_tint_qt.rs, AND
// into black/ + secondary/ + muted/ if any theme-following tint can ask for
// it.

import QtQuick
import com.blitzfc.qbz

Image {
    id: root
    // Icon file name, e.g. "compass" (".svg" appended).
    property string name: ""
    // One of the names in the vocabulary above.
    property string tintName: "secondary"

    // The HSL lightness test for one theme token, read straight out of the
    // live document without parsing it. `QbzShell.themeJson` is COMPACT serde
    // output (theme_qt.rs:266 `serde_json::to_string`) and every colour goes
    // through `argb()` (theme_qt.rs:124-126), so a token always reads exactly
    //     "textPrimary":"#ff0f0f0f"
    // Reading it costs one indexOf over ~2.7 KB — next to nothing beside the
    // QbzIconTint.dirFor() round trip this file already makes per icon (which
    // hashes that whole string). The alternatives were both per-icon and much
    // worse: a JSON.parse here, or a `QbzTheme { }` instance inside QbzIcon —
    // ~70 eager bindings AND a parse each, multiplied by the hundreds of
    // icons alive at once and re-run on every theme switch. QbzIcon is the
    // one component in this tree that cannot afford an owned QbzTheme.
    //
    // Missing / unparsable -> true (light), which is the same default
    // QbzTheme.qml takes pre-publish: its baked fallback document is the DARK
    // palette, whose text-primary IS #ffffff.
    function tokenIsLight(key) {
        // Also the binding dependency — property reads are captured through
        // the call, so `qrcTint` re-resolves when the theme is republished.
        const json = QbzShell.themeJson
        const needle = '"' + key + '":"#'
        const at = json.indexOf(needle)
        if (at < 0)
            return true
        const end = json.indexOf('"', at + needle.length)
        if (end < 0)
            return true
        const hex = json.substring(at + needle.length, end)
        if (hex.length < 6)
            return true
        // Drop the alpha byte of #aarrggbb (6-digit values work unchanged).
        const o = hex.length - 6
        const r = parseInt(hex.substring(o, o + 2), 16)
        const g = parseInt(hex.substring(o + 2, o + 4), 16)
        const b = parseInt(hex.substring(o + 4, o + 6), 16)
        if (isNaN(r) || isNaN(g) || isNaN(b))
            return true
        // (max + min) / 2 over 0..255 — `color.hslLightness > 0.5`, the
        // metric the per-view ternaries used before the vocabulary landed.
        return (Math.max(r, g, b) + Math.min(r, g, b)) / 510 > 0.5
    }

    // The qrc directory this tint falls back to. Every tint name — including
    // the ones with no directory of their own ("white", "textPrimary",
    // "accentText" ...) — must land on a REAL dir, or the fallback renders
    // nothing at all. And a theme-following name must land on a dir whose
    // POLARITY matches the token it stands for (see the header): this is the
    // floor, so it is what a light theme gets when the bake is unreachable.
    readonly property string qrcTint: {
        switch (root.tintName) {
        // Fixed by design — a light glyph on a host that is dark under every
        // theme (scrims, gradient discs, the #e81123 close hover). "primary"
        // is the legacy alias of "white" and stays a literal #ffffff.
        case "white":
        case "primary":
            return "primary"

        // Theme-following. Both of these read their OWN token, never the
        // theme polarity: accent-text is near-BLACK on plenty of DARK themes
        // whose accent fill is bright (qbz-theme registry.rs — tokyo-night
        // #1a1b26, catppuccin-mocha #1e1e2e, ...), so `isDark` would get it
        // backwards in both directions.
        case "textPrimary":
            return root.tokenIsLight("textPrimary") ? "primary" : "black"
        case "accentText":
            return root.tokenIsLight("accentText") ? "primary" : "black"

        // The weak text ramp: #cccccc is unreadable on a light theme's
        // surfaces, #888888 is the mid-grey that survives both polarities.
        case "secondary":
        case "textSecondary":
            return root.tokenIsLight("textSecondary") ? "secondary" : "muted"

        // muted / disabled stay on muted/ under BOTH polarities on purpose.
        // #888888 reads on white and on near-black alike, and it is the only
        // mid-grey bake there is: routing these by lightness would send the
        // dark themes' text-disabled (#555555, lightness 0.33) to black/ —
        // an invisible glyph on a dark surface, the bug in mirror image.
        case "muted":
        case "textMuted":
        case "textDisabled":
        case "disabled":
            return "muted"

        // "accent", "warning", "favorite" (chromatic, legible on both), and
        // the fixed "black" / "amber" / "green" / "orange" — each already
        // names its own dir. The three fixed ones also fall out of the
        // runtime bake by themselves: `token_for()` has no entry for them
        // (icon_tint_qt.rs:323), so `dirFor` returns "" and this qrc dir is
        // what actually serves them.
        default:
            return root.tintName
        }
    }

    // Absolute qrc URL: relative URLs in a shared component resolve
    // against the CONSUMER's document depth (phase-23 dir layout — root
    // consumers underflowed one level).
    readonly property string qrcDir:
        "qrc:/qt/qml/com/blitzfc/qbz/qml/assets/icons/" + root.qrcTint

    // The runtime-tinted directory for this tint under the live theme, or ""
    // when the qrc bake must be used (see above).
    readonly property string tintDir: {
        // Dependency: re-resolve every icon when the theme is republished.
        const themeJson = QbzShell.themeJson
        // `typeof` on an unregistered type name does not throw (plain JS
        // global lookup), so a missing registration degrades to the bakes
        // instead of poisoning every source binding with `undefined`.
        if (typeof QbzIconTint === "undefined")
            return ""
        const dir = QbzIconTint.dirFor(root.tintName, themeJson)
        return (typeof dir === "string") ? dir : ""
    }

    // Set once, per icon, if the runtime set has no such glyph (a master the
    // table does not carry yet). Cleared whenever the inputs change so a
    // recycled delegate is never stuck on the fallback.
    property bool qrcFallback: false
    onNameChanged: root.qrcFallback = false
    onTintNameChanged: root.qrcFallback = false
    onTintDirChanged: root.qrcFallback = false

    readonly property string activeDir:
        (root.tintDir === "" || root.qrcFallback) ? root.qrcDir : root.tintDir

    source: root.name === "" ? "" : root.activeDir + "/" + root.name + ".svg"
    onStatusChanged: {
        if (status === Image.Error && !root.qrcFallback && root.tintDir !== "")
            root.qrcFallback = true
    }

    sourceSize: Qt.size(Math.max(1, Math.round(width * 2)), Math.max(1, Math.round(height * 2)))
    fillMode: Image.PreserveAspectFit
    asynchronous: false
}
