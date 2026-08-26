#!/usr/bin/env bash
# Push newest Aurelia release to AUR.
#
# Usage: scripts/aur-push.sh [-n|--dry-run] [-p PKG] [VERSION]
#   VERSION  defaults to latest GitHub release
#   -n       update files, show diff, no push
#   -p PKG   only this package (repeatable)
#            default: aurelia aurelia-bin
#
# Env: AUR_HOST    (default ssh://aur@aur.archlinux.org)
#      AUR_WORKDIR (default ~/.cache/aurelia-aur)
#      GH_TOKEN    optional, raises GitHub API limits
set -euo pipefail

REPO=Drackrath/Aurelia
AUR_HOST="${AUR_HOST:-ssh://aur@aur.archlinux.org}"
WORKDIR="${AUR_WORKDIR:-${XDG_CACHE_HOME:-$HOME/.cache}/aurelia-aur}"
DRY_RUN=0
VERSION=""
PACKAGES=()

die() { echo "error: $*" >&2; exit 1; }

usage() { sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
    case "$1" in
        -n|--dry-run) DRY_RUN=1 ;;
        -p|--package) shift; PACKAGES+=("${1:?-p needs a package}") ;;
        -h|--help) usage; exit 0 ;;
        -*) die "unknown option: $1" ;;
        *) VERSION="$1" ;;
    esac
    shift
done
[ ${#PACKAGES[@]} -gt 0 ] || PACKAGES=(aurelia aurelia-bin)

for tool in git curl makepkg updpkgsums; do
    command -v "$tool" >/dev/null || die "missing tool: $tool"
done

# Resolve latest release tag
if [ -z "$VERSION" ]; then
    auth=()
    [ -n "${GH_TOKEN:-}" ] && auth=(-H "Authorization: Bearer $GH_TOKEN")
    VERSION=$(curl -fsSL "${auth[@]}" "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/^ *"tag_name": *"\([^"]*\)".*/\1/p')
    [ -n "$VERSION" ] || die "could not resolve latest release"
fi
VERSION=${VERSION#v}
[[ $VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "bad version: $VERSION"
echo "Target version: $VERSION"

# Keep downloaded sources out of checkouts
SRCDEST=$(mktemp -d)
export SRCDEST
trap 'rm -rf "$SRCDEST"' EXIT

update_package() {
    local pkg=$1 dir="$WORKDIR/$1" current
    echo
    echo "==> $pkg"

    # Clone or refresh AUR checkout
    if [ -d "$dir/.git" ]; then
        git -C "$dir" fetch -q origin master
        git -C "$dir" reset -q --hard origin/master
    else
        git clone -q "$AUR_HOST/$pkg.git" "$dir"
    fi
    cd "$dir"

    current=$(sed -n 's/^pkgver=//p' PKGBUILD)
    if [ "$current" = "$VERSION" ]; then
        echo "already at $VERSION, nothing to do"
        return 0
    fi
    echo "at $current, updating"

    sed -i -e "s/^pkgver=.*/pkgver=$VERSION/" -e "s/^pkgrel=.*/pkgrel=1/" PKGBUILD
    updpkgsums || { echo "checksum update failed (release assets missing?)" >&2; return 1; }
    makepkg --printsrcinfo > .SRCINFO

    git add PKGBUILD .SRCINFO
    git --no-pager diff --cached --stat

    if [ "$DRY_RUN" = 1 ]; then
        echo
        git --no-pager diff --cached
        echo "dry run: not committing or pushing"
        return 0
    fi

    git commit -q -m "Update to $VERSION"
    git push -q origin HEAD:master
    echo "pushed $pkg $VERSION-1"
}

failed=()
for pkg in "${PACKAGES[@]}"; do
    update_package "$pkg" || failed+=("$pkg")
done

echo
if [ ${#failed[@]} -gt 0 ]; then
    die "failed: ${failed[*]}"
fi
echo "Done: ${PACKAGES[*]}"
