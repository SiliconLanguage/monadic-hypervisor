#!/usr/bin/env bash
# scripts/setup-toolchain.sh — Install all build prerequisites for the
# Monadic Hypervisor (Rust bare-metal AArch64 target, QEMU, GDB).
#
# Usage:
#   ./scripts/setup-toolchain.sh          # install everything
#   ./scripts/setup-toolchain.sh rust     # Rust toolchain only
#   ./scripts/setup-toolchain.sh qemu     # QEMU only
#   ./scripts/setup-toolchain.sh gdb      # GDB only
#
# SPDX-License-Identifier: BSD-2-Clause-Patent
set -euo pipefail

QEMU_VERSION="${QEMU_VERSION:-9.2.2}"

# ── Helpers ───────────────────────────────────────────────────────────

info()  { printf '\033[1;34m=> %s\033[0m\n' "$*"; }
warn()  { printf '\033[1;33m=> %s\033[0m\n' "$*"; }
error() { printf '\033[1;31m=> %s\033[0m\n' "$*" >&2; exit 1; }

detect_pm() {
    if command -v dnf &>/dev/null; then
        PM=dnf
    elif command -v apt-get &>/dev/null; then
        PM=apt
    elif command -v brew &>/dev/null; then
        PM=brew
    else
        PM=unknown
    fi
}

# ── Rust ──────────────────────────────────────────────────────────────

install_rust() {
    info "Installing Rust toolchain"

    if command -v rustup &>/dev/null; then
        info "rustup already installed — updating"
        rustup update
    else
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        # shellcheck source=/dev/null
        source "${HOME}/.cargo/env"
    fi

    info "Adding bare-metal AArch64 target: aarch64-unknown-none"
    rustup target add aarch64-unknown-none

    info "Rust $(rustc --version) ready"
}

# ── QEMU ──────────────────────────────────────────────────────────────

install_qemu() {
    info "Installing QEMU (AArch64 system emulator)"

    # Try the system package manager first.
    detect_pm
    case "$PM" in
        dnf)
            if dnf list --installed qemu-system-aarch64 &>/dev/null 2>&1; then
                info "qemu-system-aarch64 already installed via dnf"
                return
            fi
            info "Attempting: sudo dnf install qemu-system-aarch64"
            if sudo dnf install -y qemu-system-aarch64; then return; fi
            warn "Package not available — falling back to source build"
            ;;
        apt)
            if dpkg -s qemu-system-arm &>/dev/null 2>&1; then
                info "qemu-system-arm already installed via apt"
                return
            fi
            info "Attempting: sudo apt install qemu-system-arm"
            if sudo apt-get install -y qemu-system-arm; then return; fi
            warn "Package not available — falling back to source build"
            ;;
        brew)
            if brew list qemu &>/dev/null 2>&1; then
                info "qemu already installed via brew"
                return
            fi
            info "Attempting: brew install qemu"
            brew install qemu
            return
            ;;
        *)
            warn "Unknown package manager — building from source"
            ;;
    esac

    # Source build fallback.
    build_qemu_from_source
}

build_qemu_from_source() {
    local src="/tmp/qemu-${QEMU_VERSION}"
    local tarball="${src}.tar.xz"

    info "Building QEMU ${QEMU_VERSION} from source"

    if [[ ! -d "${src}" ]]; then
        curl -sL "https://download.qemu.org/qemu-${QEMU_VERSION}.tar.xz" -o "${tarball}"
        tar xJf "${tarball}" -C /tmp
        rm -f "${tarball}"
    fi

    # tomli is a build-time dependency for QEMU's meson.
    pip3 install --user tomli 2>/dev/null || true

    cd "${src}"
    ./configure \
        --target-list=aarch64-softmmu \
        --enable-kvm \
        --prefix=/usr/local
    make -j"$(nproc)"
    sudo make install

    info "QEMU $(qemu-system-aarch64 --version | head -1) installed to /usr/local"
}

# ── GDB ───────────────────────────────────────────────────────────────

install_gdb() {
    info "Installing GDB (AArch64 support)"

    detect_pm
    case "$PM" in
        dnf)  sudo dnf install -y gdb ;;
        apt)  sudo apt-get install -y gdb-multiarch ;;
        brew) brew install gdb ;;
        *)    warn "Unknown package manager — install GDB manually" ;;
    esac
}

# ── Main ──────────────────────────────────────────────────────────────

main() {
    local components=("${@:-all}")

    for component in "${components[@]}"; do
        case "$component" in
            rust)  install_rust ;;
            qemu)  install_qemu ;;
            gdb)   install_gdb  ;;
            all)
                install_rust
                install_qemu
                install_gdb
                ;;
            *)
                error "Unknown component: ${component} (valid: rust, qemu, gdb, all)"
                ;;
        esac
    done

    info "Setup complete"
}

main "$@"
