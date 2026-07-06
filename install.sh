#!/usr/bin/env bash

set -Eeuo pipefail

readonly REPOSITORY_URL="https://github.com/NeoloveEngine/NeoLOVE.git"
readonly OS_NAME="$(uname -s)"

log() {
    printf '\n==> %s\n' "$*"
}

fail() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

as_root() {
    if [[ ${EUID:-$(id -u)} -eq 0 ]]; then
        "$@"
    elif command -v sudo >/dev/null 2>&1; then
        sudo "$@"
    else
        fail "Installing system packages requires root access or sudo."
    fi
}

install_macos_dependencies() {
    if xcode-select -p >/dev/null 2>&1; then
        return
    fi

    log "Installing the Xcode Command Line Tools (Git and native build tools)"
    xcode-select --install 2>/dev/null || true
    printf 'Complete the Apple installer dialog. Setup will continue automatically.\n'

    local attempts=0
    until xcode-select -p >/dev/null 2>&1; do
        ((attempts += 1))
        if ((attempts >= 360)); then
            fail "Xcode Command Line Tools were not installed within 30 minutes. Re-run this script after installation finishes."
        fi
        sleep 5
    done
}

install_linux_dependencies() {
    if command -v git >/dev/null 2>&1 &&
        command -v curl >/dev/null 2>&1 &&
        command -v cc >/dev/null 2>&1 &&
        command -v c++ >/dev/null 2>&1 &&
        command -v make >/dev/null 2>&1 &&
        command -v pkg-config >/dev/null 2>&1 &&
        pkg-config --exists alsa &&
        command -v vulkaninfo >/dev/null 2>&1; then
        log "Linux build dependencies already installed"
        return
    fi

    log "Installing Git and native build dependencies"
    if command -v apt-get >/dev/null 2>&1; then
        as_root apt-get update
        as_root apt-get install -y git curl build-essential pkg-config libasound2-dev vulkan-tools
    elif command -v dnf >/dev/null 2>&1; then
        as_root dnf install -y git curl gcc gcc-c++ make pkgconf-pkg-config alsa-lib-devel vulkan-tools
    elif command -v yum >/dev/null 2>&1; then
        as_root yum install -y git curl gcc gcc-c++ make pkgconfig alsa-lib-devel vulkan-tools
    elif command -v pacman >/dev/null 2>&1; then
        as_root pacman -Syu --needed --noconfirm git curl base-devel pkgconf alsa-lib vulkan-tools
    elif command -v zypper >/dev/null 2>&1; then
        as_root zypper --non-interactive install git curl gcc gcc-c++ make pkg-config alsa-devel vulkan-tools
    elif command -v apk >/dev/null 2>&1; then
        as_root apk add git curl build-base pkgconf alsa-lib-dev vulkan-tools
    else
        fail "Unsupported Linux package manager. Install Git, curl, a C/C++ toolchain, pkg-config, ALSA development headers, and vulkaninfo, then re-run this script."
    fi
}

install_rust() {
    if [[ -f "$HOME/.cargo/env" ]]; then
        # shellcheck disable=SC1090
        source "$HOME/.cargo/env"
    fi

    if command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1; then
        log "Rust toolchain already installed: $(rustc --version)"
        return
    fi

    if command -v rustup >/dev/null 2>&1; then
        log "Completing the existing Rust toolchain installation"
        rustup toolchain install stable --profile minimal
        rustup default stable
    else
        command -v curl >/dev/null 2>&1 || fail "curl is required to install Rust."
        log "Installing the stable Rust toolchain"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
            sh -s -- -y --profile minimal --default-toolchain stable
    fi

    # rustup updates future shells; this makes Cargo available to this process.
    # shellcheck disable=SC1090
    source "$HOME/.cargo/env"
    command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1 ||
        fail "The Rust toolchain installation did not complete successfully. Re-run this script to resume setup."
}

repository_remote() {
    git -C "$1" remote get-url origin 2>/dev/null || true
}

repository_remote_matches() {
    local remote
    remote=$(repository_remote "$1")
    [[ "$remote" == "$REPOSITORY_URL" || "$remote" == "${REPOSITORY_URL%.git}" ]]
}

repository_is_complete() {
    repository_remote_matches "$1" &&
        git -C "$1" rev-parse --verify HEAD >/dev/null 2>&1 &&
        [[ -f "$1/Cargo.toml" ]]
}

