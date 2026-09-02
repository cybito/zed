#!/usr/bin/env bash

usage() {
    cat <<'EOF'
Usage: script/update-local-release.sh [--install] [--no-build] [--tag TAG]

Merge the latest upstream release into custom and rebuild Zed Dev.

Options:
  --install    Install the built app in /Applications
  --no-build   Merge without building
  --tag TAG    Merge a specific release tag
  -h, --help   Show this help

To build the current checkout without fetching or merging:
  ./script/bundle-mac
  ./script/bundle-mac -i
  ./script/bundle-mac -d -i
EOF
}

set -euo pipefail

BRANCH=custom
REMOTE=origin
INSTALL=0
BUILD=1
PINNED=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --install)
            INSTALL=1
            ;;
        --no-build)
            BUILD=0
            ;;
        --tag)
            if [[ $# -lt 2 || "$2" == --* ]]; then
                usage >&2
                exit 2
            fi
            PINNED="$2"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
    shift
done

cd "$(git rev-parse --show-toplevel)"

if [[ -n "$(git status --porcelain)" ]]; then
    echo "ERROR: working tree dirty — 先 stash/commit 再升级:" >&2
    git status --short >&2
    exit 1
fi

CURRENT="$(git branch --show-current)"
if [[ "$CURRENT" != "$BRANCH" ]]; then
    git checkout "$BRANCH"
fi

if [[ -n "$PINNED" ]]; then
    TAG="$PINNED"
else
    # Git tag 列表也包含预发布构建的无后缀 tag；以 GitHub 的 latest release
    # 元数据作为正式发行的唯一来源。
    TAG="$(curl --fail --silent --show-error --location \
        https://api.github.com/repos/zed-industries/zed/releases/latest \
        | sed -nE 's/^[[:space:]]*"tag_name":[[:space:]]*"([^"]+)".*/\1/p')"
fi
[[ -n "$TAG" ]] || { echo "ERROR: 未找到发行 tag" >&2; exit 1; }
echo "==> target release: $TAG"

if git merge-base --is-ancestor "$TAG" HEAD 2>/dev/null; then
    echo "==> already up to date ($TAG)"
else
    git fetch --no-tags "$REMOTE" "refs/tags/$TAG:refs/tags/$TAG" --force
    echo "==> merging $TAG into $BRANCH"
    # merge 需要提交身份；浅配置环境兜底
    if [[ -z "$(git config user.email || true)" ]]; then
        GIT=(git -c user.name="${USER:-local}" -c user.email="${USER:-local}@localhost")
    else
        GIT=(git)
    fi
    if ! "${GIT[@]}" merge --no-edit "$TAG"; then
        echo "" >&2
        echo "CONFLICT: 上游与本地修改冲突。" >&2
        echo "  解决后:   git add -A && git merge --continue" >&2
        echo "  放弃合并: git merge --abort" >&2
        exit 1
    fi
fi

if [[ $BUILD -eq 1 ]] && [[ "$(uname)" == "Darwin" ]]; then
    if pgrep -xq zed || pgrep -xq Zed; then
        echo "WARNING: Zed 正在运行，安装后需重启才能生效。" >&2
    fi
    echo "==> building (首次运行会安装 zed fork 的 cargo-bundle，编译耗时较长)"
    if [[ $INSTALL -eq 1 ]]; then
        ./script/bundle-mac -i
    else
        ./script/bundle-mac
    fi
fi

echo "==> done: $BRANCH merged $TAG"
