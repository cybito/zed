#!/usr/bin/env bash
#
# update-local-release.sh — 把 local/custom 分支合并到最新上游 stable release 并重新编译。
#
# 本分支在 upstream main/release 之上维护少量本地提交（见 git log --first-parent）。
# 升级 = 拉取最新发行 tag 并 merge 进本分支；冲突时脚本停下并给出指引。
#
# 用法：
#   script/update-local-release.sh                # 合并最新 release + 编译 bundle
#   script/update-local-release.sh --install      # 编译并安装到 /Applications
#   script/update-local-release.sh --tag v0.210.2 # 锁定某个版本
#   script/update-local-release.sh --no-build     # 只做合并，不编译
#
set -euo pipefail

BRANCH=local/custom
REMOTE=origin
INSTALL=0
BUILD=1
PINNED=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --install) INSTALL=1 ;;
        --no-build) BUILD=0 ;;
        --tag) PINNED="$2"; shift ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
    shift
done

cd "$(git rev-parse --show-toplevel)"

# merge 需要提交身份；浅配置环境兜底
if [[ -z "$(git config user.email || true)" ]]; then
    GIT=(git -c user.name="${USER:-local}" -c user.email="${USER:-local}@localhost")
else
    GIT=(git)
fi

CURRENT="$(git branch --show-current)"
if [[ "$CURRENT" != "$BRANCH" ]]; then
    git checkout "$BRANCH"
fi

if [[ -n "$(git status --porcelain)" ]]; then
    echo "ERROR: working tree dirty — 先 stash/commit 再升级:" >&2
    git status --short >&2
    exit 1
fi

if [[ -n "$PINNED" ]]; then
    TAG="$PINNED"
else
    # 只认形如 vX.Y.Z 的 stable 发行 tag（排除 -pre/-nightly 等带连字符的）
    TAG="$("${GIT[@]}" ls-remote --tags --refs "$REMOTE" 'v*' \
        | cut -d'/' -f3 \
        | grep -Ev -- '-' \
        | sort -V | tail -n1)"
fi
[[ -n "$TAG" ]] || { echo "ERROR: 未找到发行 tag" >&2; exit 1; }
echo "==> target release: $TAG"

if git merge-base --is-ancestor "$TAG" HEAD 2>/dev/null; then
    echo "==> already up to date ($TAG)"
else
    "${GIT[@]}" fetch --no-tags "$REMOTE" "refs/tags/$TAG:refs/tags/$TAG" --force
    echo "==> merging $TAG into $BRANCH"
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
