#!/usr/bin/env bash
# Removes the graphics-stack libraries from a built AppImage, so the app uses
# the system's copies, the same ones its graphics driver was built against.
#
#   scripts/appimage-unbundle-graphics.sh path/to/Chummy.AppImage   (repacks it)
#   scripts/appimage-unbundle-graphics.sh --dir path/to/squashfs-root
#
# Why: linuxdeploy bundles everything WebKitGTK links to. The system's Mesa
# (libgallium, libEGL_mesa) loads at run time and needs many of the same
# libraries. With our older copies already loaded, Mesa runs against them:
# v0.1.2 on Debian with Mesa 26.2 crashed WebKitWebProcess at exit with heap
# corruption inside libgallium. Leaving these libraries to the system is
# standard practice (AppImage's excludelist does it for libGL, libdrm,
# libgbm, libxcb, libX11, libwayland-client); this goes on to the rest of what
# Mesa shares. Every desktop that has Mesa has all of them.
#
# Repacking needs appimagetool (APPIMAGETOOL, default: appimagetool on PATH).
set -euo pipefail

GRAPHICS_LIBS=(
  # Wayland (linuxdeploy already drops libwayland-client)
  'libwayland-*.so*'
  # X11 and xcb: libxcb's own dependencies and the extensions Mesa's GLX and
  # DRI3 code load
  'libX11.so*' 'libX11-xcb.so*' 'libXau.so*' 'libXdmcp.so*'
  'libXext.so*' 'libXfixes.so*' 'libXdamage.so*' 'libXxf86vm.so*'
  'libxcb*.so*' 'libxshmfence.so*'
  # The driver itself and its loaders
  'libGL.so*' 'libGLX*.so*' 'libGLdispatch.so*' 'libEGL*.so*' 'libOpenGL.so*'
  'libglapi.so*' 'libgbm.so*' 'libdrm*.so*' 'libvulkan.so*'
  # What Mesa links to besides: shader cache compression, the ELF reader,
  # device discovery, libffi (for libwayland), driconf's XML parser, zlib
  'libzstd.so*' 'libelf.so*' 'libffi.so*' 'libudev.so*' 'libsystemd.so*'
  'libexpat.so*' 'libz.so*' 'libLLVM*.so*' 'libsensors.so*'
)

strip_dir() {
  local lib="$1/usr/lib" removed=0
  [ -d "$lib" ] || { echo "no usr/lib in $1" >&2; exit 1; }
  shopt -s nullglob
  for pattern in "${GRAPHICS_LIBS[@]}"; do
    for f in "$lib"/$pattern; do
      echo "removing ${f#"$1"/}"
      rm -f "$f"
      removed=$((removed + 1))
    done
  done
  echo "removed $removed libraries"
}

if [ "${1:-}" = --dir ]; then
  strip_dir "$2"
  exit 0
fi

image="$(readlink -f "$1")"
tool="${APPIMAGETOOL:-appimagetool}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

(cd "$work" && "$image" --appimage-extract >/dev/null)
strip_dir "$work/squashfs-root"
# Keep the runtime (the launcher before the squashfs) the original came with,
# so what users need to start it is unchanged and nothing else is downloaded.
offset="$("$image" --appimage-offset)"
head -c "$offset" "$image" > "$work/runtime"
# appimagetool is itself an AppImage; extract-and-run spares the runner FUSE.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$tool" --no-appstream \
  --runtime-file "$work/runtime" "$work/squashfs-root" "$work/out.AppImage"
mv "$work/out.AppImage" "$image"
chmod +x "$image"
echo "repacked $image"
