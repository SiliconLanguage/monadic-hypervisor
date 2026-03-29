/*
 * src/hw/viommu.rs — True PCIe Bypass via Stage-2 Device Assignment
 *
 * Implements the Hardware-Enlightened Pillar: guest VMs receive direct,
 * unmediated access to physical PCIe endpoints (NVMe SSDs, ENA NICs)
 * by mapping device BAR regions into the guest IPA space through our
 * Stage-2 translation tables.
 *
 * ═══════════════════════════════════════════════════════════════════
 *                   True PCIe Bypass Architecture
 * ═══════════════════════════════════════════════════════════════════
 *
 *   Guest VM (EL1)               Hypervisor (EL2)           Silicon
 *  ┌──────────────┐            ┌──────────────────┐    ┌────────────┐
 *  │ NVMe driver  │            │                  │    │            │
 *  │              │─── MMIO ──→│  Stage-2 MMU     │───→│ NVMe BAR0  │
 *  │ writes to    │  (IPA)     │  IPA → PA        │(PA)│ Doorbell   │
 *  │ doorbell reg │            │  (Device-nGnRE)  │    │ Register   │
 *  └──────────────┘            │                  │    └────────────┘
 *                              │                  │    ┌────────────┐
 *  ┌──────────────┐            │                  │    │            │
 *  │ NVMe DMA     │←── DMA ───│  SMMUv3 Stage-2  │←───│ NVMe DMA   │
 *  │ buffers      │  (IPA)     │  StreamID→VTTBR  │(PA)│ Engine     │
 *  │ (guest RAM)  │            │  same ROOT table │    │            │
 *  └──────────────┘            └──────────────────┘    └────────────┘
 *
 * CPU path: Guest MMIO store → Stage-2 walk → physical NVMe doorbell.
 * DMA path: NVMe DMA read  → SMMUv3 Stage-2 walk → guest RAM.
 *
 * Both paths use the SAME Stage-2 ROOT table (VTTBR_EL2.BADDR),
 * ensuring a unified, coherent view of the IPA→PA address space.
 *
 * ═══════════════════════════════════════════════════════════════════
 *                Device Memory Ordering on Neoverse V2
 * ═══════════════════════════════════════════════════════════════════
 *
 * The Neoverse V2 (Graviton4) implements an aggressive out-of-order
 * store pipeline with a 128-entry store buffer.  For Normal memory,
 * stores can be reordered, merged, and coalesced freely.
 *
 * NVMe doorbell registers are MMIO — they MUST be mapped as Device
 * memory to prevent the store buffer from:
 *
 *   1. Merging adjacent doorbell writes (breaks doorbell-per-queue).
 *   2. Reordering SQ tail before CQ head (breaks NVMe protocol).
 *   3. Caching doorbell values in L1D (stale data → missed commands).
 *
 * We select Device-nGnRE (not nGnRnE) because NVMe doorbells are
 * fire-and-forget: the controller does not return a value that the
 * driver reads back immediately.  Early Write Acknowledgement lets
 * the V2 store buffer retire the MMIO write without stalling for
 * the PCIe completion TLP (~200–400 ns round-trip saved per write).
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026  SiliconLanguage — Monadic Hypervisor Project
 */

use crate::mm::stage2;

// ═══════════════════════════════════════════════════════════════════════
// §1  PCIe BAR Mapping via Stage-2 Device Assignment
// ═══════════════════════════════════════════════════════════════════════

