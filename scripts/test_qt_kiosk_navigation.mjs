#!/usr/bin/env node
// Regressions in the production QML profile-switch and search restore handlers.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
const router = fs.readFileSync(new URL('../crates/qbz-qt/qml/shell/ContentRouter.qml', import.meta.url), 'utf8');
const search = fs.readFileSync(new URL('../crates/qbz-qt/qml/kiosk/KioskSearch.qml', import.meta.url), 'utf8');
function body(source, marker) {
    const at = source.indexOf(marker);
    assert.ok(at >= 0, marker);
    const start = source.indexOf('{', at);
    for (let depth = 1, i = start + 1; i < source.length; i++) {
        if (source[i] === '{') depth++;
        if (source[i] === '}' && --depth === 0) return source.slice(start + 1, i);
    }
    throw Error('unclosed handler');
}
const reports = [];
const context = vm.createContext({
    root: {kiosk: false},
    viewLoader: {item: {tab: 2, query: 'new desktop query'}},
    QbzShell: {kioskProfile: true, currentView: 'search', reportNavState: (scope, json) => reports.push([scope, JSON.parse(json)])},
});
vm.runInContext(`function switched() {${body(router, 'function onKioskProfileChanged()')}}`, context);
context.switched();
assert.deepEqual(reports.pop(), ['search', {activeTab: '2', query: 'new desktop query'}]);
context.QbzShell.currentView = 'local';
context.viewLoader.item = {activeTab: 'artists', navigationStateJson: JSON.stringify({activeTab: 'artists', selectedGenres: ['Jazz']})};
context.switched();
assert.deepEqual(reports.pop(), ['local', {activeTab: 'artists', selectedGenres: ['Jazz']}]);
context.QbzShell.kioskProfile = false;
context.switched();
assert.equal(reports.length, 0, 'ordinary desktop activity must not invoke the capture seam');
const handler = search.match(/onDocChanged:\s*(.+)/)[1];
Object.assign(context, {restoringQuery: 'A', doc: {query: 'B', loading: false}});
vm.runInContext(handler, context);
assert.equal(context.restoringQuery, 'A');
context.doc = {query: 'A', loading: true};
vm.runInContext(handler, context);
assert.equal(context.restoringQuery, 'A', 'an early query echo still carries stale result rows');
context.doc.loading = false;
vm.runInContext(handler, context);
assert.equal(context.restoringQuery, '', 'completed or failed restores release the loading state');
console.log('Kiosk navigation: current desktop query/tab capture and asynchronous search restoration pass');
