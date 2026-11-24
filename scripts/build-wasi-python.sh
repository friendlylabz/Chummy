#!/usr/bin/env bash
# Builds the Python that Chummy's tools run on: CPython compiled to
# WebAssembly (WASI), from python.org's signed source release, with
# CPython's own WASI build script (Tools/wasm/wasi; WASI is a tier 2
# platform for CPython). The result is bundled inside Chummy, so users
# never download it.
#
# Every input is pinned below and checked before use:
#   - the CPython source tarball: SHA-256, and its Sigstore signature by
#     the 3.14 release manager (https://www.python.org/downloads/metadata/sigstore/);
#   - wasi-sdk (the compiler) and the wasmtime CLI (runs build steps): SHA-256.
#
# Output (in $OUT, default ./wasi-python):
#   python-wasi.zip         python.wasm, lib/python3.14/…, LICENSE
#   python-wasi.zip.sha256
#   build-info.txt          what went in
#
# Linux x86_64 only (it's what the release workflow runs on). Needs: curl,
# tar, xz, zip, python3 (3.11+, with venv, for the Sigstore check), a C
# compiler and make. Build dependencies on Ubuntu:
#   sudo apt-get install build-essential pkg-config libffi-dev zip xz-utils
#
# To update: change the versions and hashes below (the CPython hash is on
# its python.org release page), then run it and commit.
set -euo pipefail

PY_VERSION="3.14.10"
PY_SHA256="9e4996c19ae5a3df31f10c37723661f71d3455d73bd005f79df7098e5cc13192"
PY_SIGNER="hugo@python.org"
PY_SIGNER_ISSUER="https://github.com/login/oauth"

WASI_SDK_VERSION="24"
WASI_SDK_SHA256="c6c38aab56e5de88adf6c1ebc9c3ae8da72f88ec2b656fb024eda8d4167a0bc5"

WASMTIME_VERSION="51.0.0"
WASMTIME_SHA256="a18020674fac4107f329bc9ed720ea9c88e4b30a61e8a527787bcce051123d32"

SIGSTORE_VERSION="4.6.0"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${OUT:-$ROOT/wasi-python}"
WORK="${WORK:-$(mktemp -d)}"
# Fixed timestamps, so the same inputs give the same zip.
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1767225600}" # 2026-01-01

say() { printf '\n== %s\n' "$*"; }

fetch() { # url file sha256
  curl -sSfL --retry 3 -o "$2" "$1"
  echo "$3  $2" | sha256sum -c --quiet - || {
    echo "Checksum mismatch for $1 (got $(sha256sum "$2" | cut -d' ' -f1))" >&2
    exit 1
  }
}

[ "$(uname -s)-$(uname -m)" = "Linux-x86_64" ] || { echo "Run this on Linux x86_64." >&2; exit 1; }
mkdir -p "$OUT" "$WORK"
cd "$WORK"
echo "Working in $WORK"

say "CPython $PY_VERSION source"
tgz="Python-$PY_VERSION.tgz"
fetch "https://www.python.org/ftp/python/$PY_VERSION/$tgz" "$tgz" "$PY_SHA256"
curl -sSfL --retry 3 -o "$tgz.sigstore" "https://www.python.org/ftp/python/$PY_VERSION/$tgz.sigstore"

say "Checking the Sigstore signature ($PY_SIGNER)"
python3 -m venv sigstore-venv
sigstore-venv/bin/pip install --quiet "sigstore==$SIGSTORE_VERSION"
sigstore-venv/bin/python -m sigstore verify identity \
  --bundle "$tgz.sigstore" \
  --cert-identity "$PY_SIGNER" \
  --cert-oidc-issuer "$PY_SIGNER_ISSUER" \
  "$tgz"
tar -xzf "$tgz"
SRC="$WORK/Python-$PY_VERSION"

say "wasi-sdk $WASI_SDK_VERSION"
sdk="wasi-sdk-$WASI_SDK_VERSION.0-x86_64-linux"
fetch "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-$WASI_SDK_VERSION/$sdk.tar.gz" \
  "$sdk.tar.gz" "$WASI_SDK_SHA256"