/// Map physical NVMe BAR0 into the guest VM's IPA space with
/// Device-nGnRE ordering and bind the device's DMA stream to
/// our Stage-2 tables via SMMUv3.
///
/// This is the core of **True PCIe Bypass** (Hardware-Enlightened
/// Pillar): after this call, the guest's NVMe driver at EL1 can
/// issue MMIO writes directly to the physical NVMe doorbell registers
/// through Stage-2 translation — no hypervisor trap, no emulation,
/// no memcpy.
///
/// # Arguments
///
/// * `nvme_bar0_pa` — Physical base address of the NVMe controller's
///   BAR0 region.  On AWS Nitro / Graviton4 this is discovered via the
///   ECAM configuration space (MCFG ACPI table) or device tree.
///   Must be 4 KiB-aligned.
///
/// * `guest_bar0_ipa` — Intermediate Physical Address where the guest
///   VM expects to find the NVMe BAR0.  The guest's NVMe driver will
///   issue MMIO reads/writes to this IPA; the Stage-2 MMU translates
///   them to `nvme_bar0_pa`.  Must be 4 KiB-aligned.
///
/// # Memory Attributes
///
/// The mapping uses `S2_DEVICE_NGNRE_RW`:
///
///   - **Device-nGnRE**: non-Gathering, non-Reordering, Early Write Ack
///   - **Non-Shareable**: device MMIO is not cacheable/shareable
///   - **Read-Write**: guest driver needs both doorbell writes and
///     BAR register reads (capability discovery, status polling)
///   - **Execute-Never**: no instruction fetch through device MMIO
///   - **Access Flag set**: no first-access trap
///
/// # SMMUv3 DMA Binding
///
/// After the CPU-side BAR mapping, we bind the NVMe device's PCIe
/// Stream ID to our Stage-2 ROOT table via `smmuv3_bind_stream_id()`.
/// This ensures the device's DMA engine translates guest IPAs through
/// the same page tables as CPU accesses — a unified address space for
/// both MMIO and DMA.
///
/// # Pillar Compliance
///
/// - **0-Copy**: DMA flows directly from NVMe to guest RAM via Stage-2;
///   no bounce buffer, no intermediate copy.
/// - **0-Kernel**: No POSIX, no vhost, no kernel module in the path.
/// - **Hardware-Enlightened**: PCIe TLPs reach the guest at line rate,
///   mediated only by the Stage-2 MMU hardware walker.
///
/// # Panics
///
/// Panics (via `map_4kb_page`) if the Stage-2 sub-table pool is
/// exhausted.  The panic handler in main.rs parks the core in WFE.
#[inline(never)]
pub fn viommu_pcie_bypass_init(nvme_bar0_pa: u64, guest_bar0_ipa: u64) {
    // ── Step 1 — Map NVMe BAR0 into guest IPA space ─────────────────
    //
    // Install a Stage-2 leaf descriptor at L3 level:
    //
    //   L1[ IPA[39:30] ] → L2[ IPA[29:21] ] → L3[ IPA[20:12] ]
    //
    //   Leaf = (nvme_bar0_pa & ADDR_MASK) | S2_DEVICE_NGNRE_RW
    //
    // After this call, any guest EL1 access to `guest_bar0_ipa`
    // triggers a Stage-2 walk that resolves to `nvme_bar0_pa` with
    // Device-nGnRE attributes.  The Neoverse V2 TLB caches the
    // translation; subsequent doorbell writes hit the TLB and bypass
    // the walker entirely (~1 cycle vs ~20 cycles for a walk miss).
    stage2::map_4kb_page(
        guest_bar0_ipa,
        nvme_bar0_pa,
        stage2::S2_DEVICE_NGNRE_RW,
    );

    // ── Step 2 — Bind SMMUv3 stream for DMA isolation ───────────────
    //
    // The NVMe controller's DMA engine initiates PCIe read/write TLPs
    // using guest IPAs (the addresses the guest driver programmed into
    // the SQ entries).  Without SMMUv3 binding, these DMA transactions
    // would use raw physical addresses — bypassing Stage-2 isolation
    // and allowing the device to read/write ANY physical memory.
    //
    // By programming the SMMUv3 Stream Table Entry (STE) to translate
    // the device's PCIe Stream ID through our ROOT page table, we
    // enforce the same IPA→PA translation for DMA as for CPU accesses.
    //
    // Stream ID 0x0100 is a placeholder; real discovery comes from the
    // ECAM Configuration Space (Bus:Device.Function → Stream ID mapping
    // defined by the platform's IORT ACPI table or device tree).
    //
    // SAFETY: See documentation on smmuv3_bind_stream_id() below.
    unsafe {
        smmuv3_bind_stream_id(0x0100, stage2::get_vttbr());
    }
}


