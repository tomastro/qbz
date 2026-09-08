// ImmersiveView — the immersive root overlay (ImmersiveView.slint:1186-1705),
// the Qt port of contract 2026-08-02-immersive-port §5.1-§5.5. Mounted by
// AppShell AFTER QbzToast and BEFORE ArtPreviewOverlay/QbzTooltip (§5.1
// declaration-order z convention).
//
// BLOCK B2+B3+B4+B5 SCOPE: the overlay shell (root + click-blocker + chrome +
// auto-hide + exit paths 2-4 + the fullscreen guard; the seek arrows later
// moved to the hotkeys dispatcher — 2026-08-03 hotkeys-port §1.3) PLUS the
// B3 panels (the ImmersiveAtmosphere underlay, the five FOCUS panels Album
// Reactive / Static / Coverflow / Spectrum / Wave Bed, the layer-4
// ImmersiveSongCard) PLUS the B4 panels: FOCUS Lyrics (mode 4,
// LyricsFocusPanel) and Queue (mode 5, QueueTabsPanel), and the full §5.6
// SPLIT layout (50/50: artwork + ImmersiveTrackMeta LEFT, the Lyrics /
// TrackInfo / Suggestions / Queue panels RIGHT) with the §5.5 entry loads —
// PLUS the B5 layer-5 ImmersivePlayerBar (TinyBar/VolumeBar, §5.7/§5.8) and
// the B5 layer-7 ImmersiveSearchCortinilla (§3.4).
//
// Layer order bottom->top (§5.1):
//   1. root color #0a0a0b, clip (:1199-1200)
//   1b. ShaderSceneLayer (block A1, :1293-1300) — the GPU shader scene,
//       bottom-most VISUAL layer when QbzShaderScene.scene > 0, replacing
//       the atmosphere AND the FOCUS/SPLIT panels (their gates below gain
//       `scene === 0`, parity :1313-1405); chrome stays on top
//   2. ImmersiveAtmosphere — the underlay while NO scene is active
//      (gated `scene === 0`; in v1 the shader-mode==0 gate was constant,
//      ruling 1; :1313-1321)
//   2b. FULL-COVERAGE click-blocker MouseArea (load-bearing — without it
//      clicks pass through to the desktop header search field, the dock's
//      spectrum band and the viz eye toggle; port of the Slint root
//      TouchArea :1283-1286). Stacked BELOW panels + chrome.
//   3. FOCUS panels (viewMode==0 && mode==N) / the SPLIT layout (viewMode==1)
//   4. ImmersiveSongCard (Wave Bed, full Lyrics, scopes and shader scenes;
//      does NOT fade with the chrome)
//   5. ImmersivePlayerBar (B5, :1607-1616) — centered, y = height-90-24,
//      fades with the chrome like the header
//   6. ImmersiveHeader band
//   7. ImmersiveSearchCortinilla (B5, :1695-1705) — topmost

import QtQuick
import QtQuick.Effects
import QtQuick.Window
import com.blitzfc.qbz
import "../controls"
import "../shell"
import "../theme"

