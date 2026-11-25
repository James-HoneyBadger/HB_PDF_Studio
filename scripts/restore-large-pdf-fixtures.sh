#!/bin/sh
# Restore optional large regression fixtures from the pre-cleanup commit.
set -eu
cd "$(git rev-parse --show-toplevel)"
source_commit=6618fedeb2cf8488dc635e2f95a3a2ae9a1eb5f6
for path in \
  'test pdf-bestanden/Originele bestanden/20260316 - Barn Relocation - 389 E Hemenway Lane - for Permit.pdf' \
  'test pdf-bestanden/Originele bestanden/2885 Demo project.pdf' \
  'test pdf-bestanden/Originele bestanden/NKD1a_opm_aw.pdf' \
  'test pdf-bestanden/Originele bestanden/NKE2D2_opm_aw.pdf' \
  'test pdf-bestanden/Originele bestanden/Zware vector PDF.pdf' \
  'test pdf-bestanden/Testbestanden/20260316 - Barn Relocation - 389 E Hemenway Lane - for Permit.pdf' \
  'test pdf-bestanden/Testbestanden/Zware vector PDF.pdf'
do
  if [ -e "$path" ]; then continue; fi
  mkdir -p "$(dirname "$path")"
  tmp=$(mktemp "${path}.XXXXXX")
  if git show "$source_commit:$path" > "$tmp"; then mv "$tmp" "$path"; else rm -f "$tmp"; echo "Fixture unavailable: $path (fetch full history first)" >&2; exit 1; fi
done
