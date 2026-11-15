// #635: the Linux AArch64 PDFium archive and library must match the pinned
// runtime preparation code and the prepared binary.
import assert from 'node:assert/strict';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { PDFIUM_RUNTIME } from './native-runtime.mjs';

const app = fileURLToPath(new URL('..', import.meta.url));
const binaries = join(app, 'src-tauri', 'binaries');
const workflows = join(app, '..', '.github', 'workflows');
const manifest = JSON.parse(readFileSync(join(binaries, 'pdfium-runtime.json'), 'utf8'));
const platforms = Object.entries(manifest.platforms);
const sha256 = (file) => createHash('sha256').update(readFileSync(file)).digest('hex');

test('Linux AArch64 names its PDFium revision, archive and checksums', () => {
  assert.deepEqual(platforms.map(([name]) => name), ['linux-arm64']);
  for (const [name, entry] of platforms) {
    assert.match(entry.revision, /^\d{4,}$/, `${name}: revision`);
    assert.equal(entry.archive,
      `https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${entry.revision}/pdfium-linux-arm64.tgz`,
      `${name}: archive of the recorded revision`);
    assert.match(entry.archive_sha256, /^[0-9a-f]{64}$/, `${name}: archive checksum`);
    assert.match(entry.library_sha256, /^[0-9a-f]{64}$/, `${name}: library checksum`);
    assert.equal(entry.shipped_from, 'native-runtime', `${name}: shipped_from`);
    assert.ok(existsSync(join(binaries, entry.library)), `${name}: ${entry.library} exists`);
  }
});

test('the prepared PDFium library matches its pinned checksum', () => {
  for (const [name, entry] of platforms) {
    assert.equal(sha256(join(binaries, entry.library)), entry.library_sha256, `${name}: ${entry.library} changed without updating pdfium-runtime.json`);
  }
});

test('native-runtime.mjs downloads and verifies the recorded Linux AArch64 runtime', () => {
  const linuxArm64 = manifest.platforms['linux-arm64'];
  assert.equal(PDFIUM_RUNTIME.version, linuxArm64.revision);
  assert.equal(PDFIUM_RUNTIME.url, linuxArm64.archive);
  assert.equal(PDFIUM_RUNTIME.sha256, linuxArm64.archive_sha256);
  assert.equal(PDFIUM_RUNTIME.librarySha256, linuxArm64.library_sha256);
  const runtime = JSON.parse(readFileSync(`${join(binaries, linuxArm64.library)}.runtime.json`, 'utf8'));
  assert.deepEqual(runtime, {
    version: linuxArm64.revision,
    sha256: linuxArm64.archive_sha256,
    librarySha256: linuxArm64.library_sha256,
  });
});

test('CI, stable and nightly builds prepare PDFium through the pinned runtime', () => {
  for (const file of ['ci.yml', 'release.yml', 'nightly.yml']) {
    const text = readFileSync(join(workflows, file), 'utf8');
    assert.match(text, /npm run prepare:native-runtime/, `${file} prepares the native runtime`);
  }
});

// pdfium-render bindt bij het laden ALLE functies van zijn binding-feature
// (pdfium_xxxx); exporteert de meegeleverde PDFium er één niet, dan mislukt
// het binden en rendert de app zonder PDFium. De standaard-feature
// pdfium_latest volgt de crate en niet onze binary, dus elke crate kiest de
// binding expliciet, en die mag niet nieuwer zijn dan de meegeleverde revisie.
test('pdfium-render binds a PDFium revision no newer than the shipped one', () => {
  const revision = Number(manifest.platforms['linux-arm64'].revision);
  const crates = ['../../pdfium-worker/Cargo.toml', '../src-tauri/Cargo.toml'];
  for (const crate of crates) {
    const text = readFileSync(fileURLToPath(new URL(crate, import.meta.url)), 'utf8');
    const line = text.split(/\r?\n/).find(l => /^pdfium-render\s*=/.test(l));
    assert.ok(line, `${crate}: depends on pdfium-render`);
    assert.match(line, /default-features\s*=\s*false/, `${crate}: no pdfium_latest via the default features`);
    const bindings = [...line.matchAll(/"pdfium_(\d{4})"/g)].map(m => Number(m[1]));
    assert.equal(bindings.length, 1, `${crate}: exactly one pdfium_xxxx binding feature`);
    assert.ok(bindings[0] <= revision, `${crate}: binding ${bindings[0]} is newer than the shipped PDFium ${revision}`);
  }
});

// #634: vóór pdfium-render 0.9.5 opende elke PdfPage meteen zijn structuurboom
// en hield PDFium die data vast tot het document sloot. De worker herlaadt
// getagde documenten daarom niet meer; dat mag alleen zolang de lock 0.9.5+
// vastlegt.
test('the lock pins a pdfium-render that loads structure trees lazily', () => {
  const lock = readFileSync(join(app, '..', 'Cargo.lock'), 'utf8');
  const versions = [...lock.matchAll(/name = "pdfium-render"\r?\nversion = "(\d+)\.(\d+)\.(\d+)"/g)];
  assert.ok(versions.length >= 1, 'Cargo.lock lists pdfium-render');
  for (const [, major, minor, patch] of versions) {
    const v = [Number(major), Number(minor), Number(patch)];
    assert.ok(v[0] > 0 || v[1] > 9 || (v[1] === 9 && v[2] >= 5), `pdfium-render ${v.join('.')} is older than 0.9.5`);
  }
});
