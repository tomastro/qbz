#!/usr/bin/env node
// Regressions for the 2026-09-07 kiosk feedback round:
//   - the kiosk's client-side funnel predicates are the DESKTOP's (the two
//     ports are run side by side over the same fixtures);
//   - the chrome/transport decisions hold statically: the back bar drags,
//     Connect + Cast live in the back bar, the transport bar has no hidden
//     menu, Volume is always visible, Desktop mode has a button, the Now
//     Playing transport is one centred row, and Local Library exposes the
//     Open entry point and the funnel.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const qml = (rel) => fs.readFileSync(new URL(`../crates/qbz-qt/qml/${rel}`, import.meta.url), 'utf8');
const desktop = qml('views/LocalLibraryView.qml');
const kioskHost = qml('kiosk/KioskLocalLibrary.qml');
const shell = qml('shell/KioskShell.qml');
const bar = qml('kiosk/KioskNowPlayingBar.qml');
const nowPlaying = qml('kiosk/KioskNowPlaying.qml');
const albumsTab = qml('kiosk/KioskLocalAlbumsTab.qml');
const flyout = qml('shell/QconnectFlyout.qml');

/// The full `function name(...) {...}` text, brace-matched.
function fn(source, name) {
    const at = source.indexOf(`function ${name}(`);
    assert.ok(at >= 0, `function ${name} in source`);
    const start = source.indexOf('{', at);
    for (let depth = 1, i = start + 1; i < source.length; i++) {
        if (source[i] === '{') depth++;
        if (source[i] === '}' && --depth === 0) return source.slice(at, i + 1);
    }
    throw Error(`unclosed ${name}`);
}

// ---- funnel parity ----------------------------------------------------------
// Cross-context arrays carry foreign prototypes, so results are compared
// through JSON rather than as live objects.
const run = (ctx, code) => JSON.parse(vm.runInContext(`JSON.stringify(${code})`, ctx));
const desktopCtx = vm.createContext({ root: { albumFavorite: (r) => r.isFavorite === true } });
vm.runInContext([fn(desktop, 'sourceBucket'), fn(desktop, 'applyFilter'), fn(desktop, 'artistMatchesFilter')].join('\n'), desktopCtx);
const kioskCtx = vm.createContext({ root: {} });
vm.runInContext([fn(kioskHost, 'sourceBucket'), fn(kioskHost, 'applyFilter'), fn(kioskHost, 'artistMatchesFilter'),
    'root.sourceBucket = sourceBucket; root.applyFilter = applyFilter; root.artistMatchesFilter = artistMatchesFilter;'].join('\n'), kioskCtx);

const albums = [
    { id: 'a', isFavorite: true, mediaVariants: [{ qualityTier: 'hires', format: 'flac', source: 'user' }] },
    { id: 'b', isFavorite: false, mediaVariants: [{ qualityTier: 'dsd', format: 'dsf', source: 'qobuz_purchase' }, { qualityTier: 'cd', format: 'flac', source: 'user' }] },
    { id: 'c', isFavorite: false, qualityTier: 'mp3', format: 'mp3', sources: ['navidrome'] },
    { id: 'd', isFavorite: true, qualityTier: 'cd', format: 'alac', source: 'plex' },
    { id: 'e', isFavorite: false, mediaVariants: [{ qualityTier: 'max', format: 'wav', source: '' }] },
];
const artists = [
    { name: 'A', qualityTiers: ['hires', 'cd'], formats: ['flac'], sources: ['user'] },
    { name: 'B', qualityTiers: ['dsd'], formats: ['dsf'], sources: ['qobuz_download'] },
    { name: 'C', qualityTiers: ['mp3'], formats: ['mp3'], sources: ['gonic'] },
    { name: 'D', qualityTiers: [], formats: [], sources: [] },
];
const funnels = [
    {}, { hires: true }, { dsd: true }, { flac: true }, { flac: true, dsd: true }, { other: true },
    { plex: true }, { subsonic: true }, { offline: true }, { local: true }, { favorite: true },
    { favorite: true, cd: true }, { lossy: true, mp3: true }, { wav: true, hires: true }, { cd: true, local: true },
];
for (const f of funnels) {
    const want = run(desktopCtx, `applyFilter(${JSON.stringify(albums)}, ${JSON.stringify(f)}).map(r => r.id)`);
    const got = run(kioskCtx, `root.applyFilter(${JSON.stringify(albums)}, ${JSON.stringify(f)}).map(r => r.id)`);
    assert.deepEqual(got, want, `applyFilter parity for ${JSON.stringify(f)}`);
    const wantArtists = run(desktopCtx, `${JSON.stringify(artists)}.filter(a => artistMatchesFilter(a, ${JSON.stringify(f)})).map(a => a.name)`);
    const gotArtists = run(kioskCtx, `${JSON.stringify(artists)}.filter(a => root.artistMatchesFilter(a, ${JSON.stringify(f)})).map(a => a.name)`);
    assert.deepEqual(gotArtists, wantArtists, `artistMatchesFilter parity for ${JSON.stringify(f)}`);
}
// The fixtures actually discriminate: not every funnel keeps every row.
assert.deepEqual(run(kioskCtx, `root.applyFilter(${JSON.stringify(albums)}, {"hires":true}).map(r => r.id)`), ['a', 'e']);
assert.deepEqual(run(kioskCtx, `root.applyFilter(${JSON.stringify(albums)}, {"favorite":true,"cd":true}).map(r => r.id)`), ['d']);
assert.deepEqual(run(kioskCtx, `root.applyFilter(${JSON.stringify(albums)}, {"subsonic":true}).map(r => r.id)`), ['c']);

