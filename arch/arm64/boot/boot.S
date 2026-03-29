/*
 * arch/arm64/boot/boot.S — Monadic Hypervisor EL2 bare-metal entry point
 *
 * First instruction executed after UEFI ExitBootServices() hands off control.
 * We execute STRICTLY at Exception Level 2 (EL2) — the ARMv8-A privilege
 * level architecturally reserved for hypervisors. No POSIX, no host OS,
 * no kernel beneath us (ADR-001).
 *
 * Hardware targets:
 *   Primary  — AWS Graviton4 (Neoverse V2), Azure Cobalt 100 (Neoverse N2)
 *   Compat   — AWS Graviton2 (Neoverse N1), Graviton3 (Neoverse V1)
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026  SiliconLanguage — Monadic Hypervisor Project
 */

        .arch   armv8-a

/* ====================================================================
 * .text.boot — entry section; linker script must place this first.
 * ==================================================================== */
        .section .text.boot, "ax", @progbits
        .global _start
        .type   _start, %function

_start:
        /* ────────────────────────────────────────────────────────────
         * Step 0 — Park secondary cores.
         *
         * Only the primary core (MPIDR_EL1.Aff0 == 0) continues.
         * Secondaries drop directly into the WFE park loop.
         * The Rust hypervisor wakes them later via SEV after
         * per-CPU structures are initialised.
         * ──────────────────────────────────────────────────────────── */
        mrs     x0, mpidr_el1
        and     x0, x0, #0xFF              /* Aff0 — core ID in cluster    */
        cbnz    x0, .Lpark_core

        /* ────────────────────────────────────────────────────────────
         * Step 1 — Verify Exception Level (CurrentEL).
         *
         * ARMv8-A ARM §D13.2.22  CurrentEL encoding:
         *
         *   Bits [3:2] = Exception Level
         *     0b00 → EL0   0b01 → EL1   0b10 → EL2   0b11 → EL3
         *
         * Proof of EL2:
         *
         *     CurrentEL[3:2] = 0b10
         *   ⟹ CurrentEL AND 0xC = 0x8           (isolate bits [3:2])
         *   ⟹ EL = (CurrentEL >> 2) = 2          ∎
         *
         * If the firmware dropped us at any other EL, we cannot
         * reach EL2 without secure-monitor cooperation (ERET from
         * EL3). No recovery path — park the core.
         * ──────────────────────────────────────────────────────────── */
        mrs     x1, CurrentEL
        and     x1, x1, #0xC               /* Isolate bits [3:2]            */
        cmp     x1, #0x8                    /* 0x8 ↔ EL2                    */
        b.ne    .Lpark_core                 /* Not EL2 → fatal, park        */

        /* ────────────────────────────────────────────────────────────
         * Step 2 — Hypervisor Configuration Register (HCR_EL2).
         *
         * ARMv8-A ARM §D13.2.46:
         *
         *   Bit [31]  RW = 1  → EL1 executes in AArch64 state.
         *                       Guest kernels run in 64-bit mode.
         *   Bit [0]   VM = 1  → Enable Stage-2 address translation.
         *                       IPA→PA walks via VTTBR_EL2 are active.
         *
         *   Encoding: (1 << 31) | (1 << 0) = 0x8000_0001
         *
         * We deliberately leave all trap bits (IMO, FMO, AMO, TWI,
         * TWE, TVM, …) at 0. The Rust hypervisor programs per-VM
         * trap policy at vCPU creation time — no premature trapping.
         * ──────────────────────────────────────────────────────────── */
        movz    x2, #0x0001                 /* VM bit [0]                    */
        movk    x2, #0x8000, lsl #16        /* RW bit [31]                   */
        msr     hcr_el2, x2
        isb                                 /* Context-synchronising barrier */

        /* ────────────────────────────────────────────────────────────
         * Step 3 — Stage-2 Translation Control (VTCR_EL2).
         *
         * ARMv8-A ARM §D13.2.143:
         *
         *   Field   Bits      Value   Meaning
         *   ─────   ────      ─────   ──────────────────────────────
         *   T0SZ    [5:0]     24      40-bit IPA  (2^40 = 1 TiB)
         *   SL0     [7:6]     0b01    Start walk at Level 1
         *   IRGN0   [9:8]     0b01    Inner WB-RA-WA Cacheable
         *   ORGN0   [11:10]   0b01    Outer WB-RA-WA Cacheable
         *   SH0     [13:12]   0b11    Inner Shareable (coherent)
         *   TG0     [15:14]   0b00    4 KB translation granule  ← KEY
         *   PS      [18:16]   0b010   40-bit PA    (1 TiB phys)
         *
         * Composite value = 0x0002_3558:
         *
         *    (0b010 << 16)  PS   = 40-bit physical address size
         *  | (0b00  << 14)  TG0  = 4 KB granule (natural for N1/N2/V2)
         *  | (0b11  << 12)  SH0  = Inner-Shareable for multi-core coherence
         *  | (0b01  << 10)  ORGN0= Write-Back Write-Allocate
         *  | (0b01  <<  8)  IRGN0= Write-Back Write-Allocate
         *  | (0b01  <<  6)  SL0  = Level-1 start (covers 1 TiB with 4KB)
         *  | (24)           T0SZ = IPA width = 64 − 24 = 40 bits
         *
         * TG0 = 0b00 selects the 4 KB granule for Stage-2 page table
         * walks. This is the native page size on all Neoverse cores
         * (N1, N2, V1, V2) and matches the L1D/L1I TLB geometry.
         * ──────────────────────────────────────────────────────────── */
        movz    x3, #0x3558                 /* T0SZ|SL0|IRGN0|ORGN0|SH0     */
        movk    x3, #0x0002, lsl #16        /* PS = 0b010 (40-bit PA)        */
        msr     vtcr_el2, x3
        isb

        /* ────────────────────────────────────────────────────────────
         * Step 4 — Zero VTTBR_EL2 (security placeholder).
         *
         * VTTBR_EL2 layout (ARMv8-A ARM §D13.2.144):
         *   Bits [63:48]  VMID  — Virtual Machine Identifier
         *   Bits [47:1]   BADDR — Phys base of Stage-2 table
         *   Bit  [0]      CnP   — Common-not-Private (v8.2)
         *
         * Zeroing guarantees:
         *   • No stale firmware VMID/BADDR leaks into hypervisor
         *   • Any Stage-2 walk before Rust programs real tables
         *     produces Translation Fault → safe fail-closed
         *
         * The #![no_std] Rust hypervisor programs real IPA→PA
         * mappings per-VM:  hypervisor_main → vm_create →
         * stage2_map() → msr vttbr_el2, <table_base | vmid>.
         * ──────────────────────────────────────────────────────────── */
        msr     vttbr_el2, xzr
        isb

        /* ────────────────────────────────────────────────────────────
         * Step 5 — Load EL2 stack pointer (64-byte aligned).
         *
         * Cache-line alignment rationale:
         *
         *   Neoverse N1/N2/V2 L1D cache line = 64 bytes.
         *   alignas(64) on the stack top prevents false sharing
         *   between the boot stack and adjacent data-plane
         *   structures (SPSC ring tails, vCPU contexts, etc.).
         *
         * The linker places .bss.stack at a 64-byte boundary
         * (enforced by .balign 64 below). The AND mask is a
         * defence-in-depth belt — it costs one cycle and
         * guarantees alignment even if the linker script is loose.
         *
         * ARM64 SP must be at minimum 16-byte aligned (ABI).
         * We enforce 64-byte ⊃ 16-byte, so ABI is satisfied.
         * ──────────────────────────────────────────────────────────── */
        adrp    x4, __stack_top
        add     x4, x4, :lo12:__stack_top
        and     x4, x4, #~0x3F             /* Mask low 6 bits → 64B align   */
        mov     sp, x4

        /* ────────────────────────────────────────────────────────────
         * Step 6 — Hand off to #![no_std] Rust entry point.
         *
         *   extern "C" fn hypervisor_main() -> !
         *
         * Declared in src/main.rs.  The function is diverging
         * (-> !) per ADR-001 — it must never return.  If it
         * does return due to a logic error, we fall through
         * to the park loop below.
         * ──────────────────────────────────────────────────────────── */
        bl      hypervisor_main

        /* ──────────────────── UNREACHABLE ──────────────────────────
         * hypervisor_main() is -> ! (diverging).  If we reach here,
         * something is catastrophically wrong.  Park the core.
         * ──────────────────────────────────────────────────────────── */

        /* ────────────────────────────────────────────────────────────
         * Low-power core park — WFE spin loop.
         *
         * WFE (Wait For Event) halts the core pipeline and enters
         * a low-power standby state until an event arrives:
         *   • SEV/SEVL from another core
         *   • Physical IRQ/FIQ/SError
         *   • Debug halt event
         *
         * The branch creates an infinite park — this core is done.
         * ──────────────────────────────────────────────────────────── */
.Lpark_core:
        wfe
        b       .Lpark_core

        .size   _start, . - _start


/* ====================================================================
 * .bss.stack — EL2 boot stack (zero-initialised, costs 0 image bytes).
 *
 * Stack grows downward (high → low).  SP is loaded with __stack_top.
 * 16 KiB is sufficient for the synchronous EL2 boot path; the Rust
 * hypervisor allocates per-vCPU stacks from the frame allocator.
 *
 * .balign 64 ← enforces alignas(64) / cache-line boundary.
 * ==================================================================== */
        .section .bss.stack, "aw", @nobits
        .balign 64
__stack_bottom:
        .space  16384                       /* 16 KiB boot stack             */
__stack_top:
