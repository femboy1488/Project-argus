#!/usr/bin/env bash
# Collects release assets into one folder and writes SHA256SUMS.txt (sha256sum format) for a GitHub Release.
# Argus (desktop) REFUSES to self-update from a release without SHA256SUMS.txt or with a wrong hash.
#
# Usage:  scripts/make-release-assets.sh [DIR]          (default DIR=/workspace/work/out)
#   DIR should contain any of: Argus-Setup.exe, Argus-Node.apk, *.AppImage, *.deb
#
# Manual release (no CI):
#   1. tag must be vX.Y.Z (e.g. v0.3.1) — tags like "V2" are ignored by the updater;
#   2. upload every file listed in DIR/SHA256SUMS.txt AND SHA256SUMS.txt itself, names unchanged
#      (Argus-Setup.exe and Argus-Node.apk are looked up by exact name; AppImage by the .AppImage suffix);
#   3. the release description (markdown) is shown in the «Доступна версия …» dialog.
#   gh CLI:  gh release create v0.3.1 DIR/Argus-Setup.exe DIR/Argus-Node.apk DIR/SHA256SUMS.txt --notes-file NOTES.md
set -euo pipefail
DIR=${1:-/workspace/work/out}
cd "$DIR"
shopt -s nullglob
files=()
for f in Argus-Setup.exe Argus-Node.apk *.AppImage *.deb; do [ -f "$f" ] && files+=("$f"); done
if [ ${#files[@]} -eq 0 ]; then echo "no release assets in $DIR" >&2; exit 1; fi
sha256sum "${files[@]}" > SHA256SUMS.txt
echo "== $DIR/SHA256SUMS.txt"
cat SHA256SUMS.txt
echo "== sizes"
ls -l "${files[@]}" SHA256SUMS.txt
sha256sum -c --quiet SHA256SUMS.txt && echo "verify: OK"
