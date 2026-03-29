/*
 * src/main.rs — Monadic Hypervisor #![no_std] entry point
 *
 * This is the first Rust function executed after the EL2 assembly
 * bootstrap (arch/arm64/boot/boot.S) hands off control via:
 *
 *     bl  hypervisor_main
 *
 * At this point the hardware state is fully deterministic:
 *
 *   CurrentEL  = EL2              (verified by boot.S Step 1)
 *   HCR_EL2    = 0x8000_0001     (RW=1 AArch64 guest, VM=1 Stage-2 active)
 *   VTCR_EL2   = 0x0002_3558     (4KB TG0, 40-bit IPA/PA, Inner-Shareable)
 *   VTTBR_EL2  = 0x0             (zeroed — fail-closed Translation Fault)
 *   SP         = __stack_top & ~0x3F  (64-byte cache-line aligned)
 *
 * ADR-001 compliance: no `use std::*`, no POSIX, no libc, no dynamic alloc.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026  SiliconLanguage — Monadic Hypervisor Project
 */

// ── Core Directives (Zero-Kernel Pillar) ────────────────────────────
//
// #![no_std]  — Sever the dependency on Rust's libstd and, transitively,
//               on libc/POSIX.  Only libcore is available (ADR-001 §1).
//
// #![no_main] — Suppress Rust's default `fn main()` ABI.  Our entry
//               point is `extern "C" fn hypervisor_main()`, called
//               directly from assembly at EL2.  There is no CRT0, no
//               __libc_start_main, and no argc/argv.

#![no_std]
#![no_main]

use core::arch::asm;
use core::arch::global_asm;
use core::panic::PanicInfo;

mod mm;
mod hw;
mod dataplane;

// ═══════════════════════════════════════════════════════════════════════
// §0  Boot Assembly — included via LLVM Integrated Assembler
// ═══════════════════════════════════════════════════════════════════════
//
// boot.S is processed by LLVM's integrated assembler (not the linker).
// This ensures .text.boot, .bss.stack, _start, and __stack_top are
// emitted into the compilation unit, then placed by linker.ld.
global_asm!(include_str!("../arch/arm64/boot/boot.S"));

// ═══════════════════════════════════════════════════════════════════════
// §1  Deterministic Panic Handler
// ═══════════════════════════════════════════════════════════════════════
//
// ADR-001 §5 mandates a custom `#[panic_handler]` that:
//   • Never allocates (no heap exists at EL2).
//   • Never calls std or POSIX.
//   • Diverges (`-> !`) — execution cannot resume after a hypervisor panic.
//
// On panic we park the core in a low-power WFE loop — the same strategy
// used by boot.S for fatal EL mismatch.  This is safe because:
//
//   1. WFE halts the pipeline until an event (SEV/IRQ/debug halt).
//   2. The infinite branch prevents the core from executing stale
//      instructions or corrupting Stage-2 state.
//   3. Power draw drops to near-idle (Neoverse N2/V2 clock-gates
//      execution units during WFE).
//
// Future: wire `info` to a UART-backed `core::fmt::Write` sink so the
// PanicInfo (file, line, message) is visible on the serial console.

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // TODO: Log `_info` to PL011 UART via core::fmt::Write (no alloc).
    //       The UART base is memory-mapped at a platform-specific PA
    //       (e.g., 0x0900_0000 on virt, ACPI SPCR on Graviton).

    loop {
        // SAFETY: WFE is a hint instruction with no side effects on
        // architectural state.  `nomem` — no memory access.
        // `nostack` — no stack manipulation.  The core enters a
        // low-power standby until an event signal arrives, then
        // re-executes WFE via the loop.
        unsafe {
            asm!("wfe", options(nomem, nostack));
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// §2  Hypervisor Entry Point
// ═══════════════════════════════════════════════════════════════════════
//
// Called from arch/arm64/boot/boot.S:
//
//     bl  hypervisor_main          // Step 6 — hand off to Rust
//
// Contract:
//   • Executes at EL2 on the primary core (Aff0 == 0).
//   • SP is 64-byte aligned on a 16 KiB boot stack.
//   • HCR_EL2, VTCR_EL2, VTTBR_EL2 are pre-configured by assembly.
//   • Function is `-> !` (diverging) — must never return.
//     If it did, boot.S falls through to the WFE park loop.
//
// #[no_mangle]  — Emit the symbol `hypervisor_main` verbatim so the
//                 linker resolves the `bl hypervisor_main` in boot.S.
//
// extern "C"    — Use the C ABI (AAPCS64) for register-level
//                 compatibility with the assembly caller.

#[no_mangle]
pub extern "C" fn hypervisor_main() -> ! {
    // ── Zero-Kernel Boot Sequence ───────────────────────────────────
    //
    // Each subsystem initialiser is called in strict dependency order.
    // No dynamic dispatch, no trait objects, no allocations.

    // Phase 1 — Stage-2 MMU: construct 4KB translation tables and
    //           program VTTBR_EL2 with real IPA → PA mappings.
    //           Until this completes, VTTBR_EL2 = 0 (fail-closed).
    mm::stage2::stage2_mmu_init();

    // Phase 2 — vIOMMU / PCIe bypass: map the NVMe BAR0 into the
    //           guest IPA space via Stage-2 Device-nGnRE mapping and
    //           bind the device's DMA stream to our ROOT page table.
    //
    //           Addresses below are boot-time placeholders:
    //             nvme_bar0_pa    = 0x4000_0000 (physical NVMe BAR0)
    //             guest_bar0_ipa  = 0x1000_0000 (guest-visible IPA)
    //
    //           Real discovery: ECAM (MCFG ACPI table) at runtime.
    hw::viommu::viommu_pcie_bypass_init(0x4000_0000, 0x1000_0000);

    // Phase 3 — Data-plane poll loop: enter the infinite polling loop
    //           driving lock-free SPSC ring buffers for zero-copy I/O.
    //           This function is `-> !` — it never returns.
    dataplane::poll::dataplane_poll_loop();
}

// ═══════════════════════════════════════════════════════════════════════
// §3  Subsystem Notes
// ═══════════════════════════════════════════════════════════════════════
//
// stage2_mmu_init()        — src/mm/stage2.rs (LPAE Stage-2 tables)
// viommu_pcie_bypass_init  — src/hw/viommu.rs (True PCIe Bypass)
// dataplane_poll_loop      — src/dataplane/poll.rs (SPSC + NVMe poll)
