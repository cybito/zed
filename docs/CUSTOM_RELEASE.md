# Custom release provenance

- GitHub fork: `https://github.com/cybito/zed` (upstream: `https://github.com/zed-industries/zed.git`). `custom` is the custom build and release source; the product remains Zed Dev.
- Historical Forgejo source: `https://git.cybit.top/cybit/zed`; custom revision at migration: `b07b7e820656e3d6abe80cdf92527de943ad3ddf`. It remains intact.
- Historical OCI native artifact: none found. No historical package digest recorded.
- Custom installation packages use OCI package `ias-zed`; immutable download form: `oras pull git.cybit.top/cybit/ias-zed@sha256:<digest>`.
- Archive tag `archive/custom-before-refactor-20260902` preserves the separate pre-refactor tree at `038c7f18cf637a73a6cdac0f10013f75a7ce8ee9`; it is not merged into the current custom branch.
- Gatekeeper may require manual approval: app bundles use ad-hoc signing; no Apple identity or notarization is configured.

## Publishing

Only a **published GitHub Release** triggers `.github/workflows/custom-release.yml`.
Pushes, tag pushes and pull requests do not publish. The tag must be
`v<crates/zed/Cargo.toml version>-custom.<positive integer>`; the current base is
`1.18.0`, with proposed first release `v1.18.0-custom.1`. Choose the next unused
custom integer if that tag already exists. The dereferenced tag commit must be an
ancestor of `origin/custom` and contain the workflow and scripts. Both platforms
check out exactly that commit; later pushes cannot change the build.

Before first publication, enable only this workflow (disable inherited upstream
workflows), keep the default GitHub token read-only, and set the default branch
to `custom`. Provision the `forgejo-registry` GitHub environment with a tag-only
deployment policy `v*-custom.*` and secret `FORGEJO_REGISTRY_TOKEN`. A human must
create a dedicated `cybit` PAT with package-write scope; do not export existing
OAuth credentials or Docker helpers. Package permissions cover the owner
namespace and may authorize other `cybit` packages, not just this project.
Associate the new Forgejo `ias-zed` package with `cybit/zed` in package settings;
the OCI source annotation intentionally remains the real GitHub source.

Publish the release targeting the pushed custom SHA, without attachments:

```sh
gh release create v1.18.0-custom.1 --repo cybito/zed \
  --target <custom-commit-sha> --title v1.18.0-custom.1 \
  --notes 'Custom Zed Dev; installation packages are stored in Forgejo.'
```

This file describes the implementation, not a claim that hosted builds or
credential provisioning have already succeeded.

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
- Installation artifacts use `application/vnd.cybito.install-package.v1`,
  not the historical native-receipt artifact type. The OCI tags are
  `<release-tag>-darwin-arm64` and `<release-tag>-linux-arm64` in
  `git.cybit.top/cybit/ias-zed`. No historical version is deleted.
- `release.json` contains schema 1, exact source identity, release tag,
  platform/architecture, actual toolchain versions and payload hashes/sizes.
  `SHA256SUMS` covers payloads and the receipt. The receipt is a sidecar, never
  recursively embedded in its own archive.
- Existing tags are pulled and fully verified before reuse; a different source
  identity fails. Authentication/network failures are never interpreted as
  package absence. A successful platform survives failure of the other.
  Every new publication is built as a local OCI layout, copied to Forgejo and
  pulled back by digest with manifest, layer and checksum validation.
- No GitHub release assets, build artifacts or caches are uploaded. A final
  anonymous verification job replaces only the `custom-builds` notes block,
  preserving human-written notes and adding both immutable download references.

## Download, verify and install

Use the immutable references in the successful GitHub Release notes:

```sh
mkdir -p /tmp/zed-download
oras pull git.cybit.top/cybit/ias-zed@sha256:<digest> \
  --output /tmp/zed-download
cd /tmp/zed-download
shasum -a 256 -c SHA256SUMS
```

For stronger OCI verification, with ORAS 1.3.3 and Python 3:

```sh
python3 .github/scripts/package-release.py verify \
  --reference git.cybit.top/cybit/ias-zed@sha256:<digest> \
  --output-dir /absolute/empty/verification-directory
```

Linux:

```sh
mkdir /tmp/zed-unpacked
tar -xzf zed-v1.18.0-custom.1-linux-arm64.tar.gz -C /tmp/zed-unpacked
/tmp/zed-unpacked/install.sh --prefix /absolute/install-prefix
```

The installer puts the tree at `<prefix>/lib/zed-dev/zed-dev.app`, exposes
`<prefix>/bin/zed` as a relative link to its `bin/zed`, and copies desktop/icon
resources to `<prefix>/share`. Add `<prefix>/bin` to PATH. The application uses
host graphical/system libraries: this is not a Docker runtime image.

macOS:

```sh
mkdir /tmp/zed-mount
hdiutil attach zed-v1.18.0-custom.1-darwin-arm64.dmg \
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

Screenshot, window title and logs are separate diagnostics OCI artifacts:
`<release-tag>-<platform>-arm64-smoke`, media type
`application/vnd.cybito.gui-smoke.v1`. Their immutable reference appears in the
job summary; pull it with ORAS to review. Diagnostics never enter install payloads.

The helper's public CLI is fixed:

```text
check --tag TAG --commit SHA --platform darwin|linux --output-dir ABS
pack --tag TAG --commit SHA --platform darwin|linux --input-dir ABS --output-dir ABS
publish --directory ABS --registry-config ABS
verify --reference OCI_DIGEST_REF --output-dir ABS
```

All verification output directories must be empty. `check` emits JSON
`exists` and an immutable reference when found, or `exists:false` only on explicit
registry name/manifest-unknown errors. `pack` emits its directory; `publish`
emits reference/digest; `verify` emits the validated receipt.
The build entry is `custom-release.sh build PLATFORM TAG SHA ABS_OUTPUT`.
No registry write credential is supplied until build/install/GUI smoke succeeds.
The upload step uses a private temporary auth configuration and deletes it in
an always-run cleanup step. The workflow includes real isolated-Git regressions
for custom ancestry, upstream-only commits, malformed tags and shell-like input.

This migration does not alter infra-as-code consumers or deployment pins.