cleanup_stale_staging_repository() {
    local staging=$1
    local marker="$staging/.neolove-installer"
    [[ -e "$staging" ]] || return 0

    if [[ ! -f "$marker" || "$(sed -n '1p' "$marker")" != "$REPOSITORY_URL" ]]; then
        fail "$staging already exists and was not created by the NeoLOVE installer. Move it elsewhere and re-run this script."
    fi

    local owner_pid
    owner_pid=$(sed -n '2p' "$marker")
    if [[ "$owner_pid" =~ ^[0-9]+$ && "$owner_pid" != "$$" ]] && kill -0 "$owner_pid" 2>/dev/null; then
        fail "Another NeoLOVE installer is currently using $staging (process $owner_pid)."
    fi

    log "Cleaning up an interrupted NeoLOVE clone"
    rm -rf -- "$staging"
}

clone_repository_transactionally() {
    local destination=$1
    local staging="${destination}.installing"

    cleanup_stale_staging_repository "$staging"
    mkdir -p "$staging"
    printf '%s\n%s\n' "$REPOSITORY_URL" "$$" >"$staging/.neolove-installer"

    log "Cloning NeoLOVE into $destination"
    git clone "$REPOSITORY_URL" "$staging/checkout"
    [[ ! -e "$destination" ]] || fail "$destination appeared while NeoLOVE was being cloned. The completed clone remains in $staging/checkout."
    mv "$staging/checkout" "$destination"
    rm "$staging/.neolove-installer"
    rmdir "$staging"
}

clone_or_update_repository() {
    local destination=$1
    local parent
    parent=$(dirname "$destination")
    mkdir -p "$parent"
    cleanup_stale_staging_repository "${destination}.installing"

    if repository_is_complete "$destination"; then
        log "Existing NeoLOVE installation found"
        if [[ -z "$(git -C "$destination" status --porcelain)" ]]; then
            log "Updating the existing NeoLOVE clone"
            git -C "$destination" pull --ff-only
        else
            log "Existing clone has local changes; leaving them untouched"
        fi
    elif [[ -e "$destination/.git" ]]; then
        repository_remote_matches "$destination" ||
            fail "$destination exists but is not a clone of $REPOSITORY_URL"

        local backup="${destination}.incomplete-$(date -u +%Y%m%dT%H%M%SZ)-$$"
        log "Preserving an incomplete NeoLOVE checkout at $backup"
        mv "$destination" "$backup"
        clone_repository_transactionally "$destination"
    elif [[ -e "$destination" ]]; then
        if [[ -d "$destination" && -z "$(find "$destination" -mindepth 1 -print -quit)" ]]; then
            rmdir "$destination"
            clone_repository_transactionally "$destination"
        else
            fail "$destination already exists and is not a Git repository. Move it elsewhere and re-run this script."
        fi
    else
        clone_repository_transactionally "$destination"
    fi

    chmod 700 "$destination" 2>/dev/null || true
}

vulkan_is_available() {
    command -v vulkaninfo >/dev/null 2>&1 && vulkaninfo --summary >/dev/null 2>&1
}

main() {
    # New directories and files are private to the current user by default.
    umask 077

    local install_directory
    case "$OS_NAME" in
        Linux)
            install_directory="${XDG_DATA_HOME:-$HOME/.local/share}/NeoLOVE"
            install_linux_dependencies
            ;;
        Darwin)
            install_directory="$HOME/Library/Application Support/NeoLOVE"
            install_macos_dependencies
            ;;
        *)
            fail "Unsupported operating system: $OS_NAME. Use install.ps1 on Windows."
            ;;
    esac

    command -v git >/dev/null 2>&1 || fail "Git installation did not complete successfully."
    clone_or_update_repository "$install_directory"
    install_rust

    local -a cargo_args=(run --release --locked)
    case "${NEOLOVE_VULKAN:-auto}" in
        1|on|true)
            cargo_args+=(--features vulkan)
            log "Vulkan enabled by NEOLOVE_VULKAN"
            ;;
        0|off|false)
            log "Vulkan disabled by NEOLOVE_VULKAN"
            ;;
        auto)
            if vulkan_is_available; then
                cargo_args+=(--features vulkan)
                log "Compatible Vulkan runtime detected; enabling the Vulkan renderer"
            else
                log "No working Vulkan runtime detected; using the software renderer"
            fi
            ;;
        *)
            fail "NEOLOVE_VULKAN must be auto, 1/on/true, or 0/off/false."
            ;;
    esac

    cargo_args+=(-- editor)
    log "Compiling and launching NeoLOVE in release mode"
    cd "$install_directory"
    cargo "${cargo_args[@]}"
}

main "$@"
