#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <ad-agent> <output-directory>" >&2
  exit 64
fi

agent="$(readlink -f -- "$1")"
output="$(readlink -m -- "$2")"
[[ -x "$agent" ]] || { echo "agent is not executable: $agent" >&2; exit 66; }
workspace="$(pwd -P)"
case "$output" in
  "$workspace"/*) ;;
  *) echo "output directory must be inside $workspace" >&2; exit 64 ;;
esac
[[ "$output" != "$workspace" && "$output" != "/" ]] || {
  echo "refusing unsafe output directory: $output" >&2
  exit 64
}

rm -rf -- "$output"
mkdir -p -- "$output/libs"
install -m 700 -- "$agent" "$output/ad-agent"

declare -A seen=()
seen["ad-agent"]=1
queue=("$agent")

for element in queue capsfilter appsrc appsink videoconvert h264parse av1parse x264enc openh264enc \
  avenc_h264 svtav1enc vah264enc vaapih264enc nvh264enc v4l2h264enc; do
  plugin="$(gst-inspect-1.0 "$element" 2>/dev/null | awk '$1 == "Filename" { print $2 }' || true)"
  [[ -f "$plugin" ]] && queue+=("$plugin")
done

scanner="$(find /usr/lib /usr/lib64 /usr/libexec -type f -name gst-plugin-scanner -print -quit 2>/dev/null || true)"
if [[ -n "$scanner" ]]; then
  install -m 500 -- "$scanner" "$output/libs/gst-plugin-scanner"
  seen["gst-plugin-scanner"]=1
  queue+=("$output/libs/gst-plugin-scanner")
fi

xwayland="$(command -v Xwayland || true)"
if [[ -n "$xwayland" ]]; then
  install -m 500 -- "$xwayland" "$output/libs/Xwayland"
  seen["Xwayland"]=1
  queue+=("$output/libs/Xwayland")
  license_dir="$(dirname -- "$(dirname -- "$xwayland")")/share/licenses/aetherdesk-xwayland"
  if [[ -d "$license_dir" ]]; then
    mkdir -p "$output/licenses"
    cp -a -- "$license_dir" "$output/licenses/"
  fi
fi

while ((${#queue[@]})); do
  binary="${queue[0]}"
  queue=("${queue[@]:1}")
  source_name="$(basename -- "$binary")"
  if [[ -z "${seen[$source_name]:-}" ]]; then
    seen["$source_name"]=1
    install -m 500 -- "$binary" "$output/libs/$source_name"
    binary="$output/libs/$source_name"
  fi
  while IFS= read -r dependency; do
    [[ -f "$dependency" ]] || continue
    name="$(basename -- "$dependency")"
    [[ -n "${seen[$name]:-}" ]] && continue
    case "$name" in
      linux-vdso.so.*|ld-linux-*.so.*|libc.so.*|libm.so.*|libpthread.so.*|libdl.so.*|librt.so.*|libgcc_s.so.*)
        continue
        ;;
    esac
    seen["$name"]=1
    install -m 500 -- "$dependency" "$output/libs/$name"
    queue+=("$output/libs/$name")
  done < <(ldd -- "$binary" | awk '/=> \/|^\// { for (i=1;i<=NF;i++) if ($i ~ /^\//) { print $i; break } }')
done

python3 - "$output" <<'PY'
import hashlib
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
files = []
for path in sorted((root / "libs").iterdir(), key=lambda item: item.name):
    data = path.read_bytes()
    files.append({
        "file_name": path.name,
        "size": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
    })
manifest = {"schema_version": 1, "files": files}
(root / "runtime-manifest.json").write_text(
    json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
)
PY

env -i HOME="${HOME:-/tmp}" PATH="$output/libs:/usr/bin:/bin" \
  LD_LIBRARY_PATH="$output/libs" \
  GST_PLUGIN_PATH="$output/libs" \
  GST_PLUGIN_SYSTEM_PATH_1_0="$output/libs" \
  GST_PLUGIN_SCANNER="$output/libs/gst-plugin-scanner" \
  "$output/ad-agent" --version

# Verify required elements with system plugins disabled, so an incomplete bundle
# cannot pass only because the build machine happens to have the missing plugin.
for element in queue capsfilter appsrc appsink videoconvert h264parse x264enc; do
  env LD_LIBRARY_PATH="$output/libs" GST_PLUGIN_PATH="$output/libs" \
    GST_PLUGIN_SYSTEM_PATH_1_0="$output/libs" \
    GST_PLUGIN_SCANNER="$output/libs/gst-plugin-scanner" \
    GST_REGISTRY="$output/.registry-check.bin" \
    gst-inspect-1.0 "$element" >/dev/null
done
rm -f -- "$output/.registry-check.bin"

echo "created verified AetherDesk agent bundle at $output"
