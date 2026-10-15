#!/bin/sh
# Shared by make, the legacy Python entry point and Installer packages.
# --root is for offline filesystem tests only; it never restarts system audio.
set -eu
PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH
umask 022

die() { echo "Fast Track Ultra: $*" >&2; exit 1; }
action=${1:-}
[ "$#" -gt 0 ] && shift
source_bundle=
case "$action" in
    check|install) source_bundle=${1:?A bundle path is required}; shift ;;
    remove) ;;
    *) die 'Usage: deploy_hal.sh check|install BUNDLE|remove [--restart] [--root TEST_DIRECTORY]' ;;
esac
root=/
restart=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --restart) restart=true; shift ;;
        --root) root=${2:?Missing test directory}; shift 2 ;;
        *) die "Unknown argument: $1" ;;
    esac
done

# Check every existing path component before creating or replacing anything.
no_symlink_path() {
    case "$1" in /*) ;; *) die 'Deployment paths must be absolute' ;; esac
    checked=$1
    while [ "$checked" != / ]; do
        [ ! -L "$checked" ] || die "Refusing symlink: $checked"
        checked=$(dirname "$checked")
    done
}

validate() {
    no_symlink_path "$1"
    [ -d "$1" ] || die "Missing bundle: $1"
    [ -z "$(find "$1" ! -type f ! -type d -print)" ] || die 'Bundle contains symlinks or special files'
    info=$1/Contents/Info.plist
    [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$info")" = net.batarelo.FastTrackUltra ] || die 'Unexpected bundle identifier'
    [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$info")" = FastTrackUltra ] || die 'Unexpected bundle executable'
    [ -f "$1/Contents/MacOS/FastTrackUltra" ] || die 'Missing executable'
    /usr/bin/codesign --verify --strict "$1"
    # lipo can be a developer-tools shim; recipients need only stock macOS.
    # This project ships a single-architecture arm64 bundle, not a universal one.
    kind=$(/usr/bin/file -b "$1/Contents/MacOS/FastTrackUltra")
    case "$kind" in
        'Mach-O 64-bit bundle arm64'|'Mach-O 64-bit bundle arm64 '*) ;;
        *) die "Expected an arm64 Mach-O bundle; found: $kind" ;;
    esac
}

if [ -n "$source_bundle" ]; then validate "$source_bundle"; fi
if [ "$action" = check ]; then
    [ "$restart" = false ] || die '--restart requires install or remove'
    echo "Validated: $source_bundle"
    exit 0
fi
no_symlink_path "$root"
[ -d "$root" ] || die 'The test root must already exist'
root=$(cd "$root" && pwd -P)
if [ "$root" = / ]; then
    [ "$(id -u)" -eq 0 ] || die 'Administrator rights required; use make install or make uninstall.'
    [ "$(uname -m)" = arm64 ] || die 'This driver requires Apple Silicon (run natively, outside Rosetta).'
    major=$(/usr/bin/sw_vers -productVersion | cut -d . -f 1)
    [ "$major" -ge 14 ] || die 'macOS 14 or later is required.'
else
    [ "$restart" = false ] || die 'A test-root deployment cannot restart system audio'
fi
parent=${root%/}/Library/Audio/Plug-Ins
hal=$parent/HAL
destination=$hal/FastTrackUltra.driver
backups=$parent/FastTrackUltra-Backups
lock=$parent/.FastTrackUltra-install.lock
for path in "$hal" "$destination" "$backups" "$lock"; do no_symlink_path "$path"; done
if [ -n "$source_bundle" ]; then
    source_bundle=$(cd "$source_bundle" && pwd -P)
    case "$source_bundle/" in "$destination/"*) die 'Source must be outside the installed bundle' ;; esac
fi
mkdir -p "$hal"
mkdir "$lock" || die 'Another Fast Track Ultra installation is active (or its lock needs inspection).'
stage=
previous=
new_installed=false
committed=false
cleanup() {
    result=$?
    trap - EXIT HUP INT TERM
    if [ "$committed" = false ]; then
        if [ "$new_installed" = true ] && [ -d "$destination" ]; then
            /bin/mv "$destination" "$stage/failed.driver" || result=1
        fi
        if [ -n "$previous" ] && [ -d "$previous" ] && [ ! -e "$destination" ]; then
            /bin/mv "$previous" "$destination" || { echo "Restore the preserved bundle from $previous" >&2; result=1; }
        fi
    fi
    if [ -n "$stage" ]; then /bin/rm -rf "$stage"; fi
    rmdir "$lock" || result=1
    exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM

if [ "$action" = install ]; then
    # CoreAudio cannot discover the new bundle until the final rename.
    stage=$(mktemp -d "$parent/.ftu-stage.XXXXXX")
    staged=$stage/FastTrackUltra.driver
    /usr/bin/ditto "$source_bundle" "$staged"
    if [ "$root" = / ]; then /usr/sbin/chown -R root:wheel "$staged"; fi
    find "$staged" -type d -exec chmod 755 {} +
    find "$staged" -type f -exec chmod 644 {} +
    chmod 755 "$staged/Contents/MacOS/FastTrackUltra"
    validate "$staged"
fi
if [ -e "$destination" ]; then
    validate "$destination"
    mkdir -p "$backups"
    backup_dir=$(mktemp -d "$backups/$(date -u +%Y%m%dT%H%M%SZ).XXXXXX")
    previous=$backup_dir/FastTrackUltra.driver
    /bin/mv "$destination" "$previous"
    echo "Previous bundle preserved: $previous"
fi
if [ "$action" = install ]; then
    /bin/mv "$staged" "$destination"
    new_installed=true
    validate "$destination"
    echo "Installed: $destination"
else
    echo 'Fast Track Ultra removed from the HAL directory (or was already absent).'
fi
committed=true
if [ "$restart" = true ]; then
    /usr/bin/killall coreaudiod || die 'Files updated, but audio could not restart. Restart your Mac to load the change.'
    echo 'CoreAudio restart requested. Select Fast Track Ultra in System Settings > Sound after installation.'
else
    echo 'No audio restart requested.'
fi
