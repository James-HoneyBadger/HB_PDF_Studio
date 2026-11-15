import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { PDFDocument } from 'pdf-lib';
import { createRenderFixture } from './create-render-fixture.mjs';

const projectDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const repoDir = path.resolve(projectDir, '..');

async function readJson(relativePath) {
  return JSON.parse(await readFile(path.join(projectDir, relativePath), 'utf8'));
}

test('base config keeps mobile and auxiliary windows visible; desktop main uses a native startup gate', async () => {
  const config = await readJson('src-tauri/tauri.conf.json');
  assert.equal(config.app.windows[0].visible, true);
});

test('Linux AArch64 PDFium preparation is wired into local and release builds', async () => {
  const pkg = await readJson('package.json');
  assert.match(pkg.scripts.predev, /prepare:native-runtime/);
  assert.match(pkg.scripts.prebuild, /prepare:native-runtime/);
  assert.equal(pkg.scripts['prepare:native-runtime'], 'node scripts/native-runtime.mjs');
});

test('quality tests do not depend on shell glob expansion', async () => {
  const pkg = await readJson('package.json');
  assert.doesNotMatch(pkg.scripts['test:quality'], /\*/);
  for (const name of [
    'native-runtime.test.mjs',
    'release-config.test.mjs',
    'square-image-annotation.test.mjs',
  ]) {
    assert.match(pkg.scripts['test:quality'], new RegExp(name.replaceAll('.', '\\.')));
  }
});

test('render regression caches the workspace target and prebuilds cold CI jobs', async () => {
  const workflow = await readFile(path.join(repoDir, '.github', 'workflows', 'render-regression.yml'), 'utf8');
  const runner = await readFile(path.join(repoDir, 'scripts', 'run-render-regression.mjs'), 'utf8');
  assert.match(workflow, /^\s+target\s*$/m);
  assert.match(workflow, /uses: actions\/cache\/restore@v4/);
  assert.match(workflow, /uses: actions\/cache\/save@v4/);
  assert.match(workflow, /id: cargo-cache/);
  assert.match(workflow, /key: render-regression-v2-/);
  assert.match(workflow, /name: Prebuild desktop app/);
  assert.match(workflow, /cargo build -p open-pdf-studio/);
  assert.match(workflow, /name: Save cargo registry \+ target\s+if: steps\.cargo-cache\.outputs\.cache-hit != 'true'/);
  assert.doesNotMatch(workflow, /open-pdf-studio\/src-tauri\/target/);
  assert.doesNotMatch(workflow, /open-pdf-render\/target/);
  assert.match(workflow, /timeout-minutes: 45/);
  assert.match(runner, /const MAX_WAIT_MS = 600_000/);
});

test('render regression uses the bundled deterministic PDF fixture', async () => {
  const workflow = await readFile(path.join(repoDir, '.github', 'workflows', 'render-regression.yml'), 'utf8');
  const app = await readFile(path.join(projectDir, 'src-tauri', 'src', 'lib.rs'), 'utf8');
  const mcpServer = await readFile(path.join(projectDir, 'src-tauri', 'src', 'mcp_server.rs'), 'utf8');
  assert.match(workflow, /OPS_TEST_PDFS_DIR: \$\{\{ github\.workspace \}\}\/open-pdf-studio\/src-tauri\/resources\/kaders/);
  assert.match(workflow, /--pdf grootformaat_a1_liggend\.pdf/);
  assert.doesNotMatch(workflow, /Prepare deterministic render corpus/);
  assert.match(app, /mcp_server::resolve_test_pdfs_dir/);
  assert.match(mcpServer, /env!\("CARGO_MANIFEST_DIR"\)/);
  assert.doesNotMatch(mcpServer, /std::env::current_dir\(\)/);
});

