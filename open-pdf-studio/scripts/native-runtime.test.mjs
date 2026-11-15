import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  PDFIUM_RUNTIME,
  isNativeRuntimeReady,
  nativeRuntimePlan,
} from './native-runtime.mjs';

test('native runtime preparation is limited to Linux AArch64', () => {
  assert.equal(nativeRuntimePlan({ platform: 'win32', projectDir: 'C:\\app' }), null);
  assert.equal(nativeRuntimePlan({ platform: 'darwin', arch: 'arm64', projectDir: '/app' }), null);
  assert.equal(nativeRuntimePlan({ platform: 'linux', arch: 'x64', projectDir: '/app' }), null);
});

test('Linux AArch64 uses the pinned PDFium runtime and checksums', () => {
  const plan = nativeRuntimePlan({ platform: 'linux', arch: 'arm64', projectDir: '/app' });

  assert.equal(plan.version, '8076');
  assert.equal(plan.sha256, 'd7247b33ae5545615a5e877235dd97afc879e3a8805689684f528cae3339d352');
  assert.equal(plan.librarySha256, '2c6b186b84596e1e6c370b9058c8dfd64b3d944f4878e51993b3cddb160209e2');
  assert.equal(plan.libraryPath, path.join('/app', 'src-tauri', 'binaries', 'linux-arm64', 'libpdfium.so'));
  assert.equal(plan.metadataPath, `${plan.libraryPath}.runtime.json`);
  assert.equal(plan.url, PDFIUM_RUNTIME.url);
});

test('a cached runtime is valid only with matching metadata and library', async () => {
  const projectDir = await mkdtemp(path.join(os.tmpdir(), 'opds-runtime-'));
  const library = 'test library';
  const librarySha256 = createHash('sha256').update(library).digest('hex');
  const plan = {
    ...nativeRuntimePlan({ platform: 'linux', arch: 'arm64', projectDir }),
    librarySha256,
  };

  try {
    assert.equal(await isNativeRuntimeReady(plan), false);

    await mkdir(path.dirname(plan.libraryPath), { recursive: true });
    await writeFile(plan.libraryPath, 'wrong library');
    await writeFile(plan.metadataPath, JSON.stringify({
      version: plan.version,
      sha256: plan.sha256,
      librarySha256,
    }));
    assert.equal(await isNativeRuntimeReady(plan), false);

    await writeFile(plan.libraryPath, library);
    assert.equal(await isNativeRuntimeReady(plan), true);
  } finally {
    await rm(projectDir, { recursive: true, force: true });
  }
});