Item {
    id: root

    QbzTheme { id: theme }

    // Self-gated on the bridge open flag (§5.1: closed = invisible,
    // non-interactive, zero-cost). Every open/close transition funnels
    // through QbzImmersive's open setter (§3/D3) — this item only reacts.
    visible: QbzImmersive.open
    clip: true
    // focus: true while open so the root's Keys handler (Escape) is live;
    // the 30ms grab timer below re-asserts it after mount (:1259-1267).
    focus: QbzImmersive.open

    // §5.4 text-input gate (kept per 2026-08-03 hotkeys-port §1.3 — the
    // seek arrows and the Shift+I Shortcut it used to feed are GONE,
    // absorbed into the dispatcher's rebindable focus.seek*/ui.focusMode
    // actions; the central gate in QbzHotkeys.keyPressed now does their
    // job. Retained as the immersive surface's own "a text input is
    // focused" flag).
    readonly property bool textInputActive: header.searchActive

    // --- §5.4 fullscreen / shell guard ------------------------------------
    // Captured on the OPEN edge; on the close edge FullScreen is dropped
    // only if the window was NOT fullscreen before (port of
    // main.rs:8660-8716, as QML state — no 150ms timer, D3).
    property bool wasFullscreen: false
    // The kiosk host sets this true so closing Immersive never changes an
    // appliance window's fullscreen state. Desktop leaves the default false.
    property bool preKiosk: false

    Connections {
        target: QbzImmersive
        function onOpenChanged() {
            var w = root.Window.window
            if (QbzImmersive.open) {
                root.wasFullscreen = w !== null && w.visibility === Window.FullScreen
                root.wake()
                focusGrab.restart()
            } else {
                if (!root.wasFullscreen && !root.preKiosk
                        && w !== null && w.visibility === Window.FullScreen)
                    w.visibility = Window.Windowed
                // Focus hygiene (measured 2026-08-02 over RFB): when the
                // overlay hides while its search field holds activeFocus, Qt
                // leaves activeFocus STRANDED on the now-invisible TextInput
                // — the AppShell dispatcher's text-input gate would read
                // that dead field as focused and swallow every hotkey.
                // Hand focus to the SHELL ROOT (K8, 2026-08-03 hotkeys-port
                // §1.4.2 — NOT contentItem): the shell root is the
                // dispatcher's fallback focus item, so the pipeline keeps
                // receiving keys with no click in between.
                var p = root
                while (p.parent) {
                    if (p.parent.isQbzShellRoot === true) {
                        p.parent.forceActiveFocus()
                        break
                    }
                    p = p.parent
                }
            }
        }
    }

    // 30ms focus-grab after mount (:1259-1267) — focusing synchronously
    // inside the open edge races the visibility transition.
    Timer {
        id: focusGrab
        interval: 30
        repeat: false
        onTriggered: root.forceActiveFocus()
    }

    // --- Layer 1: root surface ---------------------------------------------
    Rectangle {
        anchors.fill: parent
        color: "#0a0a0b"
    }

    // --- Layer 1b: the shader scene (block A1, :1293-1300) ------------------
    // Bottom-most VISUAL layer, ABOVE the root surface, below everything
    // else. Self-gated: it renders and ticks only while a scene is active
    // (scene > 0 && open && available — see the file's header). The
    // atmosphere below and every FOCUS/SPLIT panel gate themselves off on
    // `scene === 0`, so the scene owns the background (parity :1313-1405).
    ShaderSceneLayer {
        anchors.fill: parent
        visible: QbzShaderScene.scene > 0
    }

    // --- Layer 2: the atmosphere underlay (§5.1, :1313-1321) ----------------
    // The underlay WHILE NO SCENE IS ACTIVE (the Slint shader-mode==0 gate,
    // :1313). Source = the host-generated 128x128 atmosphere PNG, fallback
    // the plain cover; dim 0.15. `animated` binds npPlaying (:1321); the
    // && open arm stops the drift clock while the overlay is closed (the
    // Slint view is UNMOUNTED then — same zero cost). Spectrum/WaveBed paint
    // opaque #000 over it — intended (§5.1).
    ImmersiveAtmosphere {
        anchors.fill: parent
        visible: QbzShaderScene.scene === 0
            && !(QbzImmersive.viewMode === 0
                 && (QbzImmersive.mode === 7 || QbzImmersive.mode === 8
                     || QbzImmersive.mode === 9))
        source: QbzImmersive.atmosphereUrl
        fallbackSource: QbzPlayer.npArtworkPath
        animated: visible && QbzPlayer.npPlaying && QbzImmersive.open
        dim: 0.15
    }

    // --- Layer 2b: the click-blocker (§5.1, load-bearing) ------------------
    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.AllButtons
        hoverEnabled: true
        // C1 fullscreen integration (contract 03 §5, divergence D2 — not a
        // Slint parity item): blank the cursor when the chrome has
        // auto-hidden in FullScreen; wake() restores it. Windowed mode keeps
        // the cursor always. Note: panels with their own cursor-less
        // MouseAreas above this layer still show the default arrow — the
        // blank covers the bare-atmosphere regions, which is where a
        // stationary cursor rests while watching fullscreen.
        cursorShape: (root.Window.window !== null
                      && root.Window.window.visibility === Window.FullScreen
                      && !root.chromeVisible) ? Qt.BlankCursor : Qt.ArrowCursor
        // A press on the backdrop WAKES the chrome: touch panels have no
        // hover, so this is the only way a kiosk finger brings the exit back.
        onPressed: function (mouse) { mouse.accepted = true; root.wake() }
        onReleased: function (mouse) { mouse.accepted = true }
        onClicked: function (mouse) { mouse.accepted = true }
        onDoubleClicked: function (mouse) { mouse.accepted = true }
        onWheel: function (wheel) { wheel.accepted = true }
    }

    // --- §5.3 auto-hide / wake ----------------------------------------------
    // hideTimer 6000ms (:1271); hides chrome only when !immSearchOpen &&
    // !pointerInChrome; wake on any REAL mouse move (delta guard below);
    // 300ms fades on the chrome itself.
    //
    // 2026-08-15 C1 (contract 03 §4): pointerInChrome is the union of the
    // REAL chrome rects (header ∪ player bar ∪ song card), not the Slint
    // y-only bands (:1196-1197). The bands kept the chrome awake over dead
    // strips (the 24px side margins, the 18px above the bar) which read as
    // "mouse-sensitive in some zones", and spanned the full width.
    property bool chromeVisible: true
    readonly property bool pointerInWindow: rootHover.hovered
    readonly property bool pointerInChrome: rootHover.hovered
        && (pointInChromeItem(header) || pointInChromeItem(playerBar)
            || pointInChromeItem(songCard))

    function pointInChromeItem(item) {
        if (!item.visible)
            return false
        var p = item.mapFromItem(root, rootHover.point.position.x,
                                 rootHover.point.position.y)
        return p.x >= 0 && p.y >= 0 && p.x <= item.width && p.y <= item.height
    }

    function wake() {
        chromeVisible = true
        hideTimer.restart()
    }

    // C1 macOS traffic lights (contract 03 §6): NOTHING TO CLEAR ANY MORE.
    //
    // The band used to be inset to their right (`leftMargin: 24 + inset`,
    // the shell HeaderBar's rule) because the view trigger sat at the band's
    // left edge — x ~ 7-85, exactly where the lights float. The trigger moved
    // to the RIGHT of the band, beside the window-controls capsule, so the
    // left half is empty and the lights have that corner to themselves.
    //
    // Dropping the whole row below the lights was tried and reverted: it moved
    // the search field and the capsule down with it to solve a problem only
    // the trigger had.
    // Last hover position that WOKE the chrome. Load-bearing delta guard:
    // Qt re-delivers HoverMove at the unchanged position whenever the scene
    // under a stationary cursor updates (constant on any animated surface —
    // and under the VNC platform it storms every frame, measured 2026-08-02).
    // Without the guard each re-delivery restarts hideTimer and the chrome
    // never auto-hides. "Wake on any mouse move" (:1283-1286) means an actual
    // position CHANGE.
    property point lastHoverPos: Qt.point(-1, -1)

    HoverHandler {
        id: rootHover
        acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
        onPointChanged: {
            var p = rootHover.point.position
            if (Math.abs(p.x - root.lastHoverPos.x) > 0.5
                    || Math.abs(p.y - root.lastHoverPos.y) > 0.5) {
                root.lastHoverPos = p
                root.wake()
            }
        }
    }

    Timer {
        id: hideTimer
        interval: 6000
        repeat: false
        running: root.visible
        onTriggered: {
            if (!QbzImmersive.immSearchOpen && !root.pointerInChrome)
                root.chromeVisible = false
            else
                restart() // suppressed — re-check after another interval
        }
    }

    // --- Layer 3: the FOCUS panels (B3 + B4) --------------------------------
    // FOCUS: one panel per mode, gated viewMode==0 && mode==N, full-viewport
    // — each panel carries its own Slint-internal reserves (pad-top 52/70,
    // the 132px player clearance; §6.1/§6.2/§6.6). Block A1 adds the
    // `scene === 0` gate to every panel (parity :1313-1405 — a shader scene
    // REPLACES the panels; chrome stays on top).
    AlbumReactivePanel {
        anchors.fill: parent
        visible: QbzShaderScene.scene === 0
            && QbzImmersive.viewMode === 0 && QbzImmersive.mode === 0
    }
    StaticPanel {
        anchors.fill: parent
        visible: QbzShaderScene.scene === 0
            && QbzImmersive.viewMode === 0 && QbzImmersive.mode === 1
    }
    // LOADER-GATED, and this one is not a nicety — it is the fix for a
    // 28-second frozen UI at startup.
    //
    // This overlay is mounted UNCONDITIONALLY by AppShell (see its comment
    // there: while closed it is "an invisible, non-interactive Item"), and
    // `visible: false` stops an item from PAINTING, never from being BUILT.
    // CoverflowPanel's model is the flat whole-queue document
    // (`QbzQueue.coverflowJson` — unlike the queue PANEL's, it is not
    // paginated), and it runs TWO Repeaters over it: the forward list and a
    // reversed copy. So every launch built two CoverCards per queue entry,
    // for a panel nobody had opened.
    //
    // Measured 2026-08-17 on a real session with 1935 persisted queue rows:
    // ~4000 CoverCards on the UI thread at boot. The app came up frozen for
    // ~28 s at 120% CPU — deterministic, identical across every build, and
    // "fixed" by moving ~/.local/share/qbz aside, which is what made it look
    // like corrupted state rather than a queue the UI could not carry. The
    // stacks showed the QML engine forcing GC while allocating strings.
    //
    // `active` repeats the visibility expression on purpose: the panel must
    // not exist while it is not the immersive mode being shown.
    Loader {
        anchors.fill: parent
        active: QbzShaderScene.scene === 0
            && QbzImmersive.viewMode === 0 && QbzImmersive.mode === 2
        visible: active
        sourceComponent: CoverflowPanel { }
    }
    SpectrumPanel {
        anchors.fill: parent
        visible: QbzShaderScene.scene === 0
            && QbzImmersive.viewMode === 0 && QbzImmersive.mode === 3
    }
    WaveBedPanel {
        anchors.fill: parent
        visible: QbzShaderScene.scene === 0
            && QbzImmersive.viewMode === 0 && QbzImmersive.mode === 6
    }
    ReactiveRingsPanel {
        anchors.fill: parent
        visible: QbzShaderScene.scene === 0
            && QbzImmersive.viewMode === 0 && QbzImmersive.mode === 9
    }
    Loader {
        anchors.fill: parent
        active: QbzShaderScene.scene === 0
            && QbzShell.gpuTier
            && QbzImmersive.viewMode === 0 && QbzImmersive.mode === 7
        visible: active
        sourceComponent: Item {
            // A single host-generated blurred cover, without the animated
            // four-layer atmosphere. The dark veil makes one album-derived
            // high-luminance trace colour hold over the whole viewport.
            Rectangle { anchors.fill: parent; color: "#090909" }
            Image {
                anchors.fill: parent
                source: QbzImmersive.atmosphereUrl
                fillMode: Image.PreserveAspectCrop
                asynchronous: true
                smooth: true
                mipmap: true
                opacity: status === Image.Ready ? 0.78 : 0.0
            }
            Rectangle { anchors.fill: parent; color: "#66000000" }
            ScopePanel {
                width: Math.max(1, Math.min(parent.width - 96, parent.height - 196))
                height: width
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.bottom: parent.bottom
                anchors.bottomMargin: 132
                scopeMode: 0
                albumContrast: true
            }
        }
    }
    Loader {
        anchors.fill: parent
        active: QbzShaderScene.scene === 0
            && QbzShell.gpuTier
            && QbzImmersive.viewMode === 0 && QbzImmersive.mode === 8
        visible: active
        sourceComponent: Item {
            Rectangle { anchors.fill: parent; color: "#000000" }
            ScopePanel {
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                anchors.leftMargin: 48
                anchors.rightMargin: 48
                anchors.topMargin: 96
                anchors.bottomMargin: 132
                scopeMode: 1
                albumContrast: true
            }
        }
    }
    // FOCUS Lyrics (mode 4, B4 §6.6): the one-line variant. It shares the
    // immersive LyricsSyncEngine with the split lyrics mount (trap 22 — the
    // gate covers BOTH lyric surfaces and nothing else). No entry load: Qt
    // fetches lyrics automatically per track (§5.5).
    LyricsFocusPanel {
        anchors.fill: parent
        visible: QbzShaderScene.scene === 0
            && QbzImmersive.viewMode === 0 && QbzImmersive.mode === 4
        lines: root.immLyricsLines
        synced: root.immLyricsSynced
        status: root.immLyricsStatus
        sync: immLyricsEngine
    }
    // FOCUS Queue (mode 5, B4 §6.5): the same QueueTabsPanel as the split
    // placement, full-screen (centered: false -> min(vw-96, 720) column).
    // Loader-gated for the reason spelled out on CoverflowPanel above: it
    // reads the same unpaginated whole-queue document.
    Loader {
        anchors.fill: parent
        active: QbzShaderScene.scene === 0
            && QbzImmersive.viewMode === 0 && QbzImmersive.mode === 5
        visible: active
        sourceComponent: QueueTabsPanel { centered: false }
    }

    // --- The shared immersive lyrics engine + document (§6.6, trap 22) ------
    // ONE LyricsSyncEngine for all immersive lyric surfaces (FOCUS mode 4,
    // legacy SPLIT panel 0 and cinematic SPLIT panel 5); the gate extends
    // the §6.6 formula
    // (lyrics_sync.rs:214-217 parity). The QbzLyrics doc fields are parsed
    // once here and handed to both mounts.
    readonly property var immLyricsDoc: {
        try {
            return JSON.parse(QbzLyrics.docJson)
        } catch (e) {
            return ({})
        }
    }
    readonly property var immLyricsLines: root.immLyricsDoc.lines !== undefined
        && root.immLyricsDoc.lines !== null ? root.immLyricsDoc.lines : []
    readonly property bool immLyricsSynced: root.immLyricsDoc.synced === true
    readonly property int immLyricsStatus: root.immLyricsDoc.status !== undefined
        ? root.immLyricsDoc.status : 0

    LyricsSyncEngine {
        id: immLyricsEngine
        gateOpen: QbzImmersive.open
            && QbzShaderScene.scene === 0 // A1: the scenes replace the panels
            && ((QbzImmersive.viewMode === 0 && QbzImmersive.mode === 4)
                || (QbzImmersive.viewMode === 1
                    && (QbzImmersive.splitPanel === 0
                        || QbzImmersive.splitPanel === 5)))
    }
    // Every doc commit resets the ladder and re-lands on the right line (the
    // LyricsPanel.qml:60-66 idiom — the Slint kick() on commit/panel open).
    Connections {
        target: QbzLyrics
        function onDocJsonChanged() {
            immLyricsEngine.reset()
            immLyricsEngine.kick()
        }
    }

    // --- Layer 3 (viewMode==1): the SPLIT layout (§5.6/D1) -------------------
    // HorizontalLayout padding-top 56 bottom 132 sides 56 spacing 48
    // (:1423-1427); LEFT column EXPLICIT width (root.width - 160)/2 — the
    // shipped Slint is a true 50/50 (:1435,:1516; the stale "55/45"/"11:9"
    // comments at :1406-1410,:1503-1510 are wrong, D1). Artwork + the B3
    // ImmersiveTrackMeta LEFT; the active data panel RIGHT, on a pad-20
    // clipped plate with NO glass backing (owner removed it,
    // :1484-1487,:1517-1518).
    Item {
        id: splitRoot
        anchors.fill: parent
        // A1: a shader scene replaces the SPLIT layout too (:1416 parity).
        visible: QbzShaderScene.scene === 0 && QbzImmersive.viewMode === 1
            && QbzImmersive.splitPanel >= 0 && QbzImmersive.splitPanel <= 3
        // D2: re-request the large art when the SPLIT layout becomes visible
        // (leftCol's own request is gated on this visibility).
        onVisibleChanged: if (visible) leftCol.requestArtSize()

        // col-h: the column height below the header/player clearance
        // (:1415). colW: the explicit half (:1435).
        readonly property real colW: (root.width - 160) / 2
        readonly property real colH: root.height - 56 - 132

        Row {
            anchors.fill: parent
            anchors.topMargin: 56
            anchors.bottomMargin: 132
            anchors.leftMargin: 56
            anchors.rightMargin: 56
            spacing: 48

            // LEFT (50%): artwork + the shared metadata block, centered in
            // its half (:1433-1501), spacing 28.
            Item {
                id: leftCol
                width: splitRoot.colW
                height: parent.height

                // D2 (contract 04 §4): size-aware large art — prefer the
                // size-resolved large feed, fall back to the small one while
                // it resolves (the StaticPanel/AlbumReactivePanel pattern).
                readonly property string artSource: QbzPlayer.npArtworkPathLarge !== ""
                    ? QbzPlayer.npArtworkPathLarge : QbzPlayer.npArtworkPath
                // The artSize slot expression WITHOUT the native cap — the
                // request must reflect the SLOT; the probe cap below stays
                // the final upscale safety. Bucketed in Rust (one re-resolve
                // per variant tier crossed, none per pixel).
                readonly property real artSlot: Math.max(240,
                    Math.min(splitRoot.colW, splitRoot.colH - 200))
                function requestArtSize() {
                    if (splitRoot.visible)
                        QbzPlayer.requestNpArtworkSize(Math.round(leftCol.artSlot))
                }
                onArtSlotChanged: requestArtSize()
                Component.onCompleted: requestArtSize()

                // Native source resolution (physical px) of the active
                // cover, so the art is never UPSCALED beyond its own size
                // (:1449-1452) — the AlbumReactivePanel.qml artProbe idiom
                // (a hidden probe Image on the same file:// cache path; the
                // shared image cache makes the second decode free).
                Image {
                    id: splitArtProbe
                    visible: false
                    source: leftCol.artSource
                }
                readonly property real srcNative: splitArtProbe.status === Image.Ready
                    ? splitArtProbe.implicitWidth : 0
                // max(240, min(colW, colH-200, native)) (:1453-1455).
                readonly property real artSize: Math.max(240,
                    Math.min(splitRoot.colW, splitRoot.colH - 200, leftCol.srcNative))

                Column {
                    anchors.centerIn: parent
                    width: parent.width
                    spacing: 28

                    // Artwork square, radius 12, shadow 40/#00000099/12
                    // (:1458-1476).
                    Item {
                        width: parent.width
                        height: leftCol.artSize

                        // Static drop shadow, offset-y 12 — the
                        // CoverflowPanel MultiEffect idiom (blur 40 of
                        // blurMax 64); no shaders -> a plain offset rect.
                        Rectangle {
                            x: (parent.width - leftCol.artSize) / 2
                            y: 12
                            width: leftCol.artSize
                            height: leftCol.artSize
                            radius: 12
                            color: "#99000000"
                            layer.enabled: !root._noShaders
                            layer.effect: MultiEffect {
                                blurEnabled: true
                                blurMax: 64
                                blur: 0.625 // 40/64
                            }
                        }
                        // Neutral placeholder while the cover is unresolved.
                        Rectangle {
                            visible: QbzPlayer.npArtworkPath === ""
                            x: (parent.width - leftCol.artSize) / 2
                            width: leftCol.artSize
                            height: leftCol.artSize
                            radius: 12
                            color: "#14ffffff"
                        }
                        RoundedImage {
                            x: (parent.width - leftCol.artSize) / 2
                            width: leftCol.artSize
                            height: leftCol.artSize
                            visible: QbzPlayer.npArtworkPath !== ""
                            radius: 12
                            source: leftCol.artSource
                        }
                    }

                    // The shared metadata block (:1484-1500). The Slint
                    // meta-plate is padding-only (the glass fill was removed
                    // by the owner) — 22px side gutters so the elided rows
                    // have a real width to truncate against.
                    ImmersiveTrackMeta {
                        anchors.horizontalCenter: parent.horizontalCenter
                        width: parent.width - 44
                    }
                }
            }

            // RIGHT (50%): the active split panel on a clipped plate, NO
            // glass backing (:1511-1596). The panel's inner box is inset by
            // the 20px pad on all sides (:1522-1527).
            Item {
                width: splitRoot.colW
                height: parent.height
                clip: true

                Item {
                    anchors.fill: parent
                    anchors.margins: 20
                    clip: true

                    // splitPanel 0: the shared lyrics lines view with the
                    // §6.6 immersive knobs EXACT (:1528-1549). The sizes are
                    // computed from the WINDOW width, not the column
                    // (:1533-1537). Slint's show-attribution: false has NO
                    // Qt twin (LyricsLinesView.qml has no such property and
                    // no attribution row — D13, nothing to set).
                    LyricsLinesView {
                        anchors.fill: parent
                        visible: QbzImmersive.splitPanel === 0
                        lines: root.immLyricsLines
                        synced: root.immLyricsSynced
                        showTranslation: QbzLyrics.showTranslation
                        sync: immLyricsEngine
                        sidePadding: 8
                        lineGap: Math.round(Math.max(16,
                            Math.min(root.width * 0.014, 26)))
                        sizeInactive: Math.round(Math.max(24,
                            Math.min(root.width * 0.018, 34)))
                        sizeActive: Math.round(Math.max(28,
                            Math.min(root.width * 0.022, 42)))
                        dimmingMode: QbzLyrics.dimmingMode
                        autoFollow: true
                        uppercase: QbzLyrics.uppercase
                        fontIndex: QbzLyrics.fontIndex
                        // Solid white active line in immersive
                        // (data-panels.md §2, :1548). The unsung part of the
                        // active line drops to 35% white so the karaoke fill
                        // is actually visible (2026-08-31 visual cleanup).
                        activeColor: "#ffffff"
                        inactiveColor: "#59ffffff"
                        onSeekRequested: function (timeMs) {
                            if (QbzPlayer.npDurationSecs > 0)
                                QbzPlayer.seek(timeMs / 1000 / QbzPlayer.npDurationSecs)
                        }
                    }

                    // splitPanel 1: the inline Track Info credits panel
                    // (§6.3). Entry load at mount + reload on track change
                    // below (:1557-1560, §5.5).
                    TrackInfoPanel {
                        anchors.fill: parent
                        visible: QbzImmersive.splitPanel === 1
                        onVisibleChanged: if (visible)
                            QbzAlbum.openTrackInfo(QbzPlayer.npTrackId)
                    }

                    // splitPanel 2: the live Suggestions panel (§6.4). Entry
                    // load at mount + reload on track change below
                    // (:1574-1577, §5.5).
                    // Loader-gated like the queue panels: a closed panel has
                    // no business being built. Mounting IS becoming visible
                    // now, so the load hook is a plain Component.onCompleted
                    // — the `onVisibleChanged` pair existed only because the
                    // panel was built eagerly and its mount meant nothing.
                    Loader {
                        anchors.fill: parent
                        active: QbzImmersive.splitPanel === 2
                        visible: active
                        sourceComponent: SuggestionsPanel {
                            Component.onCompleted: QbzSuggestions.load(QbzPlayer.npTrackId)
                        }
                    }

                    // splitPanel 3: the Queue + History panel, centered in
                    // the right half (§6.5; same component as the focus
                    // mount, centered: true fills the slot). The panel
                    // self-fires queuePanelOpened() when it becomes visible
                    // (:253-255).
                    // Loader-gated like the focus mount above — same
                    // component, same whole-queue document.
                    Loader {
                        anchors.fill: parent
                        active: QbzImmersive.splitPanel === 3
                        visible: active
                        sourceComponent: QueueTabsPanel { centered: true }
                    }
                }
            }
        }
    }

    // The two cinematic SPLIT variants replace the legacy 50/50 composition
    // with the supplied full-viewport artwork stage. One shared component
    // guarantees that the background/card/waveform stay pixel-identical; only
    // the card's right-hand content changes between metadata+seek and lyrics.
    Loader {
        anchors.fill: parent
        active: QbzShaderScene.scene === 0
            && QbzImmersive.viewMode === 1
            && (QbzImmersive.splitPanel === 4
                || QbzImmersive.splitPanel === 5)
        visible: active
        sourceComponent: ReactiveSplitPanel {
            lyricsMode: QbzImmersive.splitPanel === 5
            lines: root.immLyricsLines
            synced: root.immLyricsSynced
            status: root.immLyricsStatus
            sync: immLyricsEngine
        }
    }

    // --- Layer 4: ImmersiveSongCard (§5.1, :1602-1605) ----------------------
    // Keep the compact bottom-right metadata over shader backgrounds,
    // Wave Bed, full-screen Lyrics and the two scope views. The lyrics surface
    // deliberately keeps one line at a time, so the card supplies the
    // otherwise absent track context without competing with the active line.
    // shader-mode was constant 0 so the first arm was dead (trap 19); A1
    // makes it real. NON-interactive; does NOT fade with the auto-hide
    // chrome (no opacity binding here — that is deliberate).
    ImmersiveSongCard {
        id: songCard
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.rightMargin: 24
        anchors.bottomMargin: 24
        visible: (QbzShaderScene.scene > 0
                  || (QbzImmersive.viewMode === 0
                      && (QbzImmersive.mode === 4 || QbzImmersive.mode === 6
                          || QbzImmersive.mode === 7 || QbzImmersive.mode === 8
                          || QbzImmersive.mode === 9)))
            && QbzPlayer.npHasTrack
    }

    // §5.5: TrackInfo + Suggestions RELOAD on track change while their panel
    // is mounted (:1557-1560, :1574-1577 — the Slint `changed live-track-id`
    // watchers; the dedup guard lives in the Rust loaders).
    Connections {
        target: QbzPlayer
        function onNpTrackIdChanged() {
            if (!root.visible || QbzImmersive.viewMode !== 1
                    || QbzShaderScene.scene !== 0) // A1: split panels unmounted under a scene
                return
            if (QbzImmersive.splitPanel === 1)
                QbzAlbum.openTrackInfo(QbzPlayer.npTrackId)
            else if (QbzImmersive.splitPanel === 2)
                QbzSuggestions.load(QbzPlayer.npTrackId)
        }
    }

    // No-shader renderer detection, verbatim from SpectrumBand.qml (the
    // SPLIT artwork drop shadow is a MultiEffect blur; no shaders -> plain
    // offset rect).
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null

    // --- Exit paths 3 (§5.4, §7) -------------------------------------------
    // Escape on the root: dismisses the search cortinilla FIRST when open
    // (§3.4), exits immersive otherwise. The search field's own Escape
    // declines and propagates here (ImmersiveHeader.qml comment).
    Keys.onEscapePressed: function (event) {
        if (QbzImmersive.immSearchOpen)
            QbzImmersive.dismissSearch()
        else
            QbzImmersive.open = false
        event.accepted = true
    }
    // Block A1: `g` cycles the SHIPPED shader scenes (0→1→2→3→4→5→7→0 with
    // Plasma A2, Ribbon A3 and Line Bed A4 shipped — the ring lives in
    // QbzShaderScene.cycleScene; Tunnel Flow 8 is menu-only, NOT in the
    // ring). No-op on tiers where the scenes can't run
    // (QbzShell.shaderScenesAvailable — the same flag the FOCUS menu rows
    // check), and the textInputActive gate keeps a 'g' typed in the search
    // field from cycling (§5.4). wake() on cycle, like Slint. The key is
    // ACCEPTED even when unavailable — Slint's `return accept` — so it never
    // falls through to another handler.
    Keys.onPressed: function (event) {
        if (event.key === Qt.Key_G && !root.textInputActive) {
            if (QbzShell.shaderScenesAvailable) {
                QbzShaderScene.cycleScene()
                root.wake()
            }
            event.accepted = true
        }
    }
    // The seek arrows are GONE (2026-08-03 hotkeys-port §1.3): ±5s/±10s now
    // fire as the REBINDABLE focus.seekForward/Back/Long actions through the
    // AppShell dispatcher, with the Context::Immersive gate and the central
    // text-input gate (QbzHotkeys.keyPressed, same §1.3 math). Kept here per
    // the same section: the Escape handler above, `textInputActive`, and the
    // cortinilla arrows (ImmersiveHeader).

    // --- Layer 5: the player bar (§5.7, :1607-1616) -------------------------
    // Centered, y = height-90-24; fades with the auto-hide chrome on the
    // SAME binding as the header (300ms), inert while hidden.
    // C1: hit-enabled follows the STATE (`chromeVisible`), not the animated
    // opacity — gating on `opacity > 0` left the bar clickable mid-fade and
    // re-triggered the Behavior on wake/hide races (contract 03 §2 item 4).
    ImmersivePlayerBar {
        id: playerBar
        anchors.horizontalCenter: parent.horizontalCenter
        y: root.height - 90 - 24
        viewportWidth: root.width
        opacity: root.chromeVisible ? 1 : 0
        Behavior on opacity { NumberAnimation { duration: 300 } }
        enabled: root.chromeVisible
        onWakeRequested: root.wake()
    }

    // --- Layer 6: the header band (§5.2) --------------------------------------
    ImmersiveHeader {
        id: header
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.leftMargin: 24
        anchors.rightMargin: 24
        anchors.topMargin: 16
        height: 36
        opacity: root.chromeVisible ? 1 : 0
        Behavior on opacity { NumberAnimation { duration: 300 } }
        enabled: root.chromeVisible
    }

    // --- Layer 7: the search cortinilla (§3.4, :1695-1705) -------------------
    // TOPMOST: above the header band and the player bar. Self-gated on
    // QbzImmersive.immSearchOpen && query >= 2 chars; NOT chrome-faded (its
    // open flag is exactly what suppresses the auto-hide, trap 10). The
    // accent follows the atmosphere so the keyboard-active row matches.
    ImmersiveSearchCortinilla {
        anchors.fill: parent
        accent: QbzShell.ambientAccent
    }
    // Appliance exit. It FOLLOWS the auto-hiding chrome now (owner feedback
    // 2026-09-07: an always-on 64px white-bordered box was too loud on the
    // art) — same `chromeVisible` state as the header and the player bar,
    // hit-enabled only while shown, and it comes back on any pointer move,
    // key or tap (the backdrop press below wakes the chrome for touch). The
    // close path is unchanged: it retains fullscreen and restores shell
    // focus via preKiosk. Quiet chrome: translucent fill, hairline border.
    Rectangle {
        visible: root.preKiosk
        opacity: root.chromeVisible ? 1 : 0
        enabled: root.chromeVisible
        // BOTTOM-right (owner feedback 2026-09-07): at the top it collided
        // with the immersive's own window controls. The player bar is
        // horizontally centred at y=height-114, so the right corner is clear;
        // this sits in it, clear of both the controls and the bar.
        anchors.bottom: parent.bottom
        anchors.right: parent.right
        anchors.bottomMargin: 24
        anchors.rightMargin: 16
        width: 52
        height: 52
        radius: 8
        color: "#99171717"
        border.color: "#59ffffff"
        border.width: 1
        z: 3001
        activeFocusOnTab: visible && root.chromeVisible
        Accessible.role: Accessible.Button
        Accessible.name: QbzSession.tr("Close", QbzSession.trRev)
        Accessible.onPressAction: QbzImmersive.open = false
        QbzIcon { anchors.centerIn: parent; width: 24; height: 24; name: "x"; tintName: "primary" }
        Keys.onPressed: function (event) {
            if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_Space) {
                QbzImmersive.open = false
                event.accepted = true
            }
        }
        MouseArea { anchors.fill: parent; onClicked: QbzImmersive.open = false }
    }

}
