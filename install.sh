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
    if command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1; then
        log "Rust toolchain already installed: $(rustc --version)"
        return
    fi

    command -v curl >/dev/null 2>&1 || fail "curl is required to install Rust."
    log "Installing the stable Rust toolchain"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
        sh -s -- -y --profile minimal --default-toolchain stable

    # rustup updates future shells; this makes Cargo available to this process.
    # shellcheck disable=SC1090
    source "$HOME/.cargo/env"
}

clone_or_update_repository() {
    local destination=$1
    mkdir -p "$(dirname "$destination")"

    if [[ -d "$destination/.git" ]]; then
        local remote
        remote=$(git -C "$destination" remote get-url origin 2>/dev/null || true)
        [[ "$remote" == "$REPOSITORY_URL" || "$remote" == "${REPOSITORY_URL%.git}" ]] ||
            fail "$destination exists but is not a clone of $REPOSITORY_URL"

        if [[ -z "$(git -C "$destination" status --porcelain)" ]]; then
            log "Updating the existing NeoLOVE clone"
            git -C "$destination" pull --ff-only
        else
            log "Existing clone has local changes; leaving them untouched"
        fi
    elif [[ -e "$destination" ]]; then
        fail "$destination already exists and is not a Git repository."
    else
        log "Cloning NeoLOVE into $destination"
        git clone "$REPOSITORY_URL" "$destination"
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