// ═══════════════════════════════════════════════════════════════════════
// §2  SMMUv3 DMA Stream Binding (Stub)
// ═══════════════════════════════════════════════════════════════════════
//
// The ARM System Memory Management Unit v3 (SMMUv3) translates
// device-initiated DMA transactions using a two-level lookup:
//
//   1. Stream Table: PCIe Stream ID → Stream Table Entry (STE)
//   2. STE contains a pointer to a Context Descriptor (CD)
//   3. CD contains S2TTB — the Stage-2 Translation Table Base
//      (identical to VTTBR_EL2.BADDR for unified CPU/DMA translation)
//
// ┌─────────────┐     ┌─────────┐     ┌──────────────────────┐
// │ PCIe Device  │────→│  Stream  │────→│  Context Descriptor   │
// │ Stream ID    │     │  Table   │     │  S2TTB = ROOT table   │
// │ 0x0100       │     │  Entry   │     │  (same as VTTBR_EL2)  │
// └─────────────┘     └─────────┘     └──────────────────────┘
//                                              │
//                                              ▼
//                                     ┌──────────────────┐
//                                     │  Stage-2 ROOT    │
//                                     │  (shared with CPU)│
//                                     │  IPA → PA tables  │
//                                     └──────────────────┘
//
// By pointing S2TTB at the same ROOT table that VTTBR_EL2 uses,
// CPU and DMA see an identical IPA→PA mapping.  A page mapped for
// DMA at IPA X resolves to the same PA whether the access comes
// from the NVMe controller's DMA engine or the guest's CPU.
//
// On AWS Nitro (Graviton2/3/4):
//
//   Nitro does NOT expose a programmable SMMUv3 to the hypervisor.
//   Instead, the Nitro card hardware enforces DMA isolation at the
//   PCIe root complex level — each VF (Virtual Function) is
//   pre-bound to a specific VM's physical address window by the
//   Nitro firmware.  In this case, smmuv3_bind_stream_id() is a
//   no-op on Graviton targets and the DMA isolation guarantee comes
//   from Nitro hardware rather than SMMUv3 software programming.
//
// On Azure Cobalt 100 (Neoverse N2) with SMMU:
//
//   Azure exposes a programmable SMMUv3.  The hypervisor must
//   program the Stream Table and Context Descriptors to bind each
//   assigned device's Stream ID to the VM's Stage-2 tables.
//
// Implementation roadmap:
//
//   1. Discover SMMU base address from IORT ACPI table.
//   2. Map SMMU MMIO pages into the hypervisor's address space
//      (EL2 identity map, Device-nGnRnE attributes).
//   3. Locate the Stream Table Entry for `stream_id`.
//   4. Write STE.Config = 0b110 (Stage-2 translation enabled).
//   5. Write STE.S2TTB = vttbr & ADDR_MASK (Stage-2 table base).
//   6. Write STE.S2T0SZ, S2SL0, S2TG matching our VTCR_EL2 config.
//   7. Issue SMMU CMDQ_CFGI_STE + CMDQ_TLBI_S12 to invalidate.
//   8. Poll SMMU CMDQ for completion.

/// Bind a PCIe device's DMA traffic to our Stage-2 translation tables
/// via the ARM SMMUv3 Stream Table.
///
/// After this call, all DMA transactions from the device identified by
/// `stream_id` are translated through the same Stage-2 page tables
/// pointed to by `vttbr` — enforcing identical IPA→PA mappings for
/// both CPU and DMA accesses.
///
/// # Arguments
///
/// * `stream_id` — PCIe Stream ID (typically derived from the
///   Bus:Device.Function of the assigned endpoint via the platform's
///   IORT ACPI table or device tree `iommu-map` property).
///
/// * `vttbr` — The VTTBR_EL2 value containing the VMID and Stage-2
///   root table physical address.  Obtained from `stage2::get_vttbr()`.
///   The S2TTB field in the SMMUv3 Context Descriptor will be set to
///   `vttbr & ADDR_MASK` (extract BADDR, discard VMID bits).
///
/// # Safety
///
/// This function is `unsafe` because:
///
///   1. **MMIO writes to SMMUv3 registers** — incorrect Stream Table
///      programming can break DMA isolation between VMs, allowing a
///      device to DMA into another VM's physical memory.
///
///   2. **Platform-dependent base address** — the SMMU base must be
///      discovered from IORT/DT and mapped with Device-nGnRnE before
///      any register access.  Using an incorrect base causes bus
///      errors or silent corruption.
///
///   3. **Stream ID validity** — the caller must guarantee that
///      `stream_id` corresponds to a device actually assigned to the
///      target VM.  Binding an unassigned device's Stream ID to a VM's
///      tables would grant that VM DMA access to the device.
///
/// # Current Status: Stub
///
/// This function is intentionally a no-op.  The implementation
/// requires SMMU MMIO register access (CMDQ, STRTAB) which will be
/// built in `src/hal/arm64/smmu.rs` under the HAL unsafe audit
/// policy (ADR-001 §4).
///
/// On AWS Graviton (Nitro), this function remains a no-op permanently:
/// Nitro hardware enforces DMA isolation at the PCIe root complex.
#[allow(unused_variables)]
unsafe fn smmuv3_bind_stream_id(stream_id: u16, vttbr: u64) {
    // TODO: Implement SMMUv3 Stream Table programming.
    //
    // Platform detection:
    //   if nitro_detected() {
    //       return;  // Nitro hardware handles DMA isolation.
    //   }
    //
    // SMMUv3 programming sequence:
    //   1. let smmu_base = discover_smmu_base_from_iort();
    //   2. let ste = &mut stream_table[stream_id as usize];
    //   3. ste.config = STE_CONFIG_S2_TRANSLATE;  // 0b110
    //   4. ste.s2ttb  = vttbr & ADDR_MASK;
    //   5. ste.s2t0sz = 24;  // Match VTCR_EL2.T0SZ
    //   6. ste.s2sl0  = 1;   // Match VTCR_EL2.SL0
    //   7. ste.s2tg   = 0;   // Match VTCR_EL2.TG0 (4KB)
    //   8. smmu_cmdq_issue(CMDQ_CFGI_STE, stream_id);
    //   9. smmu_cmdq_issue(CMDQ_TLBI_S12_VMALL, vmid);
    //  10. smmu_cmdq_sync();
}
