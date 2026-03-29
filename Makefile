# Makefile — Monadic Hypervisor Build & Simulation
#
# Targets:
#   make build   — Cross-compile bare-metal AArch64 ELF (release)
#   make run     — Boot the hypervisor in QEMU virt at EL2
#   make debug   — Same as run, but halted with GDB stub on :1234
#   make clean   — Remove target/ artefacts
#
# Prerequisites:
#   - rustup target add aarch64-unknown-none
#   - qemu-system-aarch64 (v8.0+ recommended for Neoverse CPU models)
#
# SPDX-License-Identifier: MIT
# Copyright (c) 2026  SiliconLanguage — Monadic Hypervisor Project
# ──────────────────────────────────────────────────────────────────────

# ── Toolchain ─────────────────────────────────────────────────────────
CARGO   := $(HOME)/.cargo/bin/cargo
TARGET  := aarch64-unknown-none
PROFILE := release
ELF     := target/$(TARGET)/$(PROFILE)/monadic-hypervisor

# ── QEMU ──────────────────────────────────────────────────────────────
#
# -machine virt,virtualization=on
#   • virt              — ARM virtual platform (GICv3, PCIe, UART)
#   • virtualization=on — HCR_EL2.{VM,SWIO} accessible, EL2 active.
#     Without this flag QEMU starts at EL1 and our boot.S CurrentEL
#     check will spin-park the core.
#
# -cpu max
#   • Exposes every feature the host + TCG can emulate, including
#     LSE atomics (CASAL/LDADD) and VHE (HCR_EL2.E2H).
#     For Graviton-accurate simulation replace with:
#       -cpu neoverse-n1    (Graviton2)
#       -cpu neoverse-n2    (Cobalt 100)
#       -cpu neoverse-v2    (Graviton4) — requires QEMU 9.0+
#
# -m 2G       — 2 GiB DRAM (ORIGIN 0x4000_0000 .. 0xC000_0000)
# -nographic  — UART0 on stdio, no framebuffer window
# -kernel     — ELF entry point = _start @ 0x4000_0000
#
QEMU        := qemu-system-aarch64

# Auto-detect QEMU ROM directory.  Override with: make run QEMU_ROMDIR=/path
# Probe order: system install → Homebrew → source build in /tmp → /usr/share
QEMU_ROMDIR ?= $(firstword $(wildcard \
	/usr/local/share/qemu \
	/opt/homebrew/share/qemu \
	/tmp/qemu-*/build/qemu-bundle/usr/local/share/qemu \
	/usr/share/qemu))

# Only pass -L when QEMU_ROMDIR resolved to a real directory.
QEMU_ROMFLAG := $(if $(QEMU_ROMDIR),-L $(QEMU_ROMDIR),)

QEMU_COMMON := \
	$(QEMU_ROMFLAG) \
	-machine virt,virtualization=on \
	-cpu max \
	-m 2G \
	-nographic \
	-kernel $(ELF)

# ── Targets ───────────────────────────────────────────────────────────

.PHONY: build run debug clean

build:
	$(CARGO) build --$(PROFILE)

run: build
	$(QEMU) $(QEMU_COMMON)

debug: build
	$(QEMU) $(QEMU_COMMON) -s -S

clean:
	$(CARGO) clean
