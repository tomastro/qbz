// PmFolderIcon — the folder icon tile (primitives/PmFolderIcon.slint, 48
// lines): a rounded colour square carrying either one of the seven lucide
// preset glyphs or a user-picked custom image.
//
// It lives in qml/controls/, not in qml/views/playlistmanager/, because it is
// shared chrome rather than a view part: the three manager consumers today are
// the folder card, the folder chip and the tree folder row, and the folder
// EDITOR (controls/FolderEditPanel.qml) is the fourth the moment it grows a
// live preview tile — the reference's FolderEditModal.slint has none, so the
// port draws none either (rule 5: no control the reference does not have). A
// view-part directory would make a control import a view (contract D12).
//
// ── FLAT FIELDS, NOT A FOLDER OBJECT ───────────────────────────────────────
// The reference takes one `PmFolderItem` struct. Here the publishers carry
// DIFFERENT shapes — `QbzPlaylistManager.foldersJson` entries (§4.1) and
// `QbzFolderEdit.editJson` (§4.5, which additionally carries the swatch and
// preset constants and the editor's busy flag, and whose `iconColor` is NOT
// validity-gated because it is a swatch SEED, not a paint) — so passing "the
// folder object" would mean one of them silently handing over the wrong keys.
// QML does not report that: a property the target type does not declare is an
// ignored line, and a key the object does not carry is `undefined`. The
// interface is therefore the six flat fields of contract §2.5 and nothing
// else.
//
// ── THE GLYPH SITS UNDER THE IMAGE, NOT OPPOSITE IT ────────────────────────
// The reference pairs `if has-custom-image: Image` with
// `if !has-custom-image: QbzIcon`, so a tile whose image fails to decode — or
// has not decoded yet — is a bare coloured square. Here the glyph is always
// drawn and the image is layered over it: identical once the image loads,
// degrades to the preset glyph when it does not (§5.20).
//
// ── TINT IS "white", DELIBERATELY ──────────────────────────────────────────
// A fixed #ffffff, matching the reference's hardcoded tint — NOT
// "textPrimary". The host is a saturated, user-chosen colour tile under every
// theme, and a theme-following tint would paint a dark glyph on it on the 11
// light themes. ("primary" is the legacy alias of "white"; QbzIcon.qml:55-65
// says do not add new call sites for it.)
//
// ── NO `clip: true` ────────────────────────────────────────────────────────
// `Rectangle { radius; clip: true }` does NOT round in Qt Quick — `clip` is a
// rectangular scissor. The fill Rectangle rounds itself and RoundedImage does
// the image's corners with a mask.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    id: root

    /// Edge length. The root binds BOTH dimensions off it — the reference's
    /// component does the same, and a sketch that omits it renders 0x0.
    property int size: 40
    /// One of the seven ids in §4.5. ANY unknown value renders "folder", which
    /// is also where the id "folder" itself lands: it is not a case in the
    /// reference's id -> glyph chain at all and falls through to the default
    /// arm. Both are upstream intent; both are kept.
    property string iconPreset: "folder"
    /// "" means "use the theme accent"; only read when `hasColor` is true.
    property string iconColor: ""
    /// False when the stored value was empty, a gradient or a CSS var — the
    /// validity gate lives in RUST (§5.20), so a stored gradient string can
    /// never reach `Qt.color()` here.
    property bool hasColor: false
    /// `icon_type == "custom" && custom_image_path.is_some()` — BOTH, decided
    /// in Rust. A stale path is reachable, so this is never derived from the
    /// path alone.
    property bool hasCustomImage: false
    /// A percent-encoded `file://…` URL, "" when there is none. A bare
    /// `/home/...` string would be resolved against this component's `qrc:`
    /// base and fail silently (D25).
    property string customImagePath: ""

    /// Glyph size and corner radius are DERIVED, because §2.5 gives this
    /// control `size` as its only geometry input. The three reference call
    /// sites pass 64/32/12 (grid card, PlaylistManagerView.slint:314),
    /// 36/20/8 (list chip, :397) and 28/15/6 (tree row, :903); the bands below
    /// reproduce all three exactly and interpolate sanely in between. Both are
    /// plain properties rather than readonly ones, so a call site that wants
    /// to pin the reference numbers can just assign them.
    property int glyphSize: root.size >= 56 ? Math.round(root.size * 0.5)
                          : root.size >= 32 ? Math.round(root.size * 0.556)
                                            : Math.round(root.size * 0.536)
    property int tileRadius: root.size >= 56 ? 12 : root.size >= 32 ? 8 : 6

    QbzTheme { id: theme }

    width: root.size
    height: root.size
    radius: root.tileRadius
    color: root.hasColor && root.iconColor !== "" ? root.iconColor : theme.accent

    QbzIcon {
        anchors.centerIn: parent
        width: root.glyphSize
        height: root.glyphSize
        tintName: "white"
        name: root.iconPreset === "heart" ? "heart"
            : root.iconPreset === "star" ? "star"
            : root.iconPreset === "music" ? "music"
            : root.iconPreset === "disc" ? "disc"
            : root.iconPreset === "library" ? "library"
            // The id/glyph mismatch is upstream intent, not a typo: the preset
            // persisted as "headphones" has always drawn audio-lines.
            : root.iconPreset === "headphones" ? "audio-lines"
            : "folder"
    }

    RoundedImage {
        anchors.fill: parent
        visible: root.hasCustomImage && root.customImagePath !== ""
        source: (root.hasCustomImage && root.customImagePath !== "")
                ? root.customImagePath : ""
        radius: root.tileRadius
        fit: "crop"
    }
}
