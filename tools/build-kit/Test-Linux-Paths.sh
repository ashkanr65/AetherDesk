#!/usr/bin/env bash
set -Eeuo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
runner="$script_dir/Run-Linux-Step.sh"
test_root=$(mktemp -d "${TMPDIR:-/tmp}/aetherdesk-linux-paths.XXXXXX")
cleanup() {
  local resolved parent
  resolved=$(realpath -e -- "$test_root")
  parent=$(realpath -e -- "${TMPDIR:-/tmp}")
  [[ "$resolved" == "$parent"/aetherdesk-linux-paths.* ]] || return 1
  rm -rf -- "$resolved"
}
trap cleanup EXIT

checkout="$test_root/moved Aether/AetherDesk-localbuild"
mkdir -p -- "$checkout/.git" "$test_root/unmanaged/.git" "$test_root/outside"
printf '[]\n' > "$checkout/.aetherdesk-source-files.json"
ln -s -- "$test_root/outside" "$checkout/escape"

expect_rejection() {
  local repository=$1 destination=$2 expected=$3 result code
  # An unknown mode can reach validation but cannot invoke any build tools.
  if result=$(bash "$runner" path-test-only "$repository" "$destination" 2>&1); then
    echo 'Path test unexpectedly ran successfully.' >&2
    exit 1
  else
    code=$?
  fi
  [[ "$code" == 64 && "$result" == "$expected" ]] || {
    printf 'Expected exit 64 and %s; got exit %s and %s\n' "$expected" "$code" "$result" >&2
    exit 1
  }
}

expect_rejection "$checkout" "$checkout/bundle-new" 'Unknown Linux step.'
expect_rejection "$checkout" "$checkout/nested/bundle-new" 'Unknown Linux step.'
expect_rejection "$test_root/unmanaged" "$test_root/unmanaged/bundle" 'Unexpected build checkout; no action taken.'
expect_rejection relative-checkout "$checkout/bundle" 'Build checkout must be an absolute path.'
expect_rejection / /bundle 'Unexpected build checkout; no action taken.'
expect_rejection "$checkout" "$test_root/outside/bundle" 'Output must be inside the build checkout.'
expect_rejection "$checkout" "$checkout/escape/bundle" 'Output must be inside the build checkout.'
expect_rejection "$checkout" "$checkout/../AetherDesk-localbuild-extra/bundle" 'Output must be inside the build checkout.'
expect_rejection "$checkout" relative-output 'Output must be an absolute path.'
expect_rejection "$checkout" "$checkout" 'Output must be inside the build checkout.'
echo 'PASS: moved Linux checkout, paths with spaces and unsafe output rejection (no build tools run).'
