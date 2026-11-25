**English** · [Nederlands](README.nl.md)

<div align="center">

# HB PDF Studio

**A free, open-source PDF editor for people who work with drawings: markup, measuring to scale, revision compare and CAD import, with a Linux AArch64 desktop app.**

[![License](https://img.shields.io/badge/license-LGPL--3.0--or--later-blue)](#license)
[![Languages](https://img.shields.io/badge/languages-40-informational)](open-pdf-studio/js/i18n/locales/)
[![MCP tools](https://img.shields.io/badge/MCP%20tools-94-informational)](mcp-stdio/tools.json)

### [Download](https://github.com/James-HoneyBadger/HB_PDF_Studio/releases)

![A floor plan with a revision cloud, a callout and a measured distance; the Format tab and the properties of the selected callout are open](docs/images/readme/hero.png)

</div>

HB PDF Studio is a PDF editor for architects, engineers, contractors and everyone else who reviews drawings and documents. You mark up, measure with the scale the drawing carries, compare revisions, place symbols, count quantities and organise pages, with no subscription, account or watermark.

The desktop app supports Linux on AArch64 only. It brings what needs your computer: printing, OCR, DWG and DXF import and export, digital signatures and a local AI connection (MCP).

## Download and support

**Supported platform: Linux AArch64 (ARM64) only.** No Windows, macOS, Android or x86-64 Linux support is offered. See [HB PDF Studio releases](https://github.com/James-HoneyBadger/HB_PDF_Studio/releases) for available packages. Upstream OpenAEC packages are not HB releases. Build from source using [docs/building.md](docs/building.md) if a verified HB package is not available.

Automatic update checks are disabled in this development branch; release signing and packaging must be independently qualified before publication.

## Why use it?

- **Free and open.** LGPL-3.0-or-later. No licence fees, no account, no telemetry and no watermark ([PRIVACY.md](PRIVACY.md)).
- **Made for drawings.** Clouds, callouts, hatches, stamps and symbol libraries, and printing to scale on sheets up to A0.
- **Measure to scale at once.** A drawing that carries its own scale, such as a CAD plot or an imported DWG, measures correctly without calibrating.
- **See what changed.** Compare two revisions side by side or on top of each other, with a list of every change.
- **CAD in and out.** Import DWG and DXF as a vector PDF with layers and a measuring scale, and export a page back to DXF or DWG (desktop).
- **Works with an AI assistant.** A local MCP server with 94 tools lets an assistant open, mark up, measure and save documents for you (desktop).
- **Available on Linux AArch64**, in 40 languages.

## Screenshots

| | |
|---|---|
| ![Area, perimeter and two distances measured on a floor plan at scale 1:50, Default theme](docs/images/readme/measure.png) **Measuring with the scale of the drawing** | ![Two revisions of a floor plan on top of each other, with the moved wall highlighted and a list of 18 changes](docs/images/readme/compare.png) **Comparing two revisions** |
| ![Fire safety symbols from the NEN 1414 library placed on a floor plan, with the symbol palette open](docs/images/readme/symbols.png) **Symbols from the palette** | ![The Edit Pages window with six pages: a cover, two floor plans and three pages of text](docs/images/readme/pages.png) **Organising pages** |
| ![Search results for "wall" in the text and the annotations, theme Dark](docs/images/readme/search.png) **Searching text and annotations, theme Dark** | ![The View tab with the window split in two: a floor plan on the left and the specification on the right, theme Blue](docs/images/readme/split.png) **Split view, theme Blue** |

The screenshots come from the app's interface with demo PDFs that the script makes itself: `open-pdf-studio/scripts/generate-readme-screenshots.mjs`.

## Markup and measuring

- **Markup.** Highlights, shapes, clouds, callouts, text boxes, sticky notes, freehand, stamps, images and signatures, with styles, layers and alignment tools.
- **Measuring.** Distance, area, perimeter and angle, to the scale in the drawing or a scale you calibrate, with snapping to the drawing.
- **Quantities.** Count objects by category and collect counts, lengths and areas in a schedule that you place on the page or export as CSV.
- **Redaction.** Removes the text, images and vectors under the marked area from the file, not just from view.

The full list is in [docs/features.md](docs/features.md).

## Documents and pages

- **Pages.** Insert, delete, extract, split, rotate, reorder and merge; straighten a skewed scan or shift page content.
- **Forms, bookmarks and attachments.** Fill and create form fields, edit bookmarks, manage attachments.
- **Watermarks, headers and footers** with page numbers and dates.
- **Search** in the page text and in annotations, with a result list per page.
- **Keyboard shortcuts** that you can change, including a preset with single-letter markup keys: [docs/keyboard-shortcuts.md](docs/keyboard-shortcuts.md).

## Desktop features

- **CAD.** DWG and DXF import with layers, viewports and a measuring scale, and export of a page to DXF or DWG.
- **Printing** to scale on large sheets, with a live preview and *Save as PDF*.
- **OCR** to make scans searchable.
- **Digital signatures**: validation with the certificate chain, and signing.
- **Managed installs** with `OPENAEC_BEHEERD=1` and a fixed start folder: [docs/managed-deployments.md](docs/managed-deployments.md).

## Interoperability

| Format | Open / import | Save / export |
|--------|---------------|---------------|
| **PDF** | Yes, including forms (AcroForm, XFA) and layers | Yes; PDF/A validation and saving, PDF/X export, raster PDF, flattened copy |
| **XFDF** | Annotations; a tool set from sample markups | Annotations |
| **BCF 2.1** | Issues as annotations | Annotations as issues |
| **DWG, DXF** | Yes, as a vector PDF (desktop) | A page with its layers and markups (desktop) |
| **IFCX** (IFC5 JSON) | No | Yes, beta ([docs/ifcx-export.md](docs/ifcx-export.md)) |
| **PNG, JPEG** | As an image on a page | Pages, at 72 to 600 DPI |
| **DOCX, ODT, XLSX** | No | Text and tables |
| **CSV** | Edited tags of a saved quantities schedule (label, IFC category, location, count category) | Quantities schedule, with an ID column per markup |

What does not survive: XFDF and BCF carry annotations only, not the page content. A flattened copy leaves out form field values. DOCX, ODT and XLSX get the text and tables, and complex layouts may need correction. PDF/X keeps standard fonts that are not embedded as they are. What IFCX keeps and loses is listed in [docs/ifcx-export.md](docs/ifcx-export.md).

## Embedding in a web page

A separate, lightweight build lets another web application show a drawing with measuring and comments in its own page: `npm run build:web-unit` writes `open-pdf-studio/dist-web-unit/`. The script a host loads is about 10 KiB gzip; PDF.js arrives with the first document, the PDF writer only when the host asks for the result, and nothing in the unit uses the desktop shell. It uses the same annotation model as the desktop app, so a drawing marked up on the web opens the same way on the desktop.

```html
<div id="viewer" style="height:600px"></div>
<script type="module">
  import { mount } from './web-unit/open-pdf-studio.js';

  const unit = mount('#viewer', {
    scale: '1:100', unit: 'mm', lang: 'en', tools: ['measure', 'comment', 'shape'],
    onChange: ({ count }) => console.log(count, 'annotations'),
  });
  await unit.load('/drawings/123.pdf');           // a URL, bytes or a File
  const pdf = await unit.exportPdf();              // Uint8Array with the markups
  const annotations = unit.getAnnotations();       // the same JSON model as the desktop app
</script>
```

The same unit works as a `<open-pdf-studio src="…" scale="1:100">` element and inside an iframe with a `postMessage` protocol. Options include a read-only mode (`readOnly: true`), a dark theme and CSS variables, a hidden toolbar for hosts with their own buttons, and `storage: 'local'` to remember the page and scale per document (off by default). The events are `opds:load`, `opds:change`, `opds:save` and `opds:error`. A minimal host page is in [`open-pdf-studio/web-unit/voorbeeld/`](open-pdf-studio/web-unit/voorbeeld/index.html). The unit measures distances and places sticky notes and rectangles; area, perimeter, text boxes, editing existing annotations and XFDF are not in it yet ([#451](https://github.com/OpenAEC-Foundation/open-pdf-studio/issues/451)).

## MCP

The desktop app has a built-in MCP server, so an AI assistant can work with your documents: open and save files, navigate, create and edit annotations, set the measuring scale, take off quantities, import and export CAD and print to a PDF file. It has 94 public tools.

You stay in control. The server is on for a new installation and listens only on your own computer (`127.0.0.1`, port 9223); switch it off or change the port under Settings > General. The status bar shows when an assistant is connected and which tool it used last. `open-pdf-studio.mcpb` from the release installs the connection in MCP clients that support `.mcpb` bundles; [mcp-stdio/README.md](mcp-stdio/README.md) explains the bridge for other clients. A [Python library](docs/python-library.md) drives the same tools from a script.

**Where your document goes.** The MCP connection itself stays on your computer, but the assistant you connect decides where the work is done. If it uses an online AI service, the page text, screenshots and annotations it reads through the tools can be sent to that service. Check that service's data terms before you let it work on confidential drawings, or use an assistant with a model that runs locally.

## Testing and reliability

- **798 unit test files** run with `npm run test:unit` on Linux AArch64 in [CI](.github/workflows/ci.yml) for every pull request, plus 3 quality regression test files (`npm run test:quality`).
- **Inherited CI is designed to build and start the Linux AArch64 app**: the AppImage on Debian 13, Fedora 44 and in OpenAEC Linux conditions. It also tests the Python library and lints the Flatpak when it changes.
- **Pull requests that touch rendering** also run a render regression on a large-format drawing and keep the report ([render-regression.yml](.github/workflows/render-regression.yml)).
- **Release qualification is pending.** The inherited workflow and signing keys require HB-specific configuration; do not treat upstream signatures as HB release signatures.
- **Privacy is tested.** A unit test fails when the shipped source contains an internet address that [PRIVACY.md](PRIVACY.md) does not list, or when a telemetry package is added.
- **README images are checked.** `node open-pdf-studio/scripts/check-readme-images.mjs` fails when an image is missing, unused or larger than 150 kB.

Count sources: unit test files are the non-comment lines of [`open-pdf-studio/scripts/unit-testlijst.txt`](open-pdf-studio/scripts/unit-testlijst.txt); quality tests are the `test:quality` script in [`open-pdf-studio/package.json`](open-pdf-studio/package.json); MCP tools are the entries in [`mcp-stdio/tools.json`](mcp-stdio/tools.json); languages are the folders in [`open-pdf-studio/js/i18n/locales/`](open-pdf-studio/js/i18n/locales/).

## Current limitations

- **Linux packages up to 2026.44.8 render without the worker pool.** They lacked the `pdfium-worker` sidecar, so every page rendered in-process and large documents were slow ([#923](https://github.com/OpenAEC-Foundation/open-pdf-studio/issues/923)). The fix is on `main` and ships with the next release.
- **On some Linux systems the app can still crash while zooming.** The app now lowers the realtime limit and turns off GPU acceleration of the page canvas, the two known causes; the issue stays open until that is confirmed on affected systems ([#924](https://github.com/OpenAEC-Foundation/open-pdf-studio/issues/924), workarounds in [docs/install-linux.md](docs/install-linux.md)).
- **The desktop app is Linux AArch64 only.** Windows, macOS and Android builds and the self-hosted web bundle are no longer produced from this repository.
- **The Flatpak is not on Flathub yet** ([docs/flathub.md](docs/flathub.md)).
- **The Table button on the Drawing tab is not available yet.** Place a quantities schedule as a table instead.

Found something? [Open an HB issue](https://github.com/James-HoneyBadger/HB_PDF_Studio/issues).

## Under the hood

The app is a [Tauri 2](https://tauri.app/) desktop app with a Rust back end and a [SolidJS](https://www.solidjs.com/) front end built with [Vite](https://vitejs.dev/). Pages render through a multi-process [PDFium](https://pdfium.googlesource.com/pdfium/) worker pool, off the UI thread and isolated from crashes, with progressive tiles for very large drawings. [pdf-lib](https://pdf-lib.js.org/) writes the PDF changes. The licences of all components are in [LICENSING.md](LICENSING.md).

## Contributing

- **Report a bug or ask for a feature** as an [issue](https://github.com/James-HoneyBadger/HB_PDF_Studio/issues/new). Describe what you did, what you expected and what happened. Because the code moves quickly, a precise issue is often more useful than a pull request.
- **Share symbol libraries and other content.** Pull requests with symbol sets, hatch patterns and similar content from your country or discipline are very welcome.
- **Write code.** Read [CONTRIBUTING.md](CONTRIBUTING.md) first.

We are looking for someone to maintain the extensions and the content repository: curating incoming symbol libraries and keeping the catalogue organised. If that is you, open an issue to introduce yourself.

## Quick start (developers)

```bash
git clone https://github.com/James-HoneyBadger/HB_PDF_Studio.git
cd HB_PDF_Studio/open-pdf-studio
npm ci
npm run tauri:dev
```

Requires Node.js 22, Rust stable, CMake and a C/C++ toolchain, plus the WebKitGTK development packages on Linux. Details, release builds, the web version and tests are in [docs/building.md](docs/building.md).

Versions follow the OpenAEC scheme `YYYY.W.N`: year, ISO week without a leading zero, and a number within the week that starts at 0, such as `2026.44.8`.

HB PDF Studio is a Honey Badger Universe fork of [OpenAEC Foundation's Open PDF Studio](https://github.com/OpenAEC-Foundation/open-pdf-studio). Upstream copyright and license notices remain applicable.

## License

HB PDF Studio is licensed under the [GNU Lesser General Public License v3.0 or later](LICENSE.md). The LGPL builds on the [GNU General Public License v3.0](GPL-3.0.txt), whose text ships alongside it. [LICENSING.md](LICENSING.md) explains what that means in plain language, lists the licence of every component and of the third-party components the app ships with, and answers the AGPL question around the MuPDF dependency.
