# Custom release provenance

- GitHub fork: `https://github.com/cybito/zed` (upstream: `https://github.com/zed-industries/zed.git`). `custom` is the custom build and release source; the product remains Zed Dev.
- Historical Forgejo source: `https://git.cybit.top/cybit/zed`; custom revision at migration: `b07b7e820656e3d6abe80cdf92527de943ad3ddf`. It remains intact as historical source only.
- Historical OCI native artifact: none found. No historical package digest recorded.
- Archive tag `archive/custom-before-refactor-20260902` preserves the separate pre-refactor tree at `038c7f18cf637a73a6cdac0f10013f75a7ce8ee9`; it is not merged into the current custom branch.
- Gatekeeper may require manual approval: app bundles use ad-hoc signing; no Apple identity or notarization is configured.

## Publishing

Only a **published GitHub Release** triggers `.github/workflows/custom-release.yml`.
Pushes, tag pushes and pull requests do not publish. The tag must be
`v<crates/zed/Cargo.toml version>-custom.<positive integer>`; the current base is
`1.18.0`; this migration's first fully validated asset release uses
`v1.18.0-custom.4`. Existing release tags are never moved. The tag commit must be
an ancestor of `origin/custom` and contain the workflow and scripts. Both platforms
check out exactly that commit; later pushes cannot change the build.

Before first asset publication, enable only this workflow (disable inherited upstream
workflows), keep workflow-level token permissions read-only, and set the default branch
to `custom`. The build job uses GitHub's built-in token with `contents: write`
to upload release assets; no Forgejo credentials, registry environment or package
setup is needed. The summary job also has `contents: write` to update notes.

Publish the release targeting the pushed custom SHA:

```sh
gh release create v1.18.0-custom.4 --repo cybito/zed \
  --target <custom-commit-sha> --title v1.18.0-custom.4 \
  --notes 'Custom Zed Dev; installation packages are attached to this GitHub Release.'
```

This file describes the implementation, not a claim that hosted builds have
already succeeded.

## Build and storage contract

- Hosted runners: macOS ARM64 `macos-26` and Linux ARM64 `ubuntu-24.04-arm`.
  Linux packages target GNU ARM64 and are suitable for Omarchy ARM64; no AMD64,
  Debian package, paid runner or self-hosted runner is part of this flow.
- Rust `1.97.1`; cargo-bundle source pinned to
  `2be2669972dff3ddd4daf89a2cb29d2d06cad7c7`. Cargo builds and tool installation
  use `--locked`. Release debug information/LTO are disabled for these runners,
  with two build jobs and a six-hour timeout. The channel remains **dev**.
- macOS includes an ad-hoc-signed **Zed Dev.app** and Applications link in a DMG.
  Linux preserves upstream's complete `zed-dev.app` resource tree and desktop
  integration. Each platform also carries its matching remote-server `.gz`;
  Linux uses GNU ARM64 instead of an unverified ARM64 musl toolchain.
- Every file in a platform package—including `release.json` and `SHA256SUMS`—is
  attached to the GitHub Release as `<tag>-<platform>-<original-filename>`.
  Existing assets are immutable: matching bytes may be reused, differing bytes
  fail without overwrite, and missing assets in a partial set may be safely
  added. Uploads omit `--clobber`; downloaded readback verifies receipt identity,
  checksums and payload bytes. Per-file size must be below 2 GiB and each release
  may contain at most 1000 assets.
- `release.json` contains schema 1, exact source identity, release tag,
  platform/architecture, actual toolchain versions and payload hashes/sizes.
  `SHA256SUMS` covers payloads and the receipt. The receipt is a sidecar, never
  recursively embedded in its own archive.
- A successful platform remains available if the other platform fails. The final
  job verifies both platforms and updates only the managed `custom-builds` notes
  block, preserving user-authored notes and linking every GitHub Release asset.
  GUI smoke evidence is uploaded separately as a seven-day GitHub Actions
  diagnostic artifact and is never part of the product release assets.

## Download, verify and install

Use the immutable asset links in the successful GitHub Release notes. Fetch one
platform's assets with a matching pattern:

