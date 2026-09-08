#!/usr/bin/env node
// Exercise the production artwork URL policy without Qt, network or audio.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
const source = fs.readFileSync(new URL('../crates/qbz-qt/qml/assets/kiosk-art.js', import.meta.url), 'utf8');
const policy = vm.createContext({});
vm.runInContext(source.replace(/^\.pragma library\s*/, ''), policy);
const cover = 'https://static.qobuz.com/images/covers/AB/CD/123_600.jpg';
assert.equal(policy.sizedUrl(cover, 96), cover.replace('_600', '_100'));
assert.equal(policy.sizedUrl(cover, 192), cover.replace('_600', '_230'));
assert.equal(policy.sizedUrl(cover.replace('_600', '_50'), 320), cover);
assert.equal(policy.sizedUrl(cover.replace('_600', '_org') + '?v=1', 192), cover.replace('_600', '_230') + '?v=1');
for (const path of ['/music/album_600.jpg', 'file:///music/album_600.jpg',
                    'https://example.org/album_600.jpg',
                    'https://qobuz.com.example.org/album_600.jpg']) {
    assert.equal(policy.sizedUrl(path, 96), path);
}
assert.equal(policy.sizedUrl(cover, 0), cover);
assert.equal(policy.sizedUrl(cover, 720), cover);
console.log('Kiosk artwork: physical DPR targets, undersized input, query strings and foreign/local paths pass');
