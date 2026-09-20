#!/usr/bin/env bash
set -euo pipefail

# Napstrfy plays through WebKit/GStreamer; it does not bundle Napstr's Tor or
# native ALSA player. Preserve the generated media hooks and use host graphics.
input="$(realpath "${1:?usage: postprocess-appimage.sh Napstrfy.AppImage}")"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/napstrfy-appimage.XXXXXXXX")"
trap 'rm -rf -- "$work_dir"' EXIT
(
  cd "$work_dir"
  "$input" --appimage-extract >/dev/null
)
app_dir="$work_dir/squashfs-root"
test -x "$app_dir/usr/bin/napstrfy"
test -f "$app_dir/apprun-hooks/linuxdeploy-plugin-gtk.sh"
test -f "$app_dir/apprun-hooks/linuxdeploy-plugin-gstreamer.sh"

# Use the host's Wayland libraries alongside its Mesa drivers, avoiding the
# EGL_BAD_PARAMETER blank window seen with linuxdeploy's bundled versions.
find "$app_dir/usr/lib" -maxdepth 1 \( -type f -o -type l \) -name 'libwayland-*' -delete
if [[ ! -e "$app_dir/usr/lib/libz.so.1" ]]; then
  zlib_path="$(ldconfig -p | awk '$1 == "libz.so.1" { path=$NF } END { print path }')"
  test -f "$zlib_path"
  cp -L "$zlib_path" "$app_dir/usr/lib/libz.so.1"
fi

for element in playbin souphttpsrc audioconvert audioresample autoaudiosink pulsesink mpg123audiodec flacdec vorbisdec opusdec wavparse; do
  env LD_LIBRARY_PATH="$app_dir/usr/lib:$app_dir/usr/lib/x86_64-linux-gnu" \
    GST_REGISTRY_1_0="$work_dir/gstreamer-registry.bin" \
    GST_PLUGIN_SYSTEM_PATH_1_0="$app_dir/usr/lib/gstreamer-1.0" \
    GST_PLUGIN_PATH_1_0="$app_dir/usr/lib/gstreamer-1.0" \
    GST_PLUGIN_SCANNER_1_0="$app_dir/usr/lib/gstreamer1.0/gstreamer-1.0/gst-plugin-scanner" \
    gst-inspect-1.0 "$element" >/dev/null
done

cat > "$app_dir/AppRun" <<'SH'
#!/usr/bin/env bash
set -e
export APPDIR="$(readlink -f "$(dirname "$0")")"
export PATH="$APPDIR/usr/bin:${PATH:-/usr/bin:/bin}"
export XDG_DATA_DIRS="$APPDIR/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
source "$APPDIR/apprun-hooks/linuxdeploy-plugin-gtk.sh"
export LD_LIBRARY_PATH="$APPDIR/usr/lib:$APPDIR/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
source "$APPDIR/apprun-hooks/linuxdeploy-plugin-gstreamer.sh"
cd "$APPDIR/usr"
exec "$APPDIR/usr/bin/napstrfy" "$@"
SH
chmod 0755 "$app_dir/AppRun"
plugin="${TAURI_APPIMAGE_PLUGIN:-${XDG_CACHE_HOME:-$HOME/.cache}/tauri/linuxdeploy-plugin-appimage.AppImage}"
test -x "$plugin"
(
  cd "$work_dir"
  ARCH="${ARCH:-$(uname -m)}" LDAI_OUTPUT="$work_dir/Napstrfy.AppImage" \
    APPIMAGE_EXTRACT_AND_RUN=1 "$plugin" --appdir="$app_dir"
)
test -s "$work_dir/Napstrfy.AppImage"
chmod 0755 "$work_dir/Napstrfy.AppImage"
mv -f "$work_dir/Napstrfy.AppImage" "$input"
echo "Prepared Napstrfy AppImage: $input"
