// De voorbeeldtekening die met de app meegaat: een plattegrond op A3, schaal
// 1:50, met een /VP-viewport en /Measure, zodat meten meteen werkt. Getekend
// met pdf-lib door scripts/readme-demo-pdfs.mjs, dus geen tekening van een
// klant of project erin.
//
// Schrijven na een wijziging aan readme-demo-pdfs.mjs:
//   node scripts/voorbeeldtekening.mjs
// De uitvoer staat in src-tauri/resources/voorbeeld/ en wordt als resource
// meegebundeld (tauri.conf.json); `voorbeeld_tekening_pad` in lib.rs wijst
// hem aan.

import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { maakDemoPdfs } from './readme-demo-pdfs.mjs';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const doelMap = path.join(appRoot, 'src-tauri', 'resources', 'voorbeeld');
const doel = path.join(doelMap, 'voorbeeld-plattegrond.pdf');

export async function schrijfVoorbeeldtekening() {
  const pdfs = await maakDemoPdfs();
  const bytes = pdfs['floor-plan-rev-a.pdf'];
  await mkdir(doelMap, { recursive: true });
  await writeFile(doel, bytes);
  return doel;
}

const entryPoint = process.argv[1] ? path.resolve(process.argv[1]) : '';
if (entryPoint === fileURLToPath(import.meta.url)) {
  schrijfVoorbeeldtekening()
    .then((pad) => console.log(`voorbeeld: ${path.relative(appRoot, pad)}`))
    .catch((fout) => {
      console.error(`voorbeeld: ${fout.message}`);
      process.exitCode = 1;
    });
}
