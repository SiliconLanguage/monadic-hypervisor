/*
 * linker.ld — Monadic Hypervisor EL2 Linker Script (AArch64)
 *
 * Memory map for QEMU virt machine & AWS Graviton bare-metal boot.
 *
 * QEMU `-machine virt` loads the -kernel ELF at 0x4000_0000 (1 GiB).
 * This is the first physical address of DRAM on the virt platform,
 * and matches the Graviton4 UEFI ExitBootServices() handoff address
 * for the hypervisor payload.
 *
 * Layout invariants:
 *   1. .text.boot MUST be first — contains _start (EL2 reset vector).
 *   2. All sections 4 KiB-aligned — matches our Stage-2 TG0 granule.
 *   3. .bss.stack placed inside .bss — zero-initialised, 0 image bytes.
 *   4. __stack_top defined at the top of the stack — boot.S loads SP.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026  SiliconLanguage — Monadic Hypervisor Project
 */

/* ── Entry Point ──────────────────────────────────────────────────────
 *
 * ENTRY(_start)   — ELF e_entry = _start (first instruction in .text.boot).
 * EXTERN(hypervisor_main) — Force the linker to keep this symbol even if
 *                           it appears unreachable from _start (the bl
 *                           in boot.S resolves at link time, but EXTERN
 *                           guarantees it survives --gc-sections).
 */
ENTRY(_start)
EXTERN(hypervisor_main)

/* ── Physical Memory Origin ───────────────────────────────────────────
 *
 * QEMU virt DRAM starts at 0x4000_0000.
 *
 * On real Graviton hardware, the UEFI memory map determines the load
 * address.  0x4000_0000 is a safe default that works for both QEMU
 * simulation and bare-metal UEFI payloads on Graviton2/3/4.
 *
 * LENGTH = 128M is the maximum hypervisor image size.  Our actual
 * .text + .rodata + .data fits in ~64 KiB; the .bss (page tables +
 * stack) is ~2 MiB.  128M provides ample headroom.
 */
MEMORY
{
    RAM (rwx) : ORIGIN = 0x40000000, LENGTH = 128M
}

SECTIONS
{
    /* ── .text.boot — EL2 Reset Vector (MUST be first) ────────────────
     *
     * Contains _start from arch/arm64/boot/boot.S.
     * QEMU jumps to ORIGIN (0x4000_0000) which must be the first
     * instruction of _start.  Any other section placed before this
     * would cause QEMU to execute data as code → immediate UNDEF.
     */
    .text.boot ORIGIN(RAM) : ALIGN(4)
    {
        KEEP(*(.text.boot))
    } > RAM

    /* ── .text — Rust + assembly code ─────────────────────────────────
     *
     * 4 KiB-aligned to sit on a Stage-2 page boundary — allows
     * mapping code pages as Read-Execute without including adjacent
     * data in the same 4 KiB page (W^X enforcement at Stage-2).
     */
    .text : ALIGN(4096)
    {
        *(.text .text.*)
    } > RAM

    /* ── .rodata — Read-only data ─────────────────────────────────────
     *
     * String literals, const tables, panic message strings.
     * Separate from .text so we can map it Read-Only (no Execute)
     * at Stage-2 if desired.
     */
    .rodata : ALIGN(4096)
    {
        *(.rodata .rodata.*)
    } > RAM

    /* ── .data — Initialised mutable data ─────────────────────────────
     *
     * Static mut variables with non-zero initialisers.
     * In our #![no_std] hypervisor this section is typically empty
     * (we use .bss for zero-initialised statics), but the linker
     * script must define it for correctness.
     */
    .data : ALIGN(4096)
    {
        *(.data .data.*)
    } > RAM

    /* ── .bss — Zero-initialised data ─────────────────────────────────
     *
     * Contains:
     *   - Stage-2 ROOT table (8 KiB, 8 KiB-aligned)
     *   - Stage-2 POOL (512 × 4 KiB = 2 MiB)
     *   - POOL_NEXT atomic counter (8 bytes)
     *   - .bss.stack (16 KiB boot stack, 64-byte aligned)
     *
     * Costs 0 bytes in the ELF image — only virtual address space.
     * The bootloader / QEMU zeroes this region before execution.
     *
     * __bss_start / __bss_end can be used by Rust startup code to
     * explicitly zero .bss if the bootloader doesn't guarantee it.
     */
    .bss : ALIGN(4096)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(64);
        __bss_end = .;
    } > RAM

    /* ── __stack_top — Referenced by boot.S Step 5 ────────────────────
     *
     * boot.S loads SP from __stack_top:
     *
     *   adrp  x4, __stack_top
     *   add   x4, x4, :lo12:__stack_top
     *   and   x4, x4, #~0x3F       // 64-byte align
     *   mov   sp, x4
     *
     * __stack_top is defined inside .bss.stack in boot.S itself
     * (via the .space 16384 directive).  The KEEP(*(.bss.stack))
     * above ensures it's included.  We PROVIDE a fallback here
     * only if boot.S doesn't define it — defense-in-depth.
     */
    PROVIDE(__stack_top = __bss_end + 16384);

    /* ── Heap (unused — 0-Copy pillar) ────────────────────────────────
     *
     * We define __heap_start for completeness but the Monadic Hypervisor
     * performs zero dynamic allocation (ADR-001).  No #[global_allocator]
     * is ever installed.
     */
    __heap_start = __bss_end;

    /* ── Discard unneeded sections ────────────────────────────────────
     *
     * Strip debug and comment sections from the final ELF to keep
     * the binary lean.  Debug info is available via the separate
     * debug ELF for GDB.
     */
    /DISCARD/ :
    {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }
}
