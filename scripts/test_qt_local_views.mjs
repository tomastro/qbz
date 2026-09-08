#!/usr/bin/env node
// Execute the actual LocalLibraryView expressions against mixed-source rows.
// No Qt window, account, database, or audio input is required.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const qml = fs.readFileSync(process.argv[2] || fileURLToPath(
    new URL('../crates/qbz-qt/qml/views/LocalLibraryView.qml', import.meta.url)), 'utf8');
function body(marker) {
    const offset = qml.indexOf(marker);
    assert.ok(offset >= 0, `missing expression: ${marker}`);
    const begin = qml.indexOf('{', offset);
    for (let depth = 1, pos = begin + 1; pos < qml.length; ++pos) {
        if (qml[pos] === '{') ++depth;
        if (qml[pos] === '}' && --depth === 0) return qml.slice(begin + 1, pos);
    }
    throw new Error(`unclosed expression: ${marker}`);
}
const rows = [
    {id: 'local-copy', source: 'user', qualityTier: 'cd', format: 'flac'},
    {id: 'plex-copy', source: 'plex', qualityTier: 'cd', format: 'flac'},
    {id: 'jellyfin-copy', source: 'jellyfin', qualityTier: 'hires', format: 'flac'},
    {id: 'mixed', mediaVariants: [
        {source: 'local', qualityTier: 'cd', format: 'flac'},
        {source: 'qobuz_purchase', qualityTier: 'dsd', format: 'dsd'},
    ]},
];
const context = vm.createContext({
    console, albums: rows, selectedArtist: 'Led Zeppelin', artistsFilter: {},
    QbzLocal: {localArtistsNativeActive: false, artistAlbumIds: () => JSON.stringify(rows.map(r => r.id))},
    albumFavorite: () => false,
});
context.root = context;
vm.runInContext(`function sourceBucket(word) {${body('function sourceBucket(')}}
    function applyFilter(rows, selectedFilter) {${body('function applyFilter(')}}
    function artistAlbums() {${body('readonly property var artistAlbums:')}}`, context);
function albumIds(filter) {
    context.artistsFilter = filter;
    return Array.from(context.artistAlbums(), row => row.id);
}
assert.deepEqual(albumIds({local: true}), ['local-copy', 'mixed']);
assert.deepEqual(albumIds({plex: true}), ['plex-copy']);
assert.deepEqual(albumIds({jellyfin: true}), ['jellyfin-copy']);
assert.deepEqual(albumIds({local: true, plex: true}), ['local-copy', 'plex-copy', 'mixed']);
assert.deepEqual(albumIds({local: true, dsd: true}), []);
assert.deepEqual(albumIds({offline: true, dsd: true}), ['mixed']);
assert.deepEqual(albumIds({}), rows.map(r => r.id));
context.selectedArtist = '';
assert.deepEqual(albumIds({local: true}), []);

const transitions = [];
Object.assign(context, {
    activeTab: 'genres', localTabOrder: ['genres', 'albums', 'artists', 'folders', 'tracks'],
    ephemeralActive: false, _navigationReady: true, _restoringNavigationState: false,
    QbzShell: {currentView: 'local', recordLocalTab: (tab, state) => {
        assert.equal(JSON.parse(state).activeTab, context.activeTab, 'stamp before switching');
        transitions.push([context.activeTab, tab]);
    }},
});
Object.defineProperty(context, 'navigationStateJson', {
    get: () => JSON.stringify({activeTab: context.activeTab}),
});
vm.runInContext(`function activateTab(tab) {${body('function activateTab(')}}`, context);
context.activateTab('artists');
assert.equal(context.activeTab, 'artists');
context.activateTab('artists');
context.activateTab('not-a-local-tab');
assert.deepEqual(transitions, [['genres', 'artists']]);
context._restoringNavigationState = true;
context.activateTab('genres');
assert.equal(transitions.length, 1, 'history restore must not record itself');
context._restoringNavigationState = false;
context._navigationReady = false;
context.activateTab('tracks');
assert.equal(transitions.length, 1, 'initial landing must be one entry');
context._navigationReady = true;
context.activateTab('ephemeral');
assert.equal(context.activeTab, 'tracks', 'a closed session is not a navigable tab');
context.ephemeralActive = true;
context.activateTab('ephemeral');
assert.equal(context.activeTab, 'ephemeral', 'an explicit open must land on its active session');
assert.deepEqual(transitions.at(-1), ['tracks', 'ephemeral']);
context.activateTab('ephemeral');
assert.equal(transitions.length, 2, 'router and view notifications must not duplicate history');
context.activateTab('artists');
context.activateTab('ephemeral');
assert.equal(context.activeTab, 'ephemeral', 'a second explicit open must navigate again');
assert.deepEqual(transitions.at(-1), ['artists', 'ephemeral']);
console.log('Local view regressions OK: source-filtered artist albums, tab transitions and ephemeral navigation');
