#!/usr/bin/env bash
set -euo pipefail

root=$(git rev-parse --show-toplevel)
version=$(cd "$root" && script/get-crate-version zed)
sha=$(git -C "$root" rev-parse HEAD)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/zed-custom-release-regressions.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
real_mktemp=$(command -v mktemp)
smoke="$root/.github/scripts/linux-gui-smoke.sh"
bash -n "$smoke"
mkdir -p "$scratch/gui-bin" "$scratch/gui-fixture/data" "$scratch/gui-diagnostics"
cat > "$scratch/gui-bin/xwininfo" <<'STUB'
#!/bin/sh
printf '0x123 "smoke.txt": ("dev.zed.Zed-Dev" "dev.zed.Zed-Dev") 1280x800+0+0\n'
STUB
cat > "$scratch/gui-bin/openbox" <<'STUB'
#!/bin/sh
: > "$FAKE_WM_MARKER"
exec /bin/sleep 60
STUB
cat > "$scratch/gui-bin/xdotool" <<'STUB'
#!/bin/sh
[ "$1" = windowactivate ] && [ "$2" = --sync ] && [ "$3" = 0x123 ] || exit 2
: > "$FAKE_FOCUS_MARKER"
STUB
cat > "$scratch/gui-bin/import" <<'STUB'
#!/bin/sh
[ "$1" = -window ] && [ "$2" = 0x123 ] || exit 2
: > "$3"
STUB
cat > "$scratch/gui-bin/identify" <<'STUB'
#!/bin/sh
[ "$1" = -format ] && [ "$2" = %k ] || exit 2
count=0
[ ! -f "$FAKE_COLOR_COUNT_FILE" ] || IFS= read -r count < "$FAKE_COLOR_COUNT_FILE"
count=$((count + 1))
printf '%s\n' "$count" > "$FAKE_COLOR_COUNT_FILE"
if [ "$count" -ge "$FAKE_VISIBLE_AFTER" ]; then printf '1010\n'; else printf '1\n'; fi
STUB
cat > "$scratch/gui-bin/zed" <<'STUB'
#!/bin/sh
exec /bin/sleep 60
STUB
cat > "$scratch/gui-bin/sleep" <<'STUB'
#!/bin/sh
exit 0
STUB
chmod +x "$scratch/gui-bin"/*
smoke_env=(PATH="$scratch/gui-bin:$PATH" ZED_SMOKE_BINARY="$scratch/gui-bin/zed" ZED_SMOKE_FIXTURE="$scratch/gui-fixture" ZED_SMOKE_DIAGNOSTICS="$scratch/gui-diagnostics" FAKE_COLOR_COUNT_FILE="$scratch/gui-diagnostics/color-count" FAKE_WM_MARKER="$scratch/gui-diagnostics/wm-started" FAKE_FOCUS_MARKER="$scratch/gui-diagnostics/window-focused")
if env "${smoke_env[@]}" FAKE_VISIBLE_AFTER=999 bash "$smoke" > "$scratch/blank-smoke.log" 2>&1; then
  echo 'blank GUI screenshot passed smoke validation' >&2
  exit 1
fi
grep -q 'no visible smoke.txt window after 90 attempts' "$scratch/blank-smoke.log"
rm -f "$scratch/gui-diagnostics/color-count" "$scratch/gui-diagnostics/wm-started" "$scratch/gui-diagnostics/window-focused"
env "${smoke_env[@]}" FAKE_VISIBLE_AFTER=3 bash "$smoke"
[[ -f "$scratch/gui-diagnostics/wm-started" && -f "$scratch/gui-diagnostics/window-focused" ]] || { echo 'GUI smoke did not activate the desktop window' >&2; exit 1; }

git clone --shared --no-checkout "$root" "$scratch/repo" >/dev/null
git -C "$scratch/repo" checkout --detach "$sha" >/dev/null
cp "$root/.github/scripts/custom-release.sh" "$scratch/repo/.github/scripts/custom-release.sh"
mkdir -p "$scratch/bin"

cat > "$scratch/bin/uname" <<'STUB'
#!/bin/sh
case "${FAKE_PLATFORM:?}:$1" in
  darwin:-m) printf 'arm64\n' ;;
  darwin:-s) printf 'Darwin\n' ;;
  linux:-m) printf 'aarch64\n' ;;
  linux:-s) printf 'Linux\n' ;;
  *) exit 2 ;;
esac
STUB
cat > "$scratch/bin/df" <<'STUB'
#!/bin/sh
printf 'Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/fake 100000000 1 99999999 1%% /\n'
STUB
cat > "$scratch/bin/sudo" <<'STUB'
#!/bin/sh
exit 0
STUB
cat > "$scratch/bin/rustup" <<'STUB'
#!/bin/sh
[ "${1:-}" = which ] || exit 2
printf '/usr/bin/true\n'
STUB
cat > "$scratch/bin/cargo" <<'STUB'
#!/bin/sh
if [ "${1:-}" = metadata ]; then
  printf '{"packages":[{"name":"zed","version":"%s"}]}\n' "$FAKE_VERSION"
fi
exit 0
STUB
cat > "$scratch/bin/hdiutil" <<'STUB'
#!/bin/sh
[ "${1:-}" = create ] || exit 2
for output do :; done
: > "$output"
STUB
cat > "$scratch/bin/mktemp" <<'STUB'
#!/bin/sh
if [ "$FAKE_PLATFORM" = darwin ]; then
  count=0
  [ ! -f "$TEST_ROOT/mktemp-count" ] || read -r count < "$TEST_ROOT/mktemp-count"
  count=$((count + 1))
  printf '%s\n' "$count" > "$TEST_ROOT/mktemp-count"
  if [ "$count" = 1 ]; then exec "$REAL_MKTEMP" -d; fi
fi
# Deliberately stop after platform dispatch, before installer/GUI smoke.
exit 42
STUB
cat > "$scratch/bin/bash" <<'STUB'
#!/bin/bash
set -e
case "${1:-}" in
  script/bundle-mac)
    mkdir -p 'target/aarch64-apple-darwin/release/dmg/Zed Dev.app'
    printf 'stub remote server\n' | gzip -c > target/zed-remote-server-macos-aarch64.gz
    exit 0 ;;
  script/linux)
    : > "$TEST_ROOT/linux-build-called"
    exit 0 ;;
  script/bundle-linux)
    mkdir -p target
    printf 'stub remote server\n' | gzip -c > target/zed-remote-server-linux-aarch64.gz
    exit 0 ;;
esac
exec /bin/bash "$@"
STUB
chmod +x "$scratch"/bin/*

run_platform() {
  local platform="$1" expect_linux="$2" status
  mkdir -p "$scratch/$platform"
  set +e
  (
    cd "$scratch/repo"
    PATH="$scratch/bin:$PATH" FAKE_PLATFORM="$platform" FAKE_VERSION="$version" TEST_ROOT="$scratch/$platform" \
      REAL_MKTEMP="$real_mktemp" /bin/bash .github/scripts/custom-release.sh \
      build "$platform" "v${version}-custom.1" "$sha" "$scratch/$platform/output"
  )
  status=$?
  set -e
  [[ $status == 42 ]] || { printf '%s dispatch stopped unexpectedly (exit %s)\n' "$platform" "$status" >&2; return 1; }
  if [[ $expect_linux == yes ]]; then
    [[ -f "$scratch/$platform/linux-build-called" ]] || { echo 'Linux runner skipped its build branch' >&2; return 1; }
  else
    [[ ! -e "$scratch/$platform/linux-build-called" ]] || { echo 'Darwin runner entered the Linux build branch' >&2; return 1; }
  fi
}

run_platform darwin no
run_platform linux yes
printf 'Zed Darwin and Linux build branches dispatch independently\n'