tar -xzf "$sdk.tar.gz"
export WASI_SDK_PATH="$WORK/$sdk"

say "wasmtime $WASMTIME_VERSION"
wt="wasmtime-v$WASMTIME_VERSION-x86_64-linux"
fetch "https://github.com/bytecodealliance/wasmtime/releases/download/v$WASMTIME_VERSION/$wt.tar.xz" \
  "$wt.tar.xz" "$WASMTIME_SHA256"
tar -xJf "$wt.tar.xz"
export PATH="$WORK/$wt:$PATH"
wasmtime --version

say "Building (CPython's Tools/wasm/wasi)"
cd "$SRC"
mkdir -p "$WORK/logs"
python3 Tools/wasm/wasi build --quiet --logdir "$WORK/logs" || {
  echo "Build failed; the last lines of each log:" >&2
  for f in "$WORK"/logs/*; do echo "--- $f" >&2; tail -40 "$f" >&2; done
  exit 1
}
triple="wasm32-wasip1"
wasm="cross-build/$triple/python.wasm"
[ -f "$wasm" ] || { echo "No $wasm after the build." >&2; exit 1; }

say "Packaging"
# The same layout as CPython's WASI release builds: python.wasm, the
# standard library under lib/python3.X, LICENSE.
minor="python${PY_VERSION%.*}"
pkg="$WORK/pkg"
rm -rf "$pkg"
mkdir -p "$pkg/lib/$minor/lib-dynload"
cp "$wasm" "$pkg/"
cp -r Lib/. "$pkg/lib/$minor/"
cp cross-build/$triple/build/lib.*/_sysconfigdata_*.py "$pkg/lib/$minor/"
cp LICENSE "$pkg/"
(
  cd "$pkg/lib/$minor"
  # Not usable or not needed in a sandbox without a terminal, threads or
  # pip: GUI, curses, test suites, ensurepip, venv, multiprocessing.
  rm -rf curses ensurepip idlelib test tkinter turtledemo venv multiprocessing \
    unittest/test ctypes/test
  find . -name __pycache__ -prune -exec rm -rf {} +
)
find "$pkg" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +
zip_out="$OUT/python-wasi.zip"
rm -f "$zip_out"
(cd "$pkg" && find . -type f -o -type d | sed 's|^\./||' | grep -v '^\.$' | LC_ALL=C sort |
  zip -X -q -9 "$zip_out" -@)
(cd "$OUT" && sha256sum python-wasi.zip > python-wasi.zip.sha256)

say "Smoke test"
check="$WORK/check"
rm -rf "$check" && mkdir -p "$check" && (cd "$check" && unzip -q "$zip_out")
wasmtime run --dir "$check::/" --env PYTHONHOME=/ "$check/python.wasm" -c \
  "import sys, json, re, zoneinfo, html.parser, hashlib, pathlib, fnmatch, urllib.parse; assert sys.platform == 'wasi'; print(sys.version)" \
  2>&1 | tee "$WORK/smoke.txt"
grep -q "^$PY_VERSION" "$WORK/smoke.txt"

cat > "$OUT/build-info.txt" <<EOF
CPython $PY_VERSION for WASI ($triple), built by scripts/build-wasi-python.sh
source:    https://www.python.org/ftp/python/$PY_VERSION/$tgz
           sha256 $PY_SHA256, Sigstore-verified as $PY_SIGNER ($PY_SIGNER_ISSUER)
wasi-sdk:  $WASI_SDK_VERSION ($WASI_SDK_SHA256)
wasmtime:  $WASMTIME_VERSION ($WASMTIME_SHA256)
result:    $(cut -d' ' -f1 "$OUT/python-wasi.zip.sha256")  python-wasi.zip ($(stat -c %s "$zip_out") bytes)
EOF
say "Done"
cat "$OUT/build-info.txt"
