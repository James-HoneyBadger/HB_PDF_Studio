# Building from source

## Prerequisites

- [Node.js](https://nodejs.org/) 22 (the version CI uses)
- [Rust](https://www.rust-lang.org/tools/install), stable (the app crate needs at
  least 1.77.2)
- [CMake](https://cmake.org/download/) and a C/C++ toolchain
- **Linux AArch64** system dependencies:
  `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`

## Development

```bash
cd open-pdf-studio
npm ci
npm run tauri:dev
```

`predev` prepares the native PDFium runtime and the OCR runtime and builds the
`pdfium-worker` sidecar and the thumbnail handler before Vite starts.

## Release build

```bash
cd open-pdf-studio
npm ci
npm run tauri:build
```

The build downloads the pinned Linux AArch64 PDFium runtime and verifies its
SHA-256 checksums. Build artifacts are written to the workspace-level
`target/release/bundle/` directory. The release workflow
([`.github/workflows/release.yml`](../.github/workflows/release.yml)) is the
reference for the supported desktop target.

## Web version

```bash
cd open-pdf-studio
npm ci
OPDS_WEB_BUILD=1 npx vite build
```

`live.yml` deploys this bundle to the public site. `npm run build:hosted`
builds it for hosting under any sub-path; `npm run pack:hosted` packs it into
an archive. Neither is part of a release: releases ship the Linux AArch64
desktop app only.

## Tests

```bash
cd open-pdf-studio
npm run test:unit       # the unit tests listed in scripts/unit-testlijst.txt
npm run test:quality    # quality regression tests
```

New unit tests go next to the code as `*.test.mjs` and are added to
`scripts/unit-testlijst.txt`.

## README images

```bash
cd open-pdf-studio
node scripts/generate-readme-screenshots.mjs   # builds the web version and writes docs/images/readme/*.png
node scripts/check-readme-images.mjs           # every image present, used in both READMEs and at most 150 kB
```

The screenshot script makes its own demo PDFs (`scripts/readme-demo-pdfs.mjs`)
and drives the web version in headless Chromium. It needs the Playwright
Chromium browser (`npx playwright install chromium`).

## Platform notes

- Linux packaging details and fixes: [docs/linux-build-fixes.md](linux-build-fixes.md)
- Flatpak and Flathub: [docs/flathub.md](flathub.md)