```sh
mkdir -p /tmp/zed-download
cd /tmp/zed-download
gh release download v1.18.0-custom.4 --repo cybito/zed \
  --pattern 'v1.18.0-custom.4-linux-*'
prefix='v1.18.0-custom.4-linux-'
for file in "$prefix"*; do mv "$file" "${file#"$prefix"}"; done
sha256sum -c SHA256SUMS
```

Asset names include release tag and platform; for example:
`v1.18.0-custom.4-linux-zed-v1.18.0-custom.4-linux-arm64.tar.gz`.
The loop restores the package's original filenames for checksum and install
commands below.

Linux:

```sh
mkdir /tmp/zed-unpacked
tar -xzf /tmp/zed-download/zed-v1.18.0-custom.4-linux-arm64.tar.gz -C /tmp/zed-unpacked
/tmp/zed-unpacked/install.sh --prefix /absolute/install-prefix
```

The installer puts the tree at `<prefix>/lib/zed-dev/zed-dev.app`, exposes
`<prefix>/bin/zed` as a relative link to its `bin/zed`, and copies desktop/icon
resources to `<prefix>/share`. Add `<prefix>/bin` to PATH. The application uses
host graphical/system libraries: this is not a Docker runtime image.

macOS:

```sh
mkdir -p /tmp/zed-darwin-download
cd /tmp/zed-darwin-download
gh release download v1.18.0-custom.4 --repo cybito/zed \
  --pattern 'v1.18.0-custom.4-darwin-*'
prefix='v1.18.0-custom.4-darwin-'
for file in "$prefix"*; do mv "$file" "${file#"$prefix"}"; done
shasum -a 256 -c SHA256SUMS
mkdir /tmp/zed-mount
hdiutil attach /tmp/zed-darwin-download/zed-v1.18.0-custom.4-darwin-arm64.dmg \
  -readonly -nobrowse -mountpoint /tmp/zed-mount
/tmp/zed-mount/install.sh --prefix /absolute/install-prefix
hdiutil detach /tmp/zed-mount
```

The macOS installer copies the app to `<prefix>/Applications/Zed Dev.app`.
Alternatively, drag the app to Applications yourself. Python 3 is required by
the explicit-prefix installers. With no arguments the prefix is `$HOME/.local`.
Installers accept only an absolute `--prefix`, reject existing different files,
and are idempotent for identical contents. They do not overwrite configuration,
uninstall packages, restart services or change running production instances.
Ad-hoc signatures are not Apple notarization: Gatekeeper may require deliberate
manual approval. No Apple Developer identity is selected or requested.

## Smoke evidence and helper interface

The build uses independent temporary HOME/XDG state, mounts/extracts the package,
installs twice to a fixture prefix, checks native versions and ARM64 object
headers, verifies macOS deep/strict signatures and Linux dependencies/desktop
entries, and opens `smoke.txt` containing `custom-ci-ok`. Native `--help` confirms
`--user-data-dir` before it is used. macOS CoreGraphics enumerates a visible
`smoke.txt` window and captures it; Linux uses a fixed X11 session under
`dbus-run-session`/Xvfb with Mesa software rendering, inspecting window titles
and capturing a screenshot. Missing window/screenshot evidence fails the job,
not a false GUI success.

GUI diagnostic files are uploaded with the repository's pinned
`actions/upload-artifact` action for seven days, separate from product assets.
The helper CLI is exercised by the release workflow:

```text
check --tag TAG --commit SHA --platform darwin|linux --output-dir ABS
pack --tag TAG --commit SHA --platform darwin|linux --input-dir ABS --output-dir ABS
publish --tag TAG --commit SHA --platform darwin|linux --directory ABS --output-dir ABS
```

The build entry is `custom-release.sh build PLATFORM TAG SHA ABS_OUTPUT`.
`check` reads GitHub release assets and validates complete packages; partial
asset sets are reported as incomplete so a build can reconcile missing files.
The workflow also runs isolated-Git regressions for custom ancestry,
upstream-only commits, malformed tags and shell-like input, plus CLI-level
Darwin/Linux build dispatch regressions.

This migration does not alter infra-as-code consumers or deployment pins.