// ---- the funnel is wired, not just computed --------------------------------
assert.match(kioskHost, /property var albumsFilter: root\.parseFilter\(QbzLocal\.albumsFilter\)/, 'seeded from the bridge');
assert.match(kioskHost, /QbzLocal\.setAlbumsFilterJson\(/, 'persists through the desktop writer');
assert.match(kioskHost, /QbzLocal\.tracksSetFilterJson\(JSON\.stringify\(root\.commonFilter\)\)/, 'Tracks installs the shared funnel');
assert.match(kioskHost, /function onAlbumsFilterChanged\(\)/, 'a later republish wins');
assert.match(albumsTab, /albumsNativeReset\("", "artist-asc", "off",\s*root\.view \? root\.view\.filterJson : "\{\}"/, 'native Albums descriptor carries the funnel');
assert.match(albumsTab, /function onFilterJsonChanged\(\)/, 'a chip toggle re-queries native Albums');
assert.match(albumsTab, /!\(root\.view && root\.view\.favoriteOnly\)/, 'Favorites only forces the legacy Albums reader');
assert.match(kioskHost, /root\.activeTab === "albums" && root\.favoriteOnly\)\n\s+QbzLocal\.loadTab\("albums-legacy"\)/, 'host loads the legacy document under Favorites only');
for (const key of ['dsd', 'hires', 'cd', 'lossy', 'flac', 'alac', 'ape', 'wav', 'mp3', 'aac', 'other', 'local', 'offline', 'plex', 'jellyfin', 'subsonic', 'favorite'])
    assert.match(kioskHost, new RegExp(`toggleFilter\\("${key}"\\)`), `chip for ${key}`);
assert.match(kioskHost, /onClicked: root\.clearFilter\(\)/, 'Clear');

// ---- Open (ephemeral) entry point ------------------------------------------
for (const call of ['QbzLocal.ephemeralOpen()', 'QbzLocal.ephemeralOpenCd()', 'QbzLocal.ephemeralOpenSacd()'])
    assert.ok(kioskHost.includes(call), `${call} reachable from the kiosk`);
assert.match(kioskHost, /"Open folder…"[\s\S]*"Open audio CD"[\s\S]*"Open SACD image…"/, 'the desktop Open menu rows, verbatim');

// ---- chrome: drag + session cluster ----------------------------------------
assert.match(shell, /enabled: !QbzShell\.systemTitleBar[\s\S]*root\.hostWindow\.startSystemMove\(\)/, 'the back bar is the drag surface');
assert.match(shell, /connectFlyout\.openBelowRight\(kioskQconnectBtn\)/, 'Connect opens from the back bar');
assert.match(shell, /name: "cast"[\s\S]{0,400}onClicked: QbzCast\.openPicker\(\)/, 'Cast opens from the back bar');
assert.match(shell, /QconnectFlyout \{ id: connectFlyout \}[\s\S]*CastPicker \{ \}/, 'flyout + picker are shell-level');
assert.match(flyout, /function openBelowRight\(sourceItem\)/, 'top-bar placement exists');
assert.ok(shell.indexOf('id: sessionCluster') < shell.indexOf('id: windowControls'), 'cluster is declared before the window controls');
assert.match(shell, /- sessionCluster\.width - 6/, 'the search row yields the cluster width');

// ---- transport bar: no hidden menu, Volume + Desktop mode always visible ---
assert.ok(!bar.includes('moreMenu') && !bar.includes('"ellipsis"'), 'the hidden more-menu is gone');
assert.ok(!bar.includes('QconnectFlyout') && !bar.includes('CastPicker'), 'Connect/Cast no longer live in the bar');
const barOrder = ['"skip-back"', '"pause" : "play-fill"', '"skip-forward"', 'id: volumeButton', '"list-music"', '"monitor"'];
let cursor = bar.indexOf('id: transport');
for (const marker of barOrder) {
    const at = bar.indexOf(marker, cursor);
    assert.ok(at > cursor, `${marker} in transport order`);
    cursor = at;
}
assert.match(bar, /name: "monitor"; label: QbzSession\.tr\("Desktop mode"[\s\S]{0,120}onClicked: QbzSession\.toggleProfile\(\)/, 'Desktop mode button');
assert.match(bar, /rotation: -90/, 'the volume slider opens vertically');
assert.match(bar, /volumePopup\.y = Math\.max\(8, g\.y - volumePopup\.height - 8\)/, 'the sheet is clamped inside the window');
assert.match(bar, /sliderLength: Math\.max\(120, Math\.min\(260,/, 'the sheet shortens on a short window');
const volumeButtonAt = bar.indexOf('id: volumeButton');
assert.ok(!/visible: root\.width >= \d+/.test(bar.slice(volumeButtonAt, volumeButtonAt + 200)), 'Volume has no width gate');

// ---- Now Playing transport: one centred row, every button on the axis -----
const transportAt = nowPlaying.indexOf('id: transport\n');
const transportBlock = nowPlaying.slice(transportAt, nowPlaying.indexOf('AudioStamp', transportAt));
assert.ok(nowPlaying.slice(transportAt - 40, transportAt).includes('Row {'), 'transport is a Row, not a Flow');
const buttons = transportBlock.match(/QbzIconButton \{[^\n]*\}/g);
assert.equal(buttons.length, 8, 'eight transport buttons');
for (const b of buttons) assert.match(b, /anchors\.verticalCenter: parent\.verticalCenter/, `centred: ${b.slice(0, 60)}`);
const order = buttons.map(b => b.match(/name: ([^;]+);/)[1]);
assert.deepEqual(order.slice(2, 5).map(n => n.includes('play-fill') ? 'play' : n), ['"skip-back"', 'play', '"skip-forward"'], 'transport in the middle');
assert.equal(order[0], '"info"'); assert.equal(order[1], '"shuffle"');
assert.match(transportBlock, /x: Math\.max\(4, Math\.min\(\(transportRow\.width - width\) \/ 2,/, 'the cluster is centred and yields to the stamp');

// ---- Artists drill-down: the query must not hang off a stale derived guard --
// A root `onSelectedArtistChanged` handler reads the DERIVED `drilled` before it
// is re-evaluated (false), so `if (root.drilled) detailQuery.restart()` never
// ran and every drill-down showed "0 albums". The trigger is a Connections on
// the host, and its guard reads the source.
const artistsTab = qml('kiosk/KioskLocalArtistsTab.qml');
assert.ok(!/^\s*onSelectedArtistChanged:/m.test(artistsTab), 'no root onSelectedArtistChanged handler');
assert.match(artistsTab, /Connections \{\s*target: root\.view\s*function onSelectedArtistChanged\(\) \{\s*if \(root\.view\.selectedArtist !== ""\)\s*detailQuery\.restart\(\)/, 'detail query re-issued from the host signal, guarded on the source');
assert.ok(!/detailQuery[\s\S]{0,400}if \(!root\.nativeActive/.test(artistsTab), 'the detail query is not gated on nativeActive');

// ---- round 2 (2026-09-07): quality badges, quiet immersive exit, 44px bar, no dead button
const localHeader = qml('views/local/LocalAlbumHeader.qml');
const kioskAlbum = qml('kiosk/KioskAlbum.qml');
const immersive = qml('immersive/ImmersiveView.qml');
const settings = qml('settings/SettingsView.qml');
assert.match(localHeader, /visible: root\.kioskHost && kioskQuality\.tier !== ""[\s\S]{0,600}QualityBadgeFull \{/, 'kiosk-only album quality badge in the local header');
assert.match(kioskAlbum, /QualityBadgeFull \{[\s\S]{0,400}detail: root\.header\.qualityDetail/, 'Qobuz kiosk album header carries a quality badge');
assert.ok(!/text: root\.header\.qualityDetail \|\| ""; color: theme\.textMuted/.test(kioskAlbum), 'the muted quality line is gone');
const exitAt = immersive.indexOf('visible: root.preKiosk');
const exitBlock = immersive.slice(exitAt, exitAt + 900);
assert.match(exitBlock, /opacity: root\.chromeVisible \? 1 : 0/, 'kiosk exit follows the auto-hiding chrome');
assert.match(exitBlock, /enabled: root\.chromeVisible/, 'kiosk exit is hit-enabled only while shown');
assert.ok(!/border\.width: 2/.test(exitBlock) && /width: 52/.test(exitBlock), 'kiosk exit is the quiet 52px form');
assert.match(immersive, /onPressed: function \(mouse\) \{ mouse\.accepted = true; root\.wake\(\) \}/, 'a backdrop press wakes the chrome (touch)');
assert.match(shell, /id: backBar[\s\S]{0,400}height: 44/, 'the back bar is 44px');
for (const id of ['backBtn', 'fwdBtn']) assert.match(shell, new RegExp(`ChromeButton \\{\\s*id: ${id}`), `${id} uses the shared chrome form`);
assert.match(shell, /ChromeButton \{\s*name: "cast"/, 'Cast uses the shared chrome form');
assert.match(shell, /id: kioskQconnectBtn[\s\S]{0,200}width: 44\s*height: 36/, 'Connect matches the 44x36 form');
const kioskHeader = settings.slice(settings.indexOf('// --- Header'), settings.indexOf('// --- Sub-nav'));
assert.ok(!kioskHeader.includes('id: logsButton') && !/onClicked: QbzShell\.logOpen\(\)/.test(kioskHeader), 'the dead Share-logs button is gone from the kiosk settings header');

// ---- round 3 (2026-09-07): search dashboard shelves, album actions, immersive exit ----
const search = qml('kiosk/KioskSearch.qml');
// The dashboard preview Components are instantiated by a Loader; reading the
// outer delegate's `section.modelData.rows` directly threw and emptied every
// shelf. Guarded stable props must be used instead.
assert.match(search, /readonly property var sRows: section\.modelData && section\.modelData\.rows/, 'guarded sRows');
assert.match(search, /readonly property string sKind: section\.modelData && section\.modelData\.kind/, 'guarded sKind');
assert.match(search, /rows: section\.sKind !== "track" \? section\.sRows : \[\]/, 'shelf is a direct child bound to guarded rows');
assert.ok(!/sourceComponent: section\.sKind === "track"/.test(search), 'the dashboard preview no longer hides behind a Loader Component');
assert.ok(!/model: section\.modelData\.rows/.test(search), 'no direct modelData.rows read in a preview list');
assert.ok(!/rows: section\.modelData\.rows/.test(search), 'no direct modelData.rows read in the shelf');
const albumQ = qml('kiosk/KioskAlbum.qml');
for (const ic of ['play-fill', 'shuffle', 'list-start', 'list-plus', 'list-end'])
    assert.match(albumQ, new RegExp(`QbzIconButton \\{ name: "${ic}"`), `album action icon ${ic}`);
assert.ok(!/model: \["Play", "Shuffle"\]/.test(albumQ), 'the text Play/Shuffle buttons are gone');
assert.match(albumQ, /QbzPlayer\.enqueueAlbum\(root\.header\.id \|\| "", "next"\)/, 'play next enqueues the album');
assert.match(albumQ, /QbzPlayer\.enqueueAlbum\(root\.header\.id \|\| "", "queue"\)/, 'add to queue enqueues the album');
assert.match(albumQ, /QualityBadgeFull \{[\s\S]{0,200}anchors\.right: parent\.right[\s\S]{0,120}anchors\.verticalCenter/, 'quality badge floats inline at the right');
assert.match(albumQ, /index % 2 === 1 \? theme\.surfaceHover/, 'album track rows are zebra-striped');
const immersive2 = qml('immersive/ImmersiveView.qml');
const exitAt2 = immersive2.indexOf('visible: root.preKiosk');
const exitBlock2 = immersive2.slice(exitAt2, exitAt2 + 700);
assert.match(exitBlock2, /anchors\.bottom: parent\.bottom/, 'immersive kiosk exit is anchored to the bottom');
assert.ok(!/anchors\.top: parent\.top/.test(exitBlock2), 'immersive kiosk exit is no longer at the top');

// ---- round 4 (2026-09-07): lyrics centered, artists rail as round-card grid ----
const llv = qml('shell/LyricsLinesView.qml');
assert.match(llv, /property bool centered: false/, 'LyricsLinesView exposes an opt-in centered');
assert.match(llv, /centered: view\.centered/, 'LyricsLinesView threads centered into the row');
const knp = qml('kiosk/KioskNowPlaying.qml');
assert.match(knp, /LyricsLinesView \{[\s\S]{0,400}centered: true/, 'kiosk Now Playing lyrics are centered');
const artistsTab2 = qml('kiosk/KioskLocalArtistsTab.qml');
assert.match(artistsTab2, /GridView \{\s*id: rail/, 'the artists rail is a GridView (multi-column), not a single-column ListView');
assert.match(artistsTab2, /KioskCard \{[\s\S]{0,120}round: true/, 'artists render as round cards');
assert.match(artistsTab2, /positionViewAtIndex\(root\.focusedItem, GridView\.Contain\)/, 'grid focus scroll uses GridView.Contain');

console.log('Kiosk feedback round: funnel parity with the desktop, Open entry point, back-bar drag/Connect/Cast, transport bar and Now Playing transport pass');