test('generated render fixture is a complete one-page PDF', async () => {
  const dir = await mkdtemp(path.join(tmpdir(), 'open-pdf-studio-render-'));
  const output = path.join(dir, 'render-fixture.pdf');
  try {
    await createRenderFixture(output);
    const bytes = await readFile(output);
    const pdf = await PDFDocument.load(bytes);
    assert.equal(pdf.getPageCount(), 1);
    assert.match(Buffer.from(bytes).subarray(-32).toString('latin1'), /%%EOF/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test('the only platform layer is Linux', async () => {
  const config = await readJson('src-tauri/tauri.conf.json');
  const linux = await readJson('src-tauri/tauri.linux.conf.json');
  const resources = { ...config.bundle.resources, ...linux.bundle.resources };

  assert.equal(resources['binaries/linux-arm64/libpdfium.so'], 'libpdfium.so');
  for (const name of Object.keys(resources)) {
    assert.doesNotMatch(name, /\\.dll$|\\.dylib$/);
  }
  assert.deepEqual(config.bundle.targets, ['deb', 'appimage']);
});

test('Linux packages ship the pdfium-worker sidecar for the render pool (#923)', async () => {
  const config = await readJson('src-tauri/tauri.conf.json');
  const linux = await readJson('src-tauri/tauri.linux.conf.json');
  // Tauri merges tauri.linux.conf.json as a JSON merge patch: an array in the
  // platform file replaces the base array instead of extending it.
  const externalBin = linux.bundle.externalBin ?? config.bundle.externalBin;
  assert.ok(externalBin.includes('binaries/pdfium-worker'), 'Linux bundles binaries/pdfium-worker');

  // The worker runs from usr/bin; libpdfium.so is a resource in
  // usr/lib/<productName> (Tauri's Linux resource_dir).
  const resources = { ...config.bundle.resources, ...linux.bundle.resources };
  assert.equal(resources['binaries/linux-arm64/libpdfium.so'], 'libpdfium.so');
  const render = await readFile(path.join(repoDir, 'pdfium-worker', 'src', 'render.rs'), 'utf8');
  assert.ok(render.includes(`"../lib/${config.productName}"`), 'the worker looks in ../lib/<productName>');

  // tauri-build requires binaries/pdfium-worker-<target-triple> before bundling.
  for (const name of ['ci.yml', 'release.yml', 'nightly.yml']) {
    const workflow = await readFile(path.join(repoDir, '.github', 'workflows', name), 'utf8');
    const step = workflow.match(/- name: Build pdfium-worker sidecar \(Linux\)[\s\S]*?(?=\n\s*- name:)/);
    assert.ok(step, `${name} builds the Linux worker`);
    assert.match(step[0], /cargo build --release -p pdfium-worker/, name);
    assert.match(step[0], /binaries\/pdfium-worker-aarch64-unknown-linux-gnu/, name);
  }

  const smoke = await readFile(path.join(projectDir, 'scripts', 'linux-appimage-smoke.sh'), 'utf8');
  assert.match(smoke, /squashfs-root\/usr\/bin\/pdfium-worker/);
});

test('desktop CI and release workflows only build Linux AArch64', async () => {
  const workflow = await readFile(path.join(repoDir, '.github', 'workflows', 'ci.yml'), 'utf8');
  const ciBuild = workflow.match(/  build:\n[\s\S]*?(?=\n  [a-z][a-z0-9-]*:)/)?.[0];
  assert.ok(ciBuild);
  assert.match(ciBuild, /runs-on: ubuntu-24\.04-arm/);
  assert.doesNotMatch(ciBuild, /windows-latest|macos-|matrix:/);
  assert.match(workflow, /npm run prepare:native-runtime/);

  for (const name of ['release.yml', 'nightly.yml']) {
    const text = await readFile(path.join(repoDir, '.github', 'workflows', name), 'utf8');
    assert.doesNotMatch(text, /windows-latest|macos-|android|web-bundle/, name);
    assert.match(text, /runs-on: ubuntu-24\.04-arm/, name);
  }
});

// De versievelden (allemaal gelijk, schema JJJJ.W.N) en het ontbreken van
// een MSI-build bewaakt scripts/versievelden.test.mjs (#667).

test('development optimization profiles live at the workspace root', async () => {
  const workspaceCargo = await readFile(path.join(repoDir, 'Cargo.toml'), 'utf8');
  const appCargo = await readFile(path.join(projectDir, 'src-tauri', 'Cargo.toml'), 'utf8');
  assert.match(workspaceCargo, /\[profile\.dev\.package\.open-pdf-render\]/);
  assert.match(workspaceCargo, /\[profile\.dev\.package\.pdfium-render\]/);
  assert.doesNotMatch(appCargo, /\[profile\./);
});

test('Little CMS is built from source, never linked from the build machine (#750, #361)', async () => {
  const cargo = await readFile(path.join(repoDir, 'open-pdf-cmyk', 'Cargo.toml'), 'utf8');
  const line = cargo.split('\n').find((l) => /^lcms2\s*=/.test(l));
  assert.ok(line, 'open-pdf-cmyk depends on lcms2');
  assert.match(line, /features\s*=\s*\[[^\]]*"static"/, line);

  for (const name of ['ci.yml', 'release.yml', 'nightly.yml']) {
    const workflow = await readFile(path.join(repoDir, '.github', 'workflows', name), 'utf8');
    assert.doesNotMatch(workflow, /LCMS2_LIB_DIR/, `${name} does not point lcms2-sys at a system library`);
  }
});
