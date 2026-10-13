#!/usr/bin/env bash
set -Eeuo pipefail
trap 'printf "\nLINUX STEP FAILED at line %s. Stop and keep the log.\n" "$LINENO" >&2' ERR

mode=${1:?missing mode}
repo=${2:?missing build checkout}
output=${3:?missing output path}
# The Windows launcher supplies the generated checkout after translating it
# with wslpath. Validate that checkout instead of pinning a user's old folder.
[[ "$repo" == /* ]] || { echo 'Build checkout must be an absolute path.' >&2; exit 64; }
repo=$(realpath -e -- "$repo") || { echo 'Build checkout does not exist.' >&2; exit 64; }
[[ "$repo" != / && -d "$repo/.git" && -f "$repo/.aetherdesk-source-files.json" ]] || {
  echo 'Unexpected build checkout; no action taken.' >&2; exit 64;
}
[[ "$output" == /* ]] || { echo 'Output must be an absolute path.' >&2; exit 64; }
# Resolve existing symlinks even when the final output directory is new. A
# lexical prefix check alone could allow output through a link outside it.
output=$(realpath -m -- "$output") || { echo 'Invalid output path.' >&2; exit 64; }
[[ "$output" == "$repo"/* ]] || { echo 'Output must be inside the build checkout.' >&2; exit 64; }
cd "$repo"

# Never use Windows SDK paths or targets when running the Linux compiler.
unset PKG_CONFIG PKG_CONFIG_PATH GSTREAMER_ROOT CARGO_TARGET_DIR
case "$mode" in
  bundle)
    [[ $(id -u) == 0 ]] || { echo 'The bundle step needs the configured WSL root user.' >&2; exit 64; }
    [[ ! -e "$output" ]] || { echo 'Output already exists. Rerun the step for a fresh output folder.' >&2; exit 64; }
    # WSL direct execution can omit the system administration tool paths.
    export PATH="/usr/sbin:/sbin:$PATH"
    command -v nft >/dev/null || {
      echo 'Podman needs the nftables package in the existing Ubuntu distribution. No new Ubuntu installation is needed.' >&2
      exit 69
    }
    podman --storage-driver=vfs build --layers \
      --from docker.io/library/almalinux:8 --platform linux/amd64 \
      --target artifact --output "type=local,dest=$output" \
      -f agent/linux/Dockerfile.bundle .
    test -s "$output/ad-agent"
    test -s "$output/runtime-manifest.json"
    test -s "$output/libs/Xwayland"
    python3 - "$output" <<'PY'
import hashlib, json, pathlib, sys
root = pathlib.Path(sys.argv[1])
manifest = json.loads((root / 'runtime-manifest.json').read_text())
if manifest.get('schema_version') != 1 or not manifest.get('files'):
    raise SystemExit('Invalid or empty agent runtime manifest')
for item in manifest['files']:
    name = item['file_name']
    if pathlib.Path(name).name != name or name in ('.', '..'):
        raise SystemExit('Invalid filename in manifest')
    data = (root / 'libs' / name).read_bytes()
    if len(data) != item['size'] or hashlib.sha256(data).hexdigest() != item['sha256']:
        raise SystemExit('Agent runtime checksum mismatch: ' + name)
print('Linux agent files and runtime checksums verified.')
PY
    ;;
  checks)
    [[ $(id -u) != 0 ]] || { echo 'Linux tests must use the normal Ubuntu user.' >&2; exit 64; }
    export PATH="/opt/aetherdesk-xwayland/bin:$HOME/.cargo/bin:$PATH"
    export LD_LIBRARY_PATH=/opt/aetherdesk-xwayland/lib
    export CARGO_TARGET_DIR="$HOME/.cache/aetherdesk-local-target"
    /opt/aetherdesk-xwayland/bin/Xwayland -version
    python3 tools/test_x11_icons.py
    cd core/rust
    cargo check --locked --workspace
    cargo test --locked -p ad-vpn -p ad-ssh
    packages=(-p agent-wayland -p agent-input -p agent-encoder -p agent-supervisor
      -p ad-stream-protocol -p ad-qos-controller -p ad-display-decoder
      -p ad-display-renderer -p ad-stream-client -p ad-remote-session)
    features=ad-stream-client/display-stream,ad-remote-session/rendering
    cargo fmt --check "${packages[@]}"
    cargo clippy --locked "${packages[@]}" --features "$features" --all-targets --no-deps -- -D warnings
    cargo test --locked "${packages[@]}" --features "$features" -- --test-threads=1
    cargo build --locked --release -p agent-supervisor -p ad-stream-client \
      -p ad-display-renderer --features ad-stream-client/display-stream
    "$CARGO_TARGET_DIR/release/ad-agent" --version
    cd "$repo"
    bash tools/package-linux-agent.sh "$CARGO_TARGET_DIR/release/ad-agent" "$output"
    LD_LIBRARY_PATH="$output/libs" ldd "$output/ad-agent" > "$output/dependency-check.txt"
    cat "$output/dependency-check.txt"
    if grep -q 'not found' "$output/dependency-check.txt"; then
      echo 'A Linux runtime dependency is missing.' >&2; exit 1
    fi
    ;;
  *) echo 'Unknown Linux step.' >&2; exit 64 ;;
esac
