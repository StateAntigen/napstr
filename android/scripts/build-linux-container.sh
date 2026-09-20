#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
image="napstr-appimage-builder:ubuntu-22.04"

# Reuse the existing Ubuntu toolchain; only the companion and build caches are
# writable in the container. Napstr and the shared protocol are read-only.
if ! docker image inspect "$image" >/dev/null 2>&1; then
  docker build --file "$project_dir/packaging/appimage.Dockerfile" \
    --tag "$image" "$project_dir/packaging"
fi
mkdir -p "$project_dir/android/src-tauri/target/appimage-container" \
  "$project_dir/.cache/appimage/cargo" "$project_dir/.cache/appimage/npm" \
  "$project_dir/.cache/appimage/tauri"
docker run --rm --user "$(id -u):$(id -g)" \
  --env CARGO_HOME=/workspace/.cache/appimage/cargo \
  --env CARGO_TARGET_DIR=/workspace/android/src-tauri/target/appimage-container \
  --env npm_config_cache=/workspace/.cache/appimage/npm \
  --env XDG_CACHE_HOME=/workspace/.cache/appimage \
  --env APPIMAGE_EXTRACT_AND_RUN=1 --env NO_STRIP=1 \
  --volume "$project_dir:/workspace:ro" \
  --volume "$project_dir/android:/workspace/android" \
  --volume "$project_dir/.cache/appimage:/workspace/.cache/appimage" \
  --workdir /workspace/android "$image" \
  bash -lc 'npm ci && npm run bundle:linux -- -- --locked && bash scripts/postprocess-appimage.sh src-tauri/target/appimage-container/release/bundle/appimage/*.AppImage'
