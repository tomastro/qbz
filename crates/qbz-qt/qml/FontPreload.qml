// FontPreload — registers the SELECTED bundled typeface before any Text exists.
//
// WHY THIS FILE IS A SEPARATE DOCUMENT, loaded before Main.qml.
//
// A plain `Text` does NOT inherit a font from its parents, and it does NOT
// follow `ApplicationWindow.font` — that property propagates through Qt Quick
// CONTROLS only. Proven on this Qt build with a standalone scene: with a
// family set on the ApplicationWindow, a `Label` resolved to it and a `Text`
// sibling stayed on the APPLICATION font. This tree has 1082 plain `Text`
// items and 13 that name a family, so "the app's typeface" IS the application
// font, and nothing else.
//
// And the application font is only read when an item is CONSTRUCTED. Also
// proven, with PySide6 against this same Qt: calling QGuiApplication::setFont
// at runtime updated `QGuiApplication::font()` and left an already-built
// `Text` on its old family. So the font has to be set before the UI is built,
// which means the families have to be REGISTERED before that — and a family
// is only registered once something loads its file.
//
// Hence this document: `QQmlApplicationEngine::load` returns with its
// FontLoader already in the font database (verified: the family is present
// the instant load() returns), so main.rs can set the application font in the
// gap between this document and Main.qml. "System" leaves the Loader inactive:
// no empty URL is assigned and Qt keeps the operating-system default.
//
// It is deliberately NON-VISUAL — a QtObject, not a Window — so loading it
// adds a root object and nothing else.
//
// The four non-system faces are settings_qt::APP_FONT_VALUES[1..], in order.
// Noto Sans Devanagari is registered separately by font_qt as a script-only
// fallback; it is never an application-font selection.

import QtQuick
import com.blitzfc.qbz

QtObject {
    id: root
    readonly property string selectedSource: {
        if (QbzShell.appFontFamily === "LINE Seed JP")
            return "assets/fonts/LINESeedJP-Regular.ttf"
        if (QbzShell.appFontFamily === "Montserrat")
            return "assets/fonts/Montserrat-VariableFont_wght.ttf"
        if (QbzShell.appFontFamily === "Noto Sans")
            return "assets/fonts/NotoSans-VariableFont_wdth,wght.ttf"
        if (QbzShell.appFontFamily === "Source Sans 3")
            return "assets/fonts/SourceSans3-VariableFont_wght.ttf"
        return ""
    }
    readonly property Component selectedComponent: Component {
        FontLoader { source: root.selectedSource }
    }
    readonly property Loader selected: Loader {
        active: root.selectedSource !== ""
        sourceComponent: root.selectedComponent
    }
}
