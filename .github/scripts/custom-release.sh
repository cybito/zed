#!/usr/bin/env bash
set -euo pipefail
[[ $# == 5 && $1 == build ]] || { echo 'usage: build darwin|linux TAG SHA ABS_OUTPUT' >&2; exit 2; }
platform=$2 tag=$3 sha=$4 out=$5
[[ $out == /* && $sha == "$(git rev-parse HEAD)" && $(cat crates/zed/RELEASE_CHANNEL) == dev ]]
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+-custom\.[1-9][0-9]*$ ]]
[[ ${tag#v} == "$(script/get-crate-version zed)"-custom.* ]]
[[ $(uname -m) == arm64 || $(uname -m) == aarch64 ]]
mkdir -p "$out" "$out/diagnostics"
export RUSTUP_TOOLCHAIN=1.97.1 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_LTO=false CARGO_BUILD_JOBS=2
export RUSTC="$(rustup which --toolchain 1.97.1 rustc)" RUSTDOC="$(rustup which --toolchain 1.97.1 rustdoc)"
# Reclaim only disposable hosted-runner preinstalled SDKs, never checkout resources.
if [[ $platform == darwin ]]; then
  available=$(df -Pk . | awk 'NR==2 {print $4}')
  if (( available < 26214400 )); then
    sudo rm -rf /usr/local/share/dotnet /opt/homebrew/share/dotnet /Users/runner/Library/Android/sdk
  fi
  (( $(df -Pk . | awk 'NR==2 {print $4}') >= 26214400 )) || { echo 'Zed requires 25 GiB free' >&2; exit 1; }
  cargo +1.97.1 install cargo-bundle --git https://github.com/zed-industries/cargo-bundle.git --rev 2be2669972dff3ddd4daf89a2cb29d2d06cad7c7 --locked
  CARGO_BUNDLE_SKIP_BUILD=true bash script/bundle-mac aarch64-apple-darwin
  staging=$(mktemp -d)
  cp -R target/aarch64-apple-darwin/release/dmg/*.app "$staging/"
  ln -s /Applications "$staging/Applications"
  cp LICENSE* README.md "$staging/"
  cat > "$staging/install.sh" <<'INSTALL'
#!/bin/sh
set -eu
prefix=${HOME}/.local
if [ "$#" -gt 0 ]; then
  [ "$#" -eq 2 ] && [ "$1" = --prefix ] || exit 2
  prefix=$2
fi
case "$prefix" in /*) ;; *) echo 'prefix must be absolute' >&2; exit 2;; esac
python3 - "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)" "$prefix" <<'PY'
from pathlib import Path
import os, shutil, sys
root, prefix = map(Path, sys.argv[1:])
app, = root.glob('*.app')
destination = prefix / 'Applications' / app.name
for parent in destination.parents:
    if parent == prefix.parent: break
    if parent.is_symlink() or (parent.exists() and not parent.is_dir()): raise SystemExit('unsafe prefix parent')
entries = [p for p in app.rglob('*') if p.is_file() or p.is_symlink()]
if destination.exists():
    source_names = {str(p.relative_to(app)) for p in entries}
    target_names = {str(p.relative_to(destination)) for p in destination.rglob('*') if p.is_file() or p.is_symlink()}
    if source_names != target_names: raise SystemExit('refusing existing different app')
    for p in entries:
        target = destination / p.relative_to(app)
        if p.is_symlink():
            if not target.is_symlink() or os.readlink(p) != os.readlink(target): raise SystemExit('refusing different app link')
        elif target.is_symlink() or target.read_bytes() != p.read_bytes(): raise SystemExit('refusing different app bytes')
else:
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(app, destination, symlinks=True)
print(destination)
PY
INSTALL
  chmod +x "$staging/install.sh"
  hdiutil create -volname 'Zed Dev Custom' -srcfolder "$staging" -ov -format UDZO "$out/zed-$tag-darwin-arm64.dmg"
  rm -rf "$staging"
  cp target/zed-remote-server-macos-aarch64.gz "$out/zed-remote-server-$tag-darwin-arm64.gz"
else
  available=$(df -Pk . | awk 'NR==2 {print $4}')
  if (( available < 26214400 )); then
    sudo rm -rf /usr/share/dotnet /usr/local/share/powershell /usr/local/lib/android/sdk
  fi
  (( $(df -Pk . | awk 'NR==2 {print $4}') >= 26214400 )) || { echo 'Zed requires 25 GiB free' >&2; exit 1; }
  bash script/linux
  sudo apt-get install -y llvm desktop-file-utils xvfb xauth dbus-x11 mesa-vulkan-drivers libgl1-mesa-dri imagemagick x11-utils
  REMOTE_SERVER_TARGET=aarch64-unknown-linux-gnu bash script/bundle-linux
  cp target/zed-remote-server-linux-aarch64.gz "$out/zed-remote-server-$tag-linux-arm64.gz"
fi
fixture=$(mktemp -d)
trap 'if [[ ${mounted:-false} == true ]]; then hdiutil detach "$fixture/mount"; fi; rm -rf "$fixture"' EXIT
mkdir -p "$fixture/home" "$fixture/config" "$fixture/data"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export HOME="$fixture/home" XDG_CONFIG_HOME="$fixture/config" XDG_DATA_HOME="$fixture/data" XDG_CACHE_HOME="$fixture/cache"
printf 'custom-ci-ok\n' > "$fixture/smoke.txt"
if [[ $platform == darwin ]]; then
  mkdir "$fixture/mount"
  hdiutil attach "$out/zed-$tag-darwin-arm64.dmg" -readonly -nobrowse -mountpoint "$fixture/mount"
  mounted=true
  app=$(find "$fixture/mount" -maxdepth 1 -name '*.app' -print)
  [[ $(basename "$app") == 'Zed Dev.app' || $(basename "$app") == 'Zed Devel.app' ]]
  "$fixture/mount/install.sh" --prefix "$fixture/prefix"
  "$fixture/mount/install.sh" --prefix "$fixture/prefix"
  app=$(find "$fixture/prefix/Applications" -maxdepth 1 -name '*.app' -print)
  codesign --verify --deep --strict "$app"
  binary="$app/Contents/MacOS/zed"
  "$app/Contents/MacOS/cli" --version | tee "$out/diagnostics/version.txt"
  "$binary" --help > "$out/diagnostics/help.txt"
  grep -q -- '--user-data-dir' "$out/diagnostics/help.txt"
  "$binary" --user-data-dir "$fixture/data" "$fixture/smoke.txt" > "$out/diagnostics/gui.log" 2>&1 &
  pid=$!
  # CoreGraphics window enumeration is evidence of a visible native window, not a process check.
  cat > "$fixture/window.swift" <<'SWIFT'
import Foundation
import CoreGraphics
let pid = Int(CommandLine.arguments[1])!
let destination = CommandLine.arguments[2]
for _ in 0..<90 {
    let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    for window in windows where window[kCGWindowOwnerPID as String] as? Int == pid {
        let title = window[kCGWindowName as String] as? String ?? ""
        if title.contains("smoke.txt") {
            print(title)
            let id = window[kCGWindowNumber as String] as! UInt32
            let task = Process()
            task.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
            task.arguments = ["-x", "-l", String(id), destination]
            try task.run(); task.waitUntilExit()
            exit(task.terminationStatus)
        }
    }
    Thread.sleep(forTimeInterval: 1)
}
fputs("No smoke.txt window observed\n", stderr)
exit(1)
SWIFT
  swift "$fixture/window.swift" "$pid" "$out/diagnostics/window.png" > "$out/diagnostics/window.txt"
  kill "$pid"
else
  mkdir "$fixture/package"
  tar -xzf target/release/zed-linux-aarch64.tar.gz -C "$fixture/package"
  cp LICENSE* README.md "$fixture/package/"
  cat > "$fixture/package/install.sh" <<'INSTALL'
#!/bin/sh
set -eu
prefix=${HOME}/.local
if [ "$#" -gt 0 ]; then
  [ "$#" -eq 2 ] && [ "$1" = --prefix ] || exit 2
  prefix=$2
fi
case "$prefix" in /*) ;; *) echo 'prefix must be absolute' >&2; exit 2;; esac
python3 - "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)" "$prefix" <<'PY'
from pathlib import Path
import os, shutil, sys
root, prefix = map(Path, sys.argv[1:])
app = root / 'zed-dev.app'
entries = []
for source in sorted(app.rglob('*')):
    if source.is_dir() and not source.is_symlink(): continue
    entries.append((source, prefix / 'lib/zed-dev/zed-dev.app' / source.relative_to(app)))
for source in sorted((app / 'share').rglob('*')):
    if source.is_file() or source.is_symlink(): entries.append((source, prefix / 'share' / source.relative_to(app / 'share')))
link = prefix / 'bin/zed'
link_target = '../lib/zed-dev/zed-dev.app/bin/zed'
for parent in link.parents:
    if parent == prefix.parent: break
    if parent.is_symlink() or (parent.exists() and not parent.is_dir()): raise SystemExit('unsafe link parent: ' + str(parent))
for source, destination in entries:
    for parent in destination.parents:
        if parent == prefix.parent: break
        if parent.is_symlink() or (parent.exists() and not parent.is_dir()): raise SystemExit('unsafe parent: ' + str(parent))
    if destination.exists() or destination.is_symlink():
        same = destination.is_symlink() and source.is_symlink() and os.readlink(destination) == os.readlink(source)
        same |= not destination.is_symlink() and not source.is_symlink() and destination.is_file() and destination.read_bytes() == source.read_bytes()
        if not same: raise SystemExit('refusing overwrite: ' + str(destination))
if link.exists() or link.is_symlink():
    if not link.is_symlink() or os.readlink(link) != link_target: raise SystemExit('refusing overwrite: ' + str(link))
for source, destination in entries:
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists() or destination.is_symlink(): continue
    if source.is_symlink(): destination.symlink_to(os.readlink(source))
    else: shutil.copy2(source, destination)
link.parent.mkdir(parents=True, exist_ok=True)
if not link.is_symlink(): link.symlink_to(link_target)
PY
INSTALL
  chmod +x "$fixture/package/install.sh"
  tar -czf "$out/zed-$tag-linux-arm64.tar.gz" -C "$fixture/package" .
  "$fixture/package/install.sh" --prefix "$fixture/prefix"
  "$fixture/package/install.sh" --prefix "$fixture/prefix"
  app="$fixture/prefix/lib/zed-dev/zed-dev.app"
  binary="$app/libexec/zed-editor"
  "$fixture/prefix/bin/zed" --version | tee "$out/diagnostics/version.txt"
  ldd "$binary" > "$out/diagnostics/ldd.txt"
  ! grep -q 'not found' "$out/diagnostics/ldd.txt"
  ! ldd target/aarch64-unknown-linux-gnu/release/remote_server | grep -Eq 'libssl|libcrypto'
  desktop-file-validate "$fixture/prefix/share/applications/"*.desktop
  "$binary" --help > "$out/diagnostics/help.txt"
  grep -q -- '--user-data-dir' "$out/diagnostics/help.txt"
  export ZED_SMOKE_BINARY="$binary" ZED_SMOKE_FIXTURE="$fixture" ZED_SMOKE_DIAGNOSTICS="$out/diagnostics"
  VK_DRIVER_FILES=$(python3 -c 'import glob; files=glob.glob("/usr/share/vulkan/icd.d/*lvp*.json"); assert len(files)==1; print(files[0])')
  export VK_DRIVER_FILES
  dbus-run-session -- xvfb-run -a -s '-screen 0 1280x800x24' bash -eu -c '
    export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe XDG_SESSION_TYPE=x11
    unset WAYLAND_DISPLAY
    "$ZED_SMOKE_BINARY" --user-data-dir "$ZED_SMOKE_FIXTURE/data" "$ZED_SMOKE_FIXTURE/smoke.txt" > "$ZED_SMOKE_DIAGNOSTICS/gui.log" 2>&1 &
    pid=$!
    trap "kill $pid 2>/dev/null || true" EXIT
    for attempt in {1..90}; do
      xwininfo -root -tree > "$ZED_SMOKE_DIAGNOSTICS/window.txt"
      if grep -q smoke.txt "$ZED_SMOKE_DIAGNOSTICS/window.txt"; then
        window_id=$(awk '/"smoke.txt"/ { print $1; exit }' "$ZED_SMOKE_DIAGNOSTICS/window.txt")
        [[ -n $window_id ]] || { echo "Smoke window has no X11 ID" >&2; exit 1; }
        import -window "$window_id" "$ZED_SMOKE_DIAGNOSTICS/window.png"
        colors=$(identify -format '%k' "$ZED_SMOKE_DIAGNOSTICS/window.png")
        if [[ ! $colors =~ ^[0-9]+$ ]] || (( colors <= 16 )); then
          echo "Smoke window screenshot is blank ($colors unique colors)" >&2
          exit 1
        fi
        exit 0
      fi
      kill -0 "$pid"
      sleep 1
    done
    echo "No smoke.txt window observed" >&2
    exit 1'
fi
[[ -s $out/diagnostics/window.png ]]
python3 - "$binary" "$platform" "$out/diagnostics/version.txt" "${tag#v}" "$out/zed-remote-server-$tag-$platform-arm64.gz" <<'PY'
from pathlib import Path
import gzip, struct, sys
binary, platform, version_file, version, remote = sys.argv[1:]
for path, opener in [(binary, open), (remote, gzip.open)]:
    with opener(path, 'rb') as file:
        header = file.read(64)
    if platform == 'linux':
        if header[:4] != b'\x7fELF' or header[4:6] != b'\x02\x01' or struct.unpack_from('<H',header,18)[0] != 183:
            raise SystemExit('expected ARM64 ELF: ' + path)
    else:
        if header[:4] != b'\xcf\xfa\xed\xfe' or struct.unpack_from('<I',header,4)[0] != 0x0100000c:
            raise SystemExit('expected ARM64 Mach-O: ' + path)
if version.split('-custom.')[0] not in Path(version_file).read_text():
    raise SystemExit('Zed source version mismatch')
PY
python3 - "$out/toolchains.json" <<'PY'
import json, subprocess, sys
value = {name: subprocess.check_output(command, text=True).strip() for name, command in {'rustc':['rustc','--version'], 'cargo':['cargo','--version']}.items()}
json.dump(value, open(sys.argv[1], 'w'), sort_keys=True)
PY
