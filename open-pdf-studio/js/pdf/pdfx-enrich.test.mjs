// PDF/X-verrijking (#239, #422): output-intent, XMP, TrimBox, /Info.
//
// Met "sRGB — geen omzetting" moet de uitvoer byte voor byte gelijk blijven
// aan wat de export schreef. De twee hashes hieronder horen bij de code zoals
// die nu is: enrichForPdfX uit pdfx-export.js, ongewijzigd overgenomen, op
// precies deze vaste invoer met een bevroren klok. De /Producer-naam in de
// XMP is onderdeel van die bytes; bij een andere naam horen nieuwe hashes.

import assert from 'node:assert/strict';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { inflateSync } from 'node:zlib';
import { PDFDocument, PDFName, StandardFonts, rgb } from 'pdf-lib';

const FIXED = Date.UTC(2026, 0, 2, 3, 4, 5);
const RealDate = Date;
class FrozenDate extends RealDate {
  constructor(...a) {
    if (a.length) super(...a);
    else super(FIXED);
  }
  static now() {
    return FIXED;
  }
}
globalThis.Date = FrozenDate;

const { buildPdfxBytes } = await import('./pdfx-enrich.js');

async function fixture() {
  const doc = await PDFDocument.create();
  const page = doc.addPage([200, 100]);
  page.drawRectangle({ x: 10, y: 10, width: 50, height: 30, color: rgb(1, 0, 0) });
  const font = await doc.embedFont(StandardFonts.Helvetica);
  page.drawText('PDF/X', { x: 70, y: 40, size: 12, font });
  return doc.save();
}

const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');

test('sRGB zonder omzetting: PDF/X-3 byte voor byte gelijk aan de export van vóór #422', async () => {
  const out = await buildPdfxBytes(await fixture(), { conformance: 'X-3', title: 'Vaste invoer' });
  assert.equal(out.length, 2702);
  assert.equal(sha(out), 'd66b12fa59414612f4fe492abfe05fe603c1fdf206de282249f72af464f0c91d');
});

test('sRGB zonder omzetting: PDF/X-4 byte voor byte gelijk aan de export van vóór #422', async () => {
  const out = await buildPdfxBytes(await fixture(), { conformance: 'X-4', title: 'Vaste invoer' });
  assert.equal(out.length, 2697);
  assert.equal(sha(out), '2892b9a7660adb6003f2407159a9b8e66931ef0a27e58c80976254a9f90aae97');
});

test('met een CMYK-profiel: output-intent met N 4, de profielnaam en het profiel zelf', async () => {
  const profile = new Uint8Array(600).map((_, i) => (i * 7) & 0xff);
  const out = await buildPdfxBytes(await fixture(), {
    conformance: 'X-4',
    title: 'Vaste invoer',
    outputProfile: { bytes: profile, name: 'Gestreken papier 2026' },
  });
  const doc = await PDFDocument.load(out);
  const intents = doc.catalog.lookup(PDFName.of('OutputIntents'));
  assert.equal(intents.size(), 1);
  const intent = intents.lookup(0);
  assert.equal(intent.get(PDFName.of('S')).toString(), '/GTS_PDFX');
  assert.equal(intent.get(PDFName.of('OutputConditionIdentifier')).decodeText(), 'Gestreken papier 2026');
  assert.equal(intent.get(PDFName.of('Info')).decodeText(), 'Gestreken papier 2026');
  const icc = intent.lookup(PDFName.of('DestOutputProfile'));
  assert.equal(icc.dict.get(PDFName.of('N')).asNumber(), 4);
  assert.equal(icc.dict.get(PDFName.of('Filter')).toString(), '/FlateDecode');
  assert.deepEqual(new Uint8Array(inflateSync(icc.getContents())), profile);
  // XMP en TrimBox zoals bij sRGB.
  assert.ok(doc.catalog.get(PDFName.of('Metadata')));
  assert.ok(doc.getPage(0).node.get(PDFName.of('TrimBox')));
});

test('een profielnaam met niet-ASCII-tekens blijft leesbaar', async () => {
  const out = await buildPdfxBytes(await fixture(), {
    conformance: 'X-3',
    title: 'x',
    outputProfile: { bytes: new Uint8Array(8), name: 'Offset — geïllustreerd' },
  });
  const doc = await PDFDocument.load(out);
  const intent = doc.catalog.lookup(PDFName.of('OutputIntents')).lookup(0);
  assert.equal(intent.get(PDFName.of('OutputConditionIdentifier')).decodeText(), 'Offset — geïllustreerd');
});
