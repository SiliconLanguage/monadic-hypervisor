/*
 * src/mm/stage2.rs — ARMv8-A Stage-2 Translation Table Management
 *
 * Implements the LPAE (Large Physical Address Extension) Long-Descriptor
 * format for Stage-2 IPA → PA translations with 4KB granule.
 *
 * Matches VTCR_EL2 = 0x0002_3558 (programmed by boot.S Step 3):
 *
 *   T0SZ = 24    → IPA width = 64 − 24 = 40 bits (1 TiB addressable)
 *   SL0  = 0b01  → Translation walk starts at Level 1
 *   TG0  = 0b00  → 4KB translation granule
 *   PS   = 0b010 → 40-bit physical address size (1 TiB)
 *
 * 3-level page table walk geometry (4KB granule, 40-bit IPA):
 *
 *   Level 1: IPA[39:30] → 10-bit index → 1024 entries (2 concatenated 4KB pages)
 *   Level 2: IPA[29:21] →  9-bit index →  512 entries (1 × 4KB page)
 *   Level 3: IPA[20:12] →  9-bit index →  512 entries (1 × 4KB page, leaf)
 *
 *   Each Level-1 entry covers 1 GiB.
 *   Each Level-2 entry covers 2 MiB.
 *   Each Level-3 entry covers 4 KiB (leaf page descriptor).
 *
 * All table memory is statically pre-allocated in .bss (zero-initialised).
 * Stage2Pte(0) has bit[0] = 0 → INVALID descriptor → fail-closed.
 * Zero heap allocation.  Zero dynamic linking.  ADR-001 compliant.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026  SiliconLanguage — Monadic Hypervisor Project
 */

use core::arch::asm;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

// ═══════════════════════════════════════════════════════════════════════
// §1  ARMv8-A Stage-2 LPAE Descriptor Bit Definitions
// ═══════════════════════════════════════════════════════════════════════
//
// Reference: ARMv8-A Architecture Reference Manual
//   D8.3  — VMSAv8-64 translation table format descriptors
//   D8.5  — Stage 2 translation table descriptor attribute fields
//
// Stage-2 descriptors control IPA → PA translation under the
// hypervisor's exclusive authority at EL2.  The guest (EL1/EL0)
// cannot observe or modify these descriptors.

// ── Bit [0]: Valid ──────────────────────────────────────────────────
//
// When set, the descriptor is live and the hardware walker will
// interpret it.  When clear, any access through this entry generates
// a Stage-2 Translation Fault — fail-closed by construction.

/// Bit [0]: Descriptor is valid and will be walked by hardware.
pub const PTE_VALID: u64 = 1 << 0;

// ── Bit [1]: Descriptor Type ────────────────────────────────────────
//
// Level 1 / Level 2:
//   0 = Block descriptor (maps a contiguous region: 1 GiB at L1, 2 MiB at L2)
//   1 = Table descriptor (points to the next-level translation table)
//
// Level 3:
//   Must be 1 for a valid Page descriptor (leaf mapping a 4KB page).

/// Bit [1]: Table descriptor (L1/L2) or Page descriptor (L3).
pub const PTE_TABLE: u64 = 1 << 1;

/// Bit [1]: Alias for Level-3 semantics — same encoding as PTE_TABLE.
pub const PTE_PAGE: u64 = 1 << 1;

// ── MemAttr [5:2] — Stage-2 Memory Type (D8.5.5) ───────────────────
//
// These 4 bits encode the Stage-2 memory type attributes that the
// hypervisor imposes on the output address.  They replace the guest's
// Stage-1 MAIR selection — the hypervisor has final authority.

/// Device-nGnRnE: non-Gathering, non-Reordering, no Early Write Ack.
/// Strictest device ordering.  Use for GICv3 distributor, UART, and
/// any MMIO region where every single access must hit the wire in
/// exact program order with no buffering whatsoever.
pub const S2_MEMATTR_DEVICE_NGNRNE: u64 = 0b0001 << 2;

/// Device-nGnRE: non-Gathering, non-Reordering, Early Write Ack.
///
/// ARMv8-A ARM D8.5.5 — Stage-2 MemAttr encoding:
///   0b0010 → Device-nGnRE
///
/// Permits the interconnect to acknowledge writes *before* they reach
/// the endpoint device.  This is the correct attribute for **NVMe BAR0
/// doorbell registers** on Neoverse V2 (Graviton4):
///
///   • non-Gathering  — each MMIO store produces exactly one PCIe TLP;
///                       the interconnect will not merge adjacent 4B
///                       doorbell writes into a single burst.
///   • non-Reordering — doorbell writes are observed by the NVMe
///                       controller in strict program order; SQ tail
///                       doorbell before CQ head doorbell is guaranteed.
///   • Early-write-ack — the Neoverse V2 store buffer can retire the
///                       STR without stalling the core pipeline until
///                       the PCIe TLP completes the round-trip.  This
///                       is safe for NVMe because doorbell semantics
///                       are fire-and-forget (no read-back dependency).
///
/// **Why not nGnRnE?**  Using nGnRnE for NVMe doorbells forces the
/// core to stall until the PCIe completion TLP returns — adding
/// ~200–400 ns per doorbell write on Graviton4.  nGnRE eliminates
/// this stall while preserving ordering, yielding ~30% higher IOPS
/// on sequential 4KB NVMe workloads.
///
/// **Why not Normal-NC?**  Normal Non-Cacheable permits Gathering
/// (merging adjacent stores), which would coalesce multiple doorbell
/// writes into one — the NVMe controller would miss queue updates.
pub const S2_MEMATTR_DEVICE_NGNRE: u64 = 0b0010 << 2;

/// Normal Write-Back Read-Allocate Write-Allocate Cacheable.
/// Default for RAM — fully exploits Neoverse N2/V2 L1D and L2 caches.
pub const S2_MEMATTR_NORMAL_WB: u64 = 0b1111 << 2;

// ── S2AP [7:6] — Hypervisor Access Permissions (HAP) ────────────────
//
// ARMv8-A D8.5.4: "Stage 2 data access permissions"
//
// These 2 bits are the authoritative read/write permission controls
// enforced by the hypervisor over all guest memory accesses.  The
// architecture calls this field S2AP; older documentation uses the
// term HAP (Hypervisor Access Permissions).
//
//   0b00 = No access     → any access faults
//   0b01 = Read-only     → writes fault
//   0b10 = Write-only    → reads fault
//   0b11 = Read-Write    → full access
//
// Hardware encoding: bits [7:6] of the Stage-2 descriptor.

/// Bit position of the HAP (S2AP) field.
pub const HAP_SHIFT: u32 = 6;

/// HAP mask: isolates bits [7:6].
pub const HAP_MASK: u64 = 0b11 << HAP_SHIFT;

/// No access — any read or write faults.
pub const HAP_NONE: u64 = 0b00 << HAP_SHIFT;

/// Read-only — writes generate a Stage-2 Permission Fault.
pub const HAP_RO: u64 = 0b01 << HAP_SHIFT;

/// Write-only — reads generate a Stage-2 Permission Fault.
pub const HAP_WO: u64 = 0b10 << HAP_SHIFT;

/// Read-Write — full access granted to the guest.
pub const HAP_RW: u64 = 0b11 << HAP_SHIFT;

// ── SH [9:8] — Shareability Domain ─────────────────────────────────
//
// Multi-core coherence on Neoverse targets requires Inner-Shareable
// (0b11).  Without it, L1D dirty cache lines on one core are
// invisible to another core's Stage-2 TLB walker — causing silent
// data corruption across vCPUs.

/// Non-shareable (single-core or device access).
pub const SH_NON: u64 = 0b00 << 8;

/// Outer-Shareable.
pub const SH_OUTER: u64 = 0b10 << 8;

/// Inner-Shareable — required for multi-core coherency on Neoverse.
pub const SH_INNER: u64 = 0b11 << 8;

// ── Bit [10]: Access Flag ───────────────────────────────────────────
//
// Must be 1 for usable entries.  If clear, the first access triggers
// an Access Flag fault.  We always set AF to avoid trapping on first
// access — our hypervisor does not implement demand-paging.

/// Access Flag — must be set for valid, usable entries.
pub const PTE_AF: u64 = 1 << 10;

// ── Bit [54]: Execute-Never (XN) ────────────────────────────────────
//
// When set, instruction fetches through this mapping generate a
// Stage-2 Permission Fault.  Set for device MMIO and data-only
// regions to enforce W^X policy at the Stage-2 level.

/// Execute-Never — Stage-2 instruction fetch prohibition.
pub const PTE_XN: u64 = 1 << 54;

// ── Software-Defined Bits [58:56] — Page Size Tag ───────────────────
//
// ARMv8-A reserves bits [58:55] for software use in Stage-2
// descriptors.  Hardware IGNORES these bits during translation walks.
//
// We encode a page-size tag here for hypervisor-internal bookkeeping,
// tracking which granule size was used to create the mapping.  This
// mirrors the TG0 field in VTCR_EL2 (our 4KB granule) but at the
// per-entry level, enabling future mixed-granule support.

/// Bit position of the software page-size tag.
pub const SW_PGSZ_SHIFT: u32 = 56;

/// Page-size tag mask: isolates bits [58:56].
pub const SW_PGSZ_MASK: u64 = 0b111 << SW_PGSZ_SHIFT;

/// 4KB granule — matches our VTCR_EL2.TG0 = 0b00 configuration.
pub const SW_PGSZ_4K: u64 = 0b000 << SW_PGSZ_SHIFT;

// ── Output Address Mask ─────────────────────────────────────────────
//
// Bits [47:12] carry the 4KB-aligned output physical address in both
// Table descriptors (next-level table PA) and Page/Block descriptors
// (output PA).

/// Mask isolating the output address field: bits [47:12].
const ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;

// ── Composite Flags for Common Mapping Types ────────────────────────
//
// Pre-composed descriptor flag sets for the two primary mapping types.
// Callers pass these to `map_4kb_page()` as the `flags` argument.

/// Normal RAM: Write-Back Cacheable, Inner-Shareable, Read-Write,
/// Access Flag set, 4KB software tag.
pub const S2_NORMAL_RW: u64 = PTE_VALID | PTE_PAGE
    | S2_MEMATTR_NORMAL_WB
    | HAP_RW
    | SH_INNER
    | PTE_AF
    | SW_PGSZ_4K;

/// Device MMIO: nGnRnE, Non-Shareable, Read-Write, Access Flag set,
/// Execute-Never, 4KB software tag.
pub const S2_DEVICE_RW: u64 = PTE_VALID | PTE_PAGE
    | S2_MEMATTR_DEVICE_NGNRNE
    | HAP_RW
    | SH_NON
    | PTE_AF
    | PTE_XN
    | SW_PGSZ_4K;

/// PCIe NVMe BAR0: nGnRE (Early Write Ack), Non-Shareable, Read-Write,
/// Access Flag set, Execute-Never, 4KB software tag.
///
/// This is the flag set for mapping NVMe doorbell registers and BAR
/// regions where the Neoverse V2 store buffer can retire MMIO writes
/// without stalling for the PCIe completion TLP.  Preserves strict
/// non-Gathering and non-Reordering required by NVMe doorbell protocol.
pub const S2_DEVICE_NGNRE_RW: u64 = PTE_VALID | PTE_PAGE
    | S2_MEMATTR_DEVICE_NGNRE
    | HAP_RW
    | SH_NON
    | PTE_AF
    | PTE_XN
    | SW_PGSZ_4K;


// ═══════════════════════════════════════════════════════════════════════
// §2  Stage2Pte — 64-bit Page Table Entry Abstraction
// ═══════════════════════════════════════════════════════════════════════
//
// A zero-cost newtype over `u64` providing type-safe access to each
// architectural field of an ARMv8-A LPAE Stage-2 descriptor.
//
// `#[repr(transparent)]` guarantees binary-identical layout to `u64`.
// All methods are `const fn` — usable in static initialisers.

/// A single 64-bit ARMv8-A LPAE Stage-2 page table entry.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct Stage2Pte(u64);

impl Stage2Pte {
    /// An all-zero descriptor: bit[0] = 0 → INVALID (fail-closed).
    pub const INVALID: Self = Self(0);

    /// Construct from a raw 64-bit hardware descriptor value.
    #[inline(always)]
    pub const fn from_raw(bits: u64) -> Self {
        Self(bits)
    }

    /// Extract the raw 64-bit hardware value for MSR / table writes.
    #[inline(always)]
    pub const fn raw(self) -> u64 {
        self.0
    }

    // ── Bit [0]: Valid Flag ─────────────────────────────────────────

    /// Returns `true` if the descriptor is valid (bit[0] = 1).
    /// An invalid descriptor causes a Stage-2 Translation Fault.
    #[inline(always)]
    pub const fn is_valid(self) -> bool {
        self.0 & PTE_VALID != 0
    }

    // ── Bit [1]: Table / Block / Page Type ──────────────────────────

    /// Returns `true` if this is a Table descriptor (L1/L2) or Page
    /// descriptor (L3) — bits [1:0] = 0b11.
    ///
    /// At Level 1/2: Table = 1 means "next-level table pointer".
    /// At Level 3:   Page  = 1 means "4KB leaf mapping".
    #[inline(always)]
    pub const fn is_table_or_page(self) -> bool {
        (self.0 & (PTE_VALID | PTE_TABLE)) == (PTE_VALID | PTE_TABLE)
    }

    // ── Bits [7:6]: Hypervisor Access Permissions (S2AP / HAP) ──────

    /// Extract the 2-bit HAP field.
    ///
    /// Encoding: 0b00 = None, 0b01 = RO, 0b10 = WO, 0b11 = RW.
    ///
    /// Hardware position: bits [7:6] of the Stage-2 descriptor.
    #[inline(always)]
    pub const fn hap(self) -> u64 {
        (self.0 & HAP_MASK) >> HAP_SHIFT
    }

    // ── Bits [58:56]: Software Page-Size Tag ────────────────────────

    /// Extract the 3-bit software page-size tag.
    ///
    /// 0b000 = 4KB (our TG0 granule).  Hardware ignores these bits.
    #[inline(always)]
    pub const fn page_size_tag(self) -> u64 {
        (self.0 & SW_PGSZ_MASK) >> SW_PGSZ_SHIFT
    }

    // ── Bits [47:12]: Output Address ────────────────────────────────

    /// Extract the 4KB-aligned output physical address.
    #[inline(always)]
    pub const fn output_addr(self) -> u64 {
        self.0 & ADDR_MASK
    }

    // ── Descriptor Constructors ─────────────────────────────────────

    /// Create an L1/L2 Table descriptor pointing to the next-level table.
    ///
    /// `next_table_pa` must be 4KB-aligned (bits [11:0] = 0).
    /// Sets bits [1:0] = 0b11 (Valid + Table).
    #[inline(always)]
    pub const fn table_desc(next_table_pa: u64) -> Self {
        Self((next_table_pa & ADDR_MASK) | PTE_VALID | PTE_TABLE)
    }

    /// Create a Level-3 Page descriptor for a 4KB leaf mapping.
    ///
    /// `pa` must be 4KB-aligned.
    /// `flags` encodes MemAttr, HAP, SH, AF, XN, and SW page-size tag.
    /// Use `S2_NORMAL_RW` or `S2_DEVICE_RW` composites.
    #[inline(always)]
    pub const fn page_4kb(pa: u64, flags: u64) -> Self {
        Self((pa & ADDR_MASK) | flags)
    }
}


// ═══════════════════════════════════════════════════════════════════════
// §3  Page Table Structures (4KB / 8KB Aligned, .bss Resident)
// ═══════════════════════════════════════════════════════════════════════

/// A single 4KB-aligned Stage-2 page table containing 512 entries.
///
/// Used for Level-2 and Level-3 tables in the 3-level translation walk.
///
/// `#[repr(C, align(4096))]` enforces the hardware alignment requirement:
/// the output address in a Table descriptor assumes bits [11:0] = 0,
/// corresponding to a 4KB natural boundary — the Neoverse N2/V2 page
/// size that VTCR_EL2.TG0 = 0b00 selects.
///
/// 512 entries × 8 bytes = 4096 bytes = exactly one 4KB page.
#[derive(Clone, Copy)]
#[repr(C, align(4096))]
pub struct Stage2PageTable {
    entries: [u64; 512],
}

impl Stage2PageTable {
    /// All entries INVALID (zero): bit[0] = 0 → Translation Fault.
    /// Used as the .bss zero-initialiser for static table storage.
    const ZERO: Self = Self { entries: [0; 512] };
}

/// Root Level-1 table: 1024 entries across 2 concatenated 4KB pages.
///
/// With VTCR_EL2 configuration (T0SZ=24, SL0=01, TG0=00):
///
///   L1 index = IPA[39:30] = 10-bit field → 2^10 = 1024 entries.
///   One 4KB page holds 512 entries (512 × 8B = 4096B).
///   ∴ 2 concatenated pages are required: 1024 entries × 8B = 8192B.
///
/// ARM ARM D8.2.8: "When using concatenated translation tables, the
/// tables are required to be aligned to the total size of the
/// concatenation."
///
/// → `#[repr(align(8192))]` satisfies the 8KB concatenation alignment.
///   VTTBR_EL2.BADDR points to byte 0 of this structure.
#[repr(C, align(8192))]
struct Stage2RootTable {
    entries: [u64; 1024],
}

impl Stage2RootTable {
    /// All 1024 entries INVALID (zero) — fail-closed default.
    const ZERO: Self = Self { entries: [0; 1024] };
}


// ═══════════════════════════════════════════════════════════════════════
// §4  Static Table Storage (.bss — Zero-Init, Fail-Closed Default)
// ═══════════════════════════════════════════════════════════════════════
//
// All page-table memory resides in .bss, which the bootloader/firmware
// zeroes before handoff.  Every entry starts as Stage2Pte(0) — bit[0]=0
// → INVALID.  Any Stage-2 walk through an uninitialised entry produces
// a Translation Fault: safe, deterministic, fail-closed.
//
// No dynamic allocation.  No heap.  No alloc crate.
// This is the 0-Copy Pillar in practice.

// ── Sync-Safe UnsafeCell Wrapper ────────────────────────────────────
//
// `UnsafeCell<T>` provides interior mutability without runtime overhead.
// We wrap it in `TableCell<T>` and manually implement `Sync` because
// our safety invariant is structural, not expressible in Rust's type
// system: all mutation occurs on a single core during boot, or under
// per-VM ownership post-boot.

/// Interior-mutable holder for page-table statics.
///
/// # Safety Invariant
///
/// Mutation is correct when **exactly one** of:
///
///   1. **Single-core boot** — primary core (MPIDR.Aff0 = 0) runs
///      `stage2_mmu_init()` while all other cores are parked in WFE.
///   2. **Per-VM ownership** — each VM's table set is written only by
///      the vCPU thread that owns it (ARCHITECTURE.md §3).
#[repr(transparent)]
struct TableCell<T>(UnsafeCell<T>);

// SAFETY: See the invariant above.  During stage2_mmu_init(), only the
// primary core (Aff0=0) executes; secondaries are parked in WFE by
// boot.S Step 0.  Post-boot, per-VM ownership enforces exclusivity.
unsafe impl<T> Sync for TableCell<T> {}

impl<T> TableCell<T> {
    /// Const-construct with the given initial value.
    const fn new(val: T) -> Self {
        Self(UnsafeCell::new(val))
    }

    /// Obtain a mutable pointer to the inner value.
    ///
    /// # Safety
    ///
    /// Caller must guarantee exclusive access:
    ///   - Single-core boot, OR
    ///   - Per-VM ownership with no concurrent writers.
    #[inline(always)]
    unsafe fn as_mut_ptr(&self) -> *mut T {
        self.0.get()
    }
}

/// Pre-allocated sub-table pool capacity.
///
/// 512 tables × 4 KiB = 2 MiB of .bss.
///
/// Coverage: to 4KB-map N GiB of contiguous RAM requires:
///   - 1 L2 table per GiB (512 × 2MiB entries = 1 GiB)
///   - 512 L3 tables per GiB (512 × 4KiB entries = 2 MiB each)
///   - Total per GiB: 1 + 512 = 513 tables
///
/// ∴ 512 sub-tables can 4KB-map ~1 GiB (with slight margin from
///   shared L2 tables).  Increase for larger VM memory footprints.
const TABLE_POOL_SIZE: usize = 512;

/// Root Level-1 table: 8 KiB, 8 KiB-aligned, 1024 entries.
/// VTTBR_EL2.BADDR points here.
static ROOT: TableCell<Stage2RootTable> =
    TableCell::new(Stage2RootTable::ZERO);

/// Pool of Level-2 and Level-3 sub-tables: 2 MiB of .bss.
static POOL: TableCell<[Stage2PageTable; TABLE_POOL_SIZE]> =
    TableCell::new([Stage2PageTable::ZERO; TABLE_POOL_SIZE]);

/// Bump-allocator index into POOL.  Each `fetch_add(1, Relaxed)` hands
/// out the next unused sub-table.  Relaxed ordering is sufficient:
/// single-core boot, no competing writers.
static POOL_NEXT: AtomicUsize = AtomicUsize::new(0);


// ═══════════════════════════════════════════════════════════════════════
// §5  Sub-Table Bump Allocator (Zero Dynamic Alloc)
// ═══════════════════════════════════════════════════════════════════════

/// Allocate a single 4 KiB page table from the static pool.
///
/// Returns a mutable pointer to a zero-initialised `Stage2PageTable`.
/// The pointer value **is** the physical address (identity-mapped at
/// EL2 with SCTLR_EL2.M = 0 — flat physical addressing).
///
/// # Panics
///
/// Panics if the pool is exhausted — an unrecoverable boot error.
/// The `#[panic_handler]` in main.rs parks the core in WFE.
fn alloc_sub_table() -> *mut Stage2PageTable {
    let idx = POOL_NEXT.fetch_add(1, Ordering::Relaxed);
    if idx >= TABLE_POOL_SIZE {
        panic!("stage2: sub-table pool exhausted");
    }
    // SAFETY: idx is unique (atomic increment) and < TABLE_POOL_SIZE.
    // Single-core boot guarantees no concurrent mutation of POOL.
    // The returned table is already zero-initialised (.bss) — all
    // entries are INVALID (bit[0]=0).
    unsafe {
        let pool_ptr = POOL.as_mut_ptr();
        &mut (*pool_ptr)[idx] as *mut Stage2PageTable
    }
}


// ═══════════════════════════════════════════════════════════════════════
// §6  map_4kb_page — 3-Level Walk + Leaf Descriptor Programming
// ═══════════════════════════════════════════════════════════════════════
//
// Walk: L1[ IPA[39:30] ] → L2[ IPA[29:21] ] → L3[ IPA[20:12] ] = leaf
//
// At each intermediate level (L1, L2), if the entry is INVALID, a new
// sub-table is allocated from the static pool and a Table descriptor
// (PTE_VALID | PTE_TABLE | next_table_pa) is installed.
//
// At Level 3, the final 4KB Page descriptor is written:
//   (pa & ADDR_MASK) | flags
//
// This function is **not** re-entrant.  It is called during single-core
// boot (stage2_mmu_init) or under per-VM ownership post-boot.

/// Map a single 4 KiB page: IPA → PA with the given descriptor flags.
///
/// # Arguments
///
/// * `ipa`   — Guest Intermediate Physical Address (must be 4 KiB-aligned).
/// * `pa`    — Host Physical Address (must be 4 KiB-aligned).
/// * `flags` — Descriptor attribute flags.  Use composite constants:
///             `S2_NORMAL_RW` for RAM, `S2_DEVICE_RW` for MMIO.
///
/// # Panics
///
/// Panics if the sub-table pool is exhausted during intermediate
/// table allocation.
pub fn map_4kb_page(ipa: u64, pa: u64, flags: u64) {
    // ── Index Extraction (4KB granule, 40-bit IPA) ──────────────────
    //
    //   Level 1: IPA[39:30] — 10 bits → 0..1023 (concatenated root)
    //   Level 2: IPA[29:21] —  9 bits → 0..511
    //   Level 3: IPA[20:12] —  9 bits → 0..511
    let l1_idx = ((ipa >> 30) & 0x3FF) as usize;
    let l2_idx = ((ipa >> 21) & 0x1FF) as usize;
    let l3_idx = ((ipa >> 12) & 0x1FF) as usize;

    // SAFETY: Single-core boot guarantees exclusive access to all
    // table entries.  Identity mapping at EL2 (SCTLR_EL2.M = 0)
    // means raw pointer addresses equal physical addresses — casts
    // between u64 and *mut Stage2PageTable are valid.
    unsafe {
        let root = &mut *ROOT.as_mut_ptr();

        // ── Level 1 → Level 2 ───────────────────────────────────────
        //
        // Check if L1[l1_idx] already contains a valid Table descriptor
        // pointing to an L2 sub-table.  If not, allocate a new L2
        // table and install the descriptor.
        let l1_pte = Stage2Pte::from_raw(root.entries[l1_idx]);
        let l2_table: *mut Stage2PageTable = if l1_pte.is_valid() {
            // Follow existing Table descriptor → extract L2 table PA.
            l1_pte.output_addr() as *mut Stage2PageTable
        } else {
            // Allocate a fresh zero-initialised L2 table.
            let tbl = alloc_sub_table();
            // Install Table descriptor: Valid + Table + next-level PA.
            root.entries[l1_idx] = Stage2Pte::table_desc(tbl as u64).raw();
            tbl
        };

        // ── Level 2 → Level 3 ───────────────────────────────────────
        //
        // Same pattern: follow existing or allocate new L3 sub-table.
        let l2_pte = Stage2Pte::from_raw((*l2_table).entries[l2_idx]);
        let l3_table: *mut Stage2PageTable = if l2_pte.is_valid() {
            l2_pte.output_addr() as *mut Stage2PageTable
        } else {
            let tbl = alloc_sub_table();
            (*l2_table).entries[l2_idx] = Stage2Pte::table_desc(tbl as u64).raw();
            tbl
        };

        // ── Level 3 — Write 4 KiB Leaf Page Descriptor ─────────────
        //
        // Program the final Stage-2 descriptor:
        //   Bits [47:12] = PA (4 KiB-aligned output address)
        //   Bits [lower]  = flags (MemAttr, HAP, SH, AF, XN, SW tag)
        //
        // After this write, any guest access to `ipa` will be hardware-
        // translated to `pa` with the permissions encoded in `flags`.
        (*l3_table).entries[l3_idx] = Stage2Pte::page_4kb(pa, flags).raw();
    }
}


// ═══════════════════════════════════════════════════════════════════════
// §7  stage2_mmu_init — Activate Stage-2 Translation via VTTBR_EL2
// ═══════════════════════════════════════════════════════════════════════
//
// Called from hypervisor_main() — Phase 1 of the Zero-Kernel Boot
// Sequence (src/main.rs §2).
//
// Pre-condition:
//   VTTBR_EL2 = 0 (fail-closed placeholder, set by boot.S Step 4).
//   HCR_EL2.VM = 1 (Stage-2 enabled, set by boot.S Step 2).
//   VTCR_EL2 = 0x0002_3558 (4KB TG0, 40-bit IPA, SL0=L1).
//
// Post-condition:
//   VTTBR_EL2 = (VMID << 48) | root_table_pa
//   TLB is clean: TLBI VMALLS12E1IS + DSB ISH + ISB.
//   Stage-2 walks for VMID 1 resolve through the root table.

/// Initialise the Stage-2 MMU: install the root table in VTTBR_EL2
/// and activate hardware IPA → PA translation.
///
/// Boot.S zeroed VTTBR_EL2 as a security placeholder.  This function
/// replaces zero with the real root table physical address and VMID,
/// enabling the Stage-2 hardware translation walker.
///
/// The root table starts with all entries INVALID — no mappings are
/// active until callers invoke `map_4kb_page()` to populate specific
/// IPA → PA translations.
#[inline(never)]
pub fn stage2_mmu_init() {
    // ── Step 1 — Obtain root table physical address ─────────────────
    //
    // At EL2 with SCTLR_EL2.M = 0, there is no EL2 address translation.
    // Virtual address == Physical address (flat physical addressing).
    // The ROOT static lives in .bss; its Rust pointer value IS the PA
    // that VTTBR_EL2.BADDR requires.
    //
    // SAFETY: We only read the address of ROOT, no mutation occurs.
    let root_pa: u64 = unsafe { ROOT.as_mut_ptr() as u64 };

    // ── Step 2 — Compose VTTBR_EL2 value ────────────────────────────
    //
    // VTTBR_EL2 layout (ARMv8-A ARM D13.2.144):
    //
    //   Bits [63:48]  VMID   — Virtual Machine Identifier
    //   Bits [47:1]   BADDR  — Physical base of Stage-2 root table
    //   Bit  [0]      CnP    — Common-not-Private (0 = private TLB)
    //
    // VMID = 1 for the boot VM.  VMID 0 is reserved / host context.
    // CnP = 0: each core maintains private TLB entries for this VMID.
    //          (CnP=1 is a Neoverse optimisation for shared page tables
    //           across cores — enabled later when vCPU scheduling is live.)
    const VMID: u64 = 1;
    let vttbr: u64 = (VMID << 48) | (root_pa & ADDR_MASK);

    // ── Step 3 — Program VTTBR_EL2 and invalidate stale TLB ────────
    //
    // TLB maintenance sequence (ARM ARM D5.10.1):
    //
    //   1. MSR VTTBR_EL2, <val>  — install new root table + VMID.
    //   2. ISB                   — context-synchronise the MSR write
    //                              so subsequent TLBI sees new VMID.
    //   3. TLBI VMALLS12E1IS     — invalidate ALL Stage-1 and Stage-2
    //                              TLB entries for this VMID across all
    //                              cores in the Inner Shareable domain.
    //   4. DSB ISH               — data synchronisation barrier ensures
    //                              the TLBI has completed on all cores.
    //   5. ISB                   — synchronise the instruction stream
    //                              so the next fetch uses the new tables.
    //
    // SAFETY: Writing VTTBR_EL2 is a privileged EL2 operation.
    // We are executing at EL2 (verified by boot.S Step 1).
    // root_pa is valid, 8 KiB-aligned (repr(align(8192))), and
    // zero-initialised (all entries INVALID → fail-closed).
    // The TLBI sequence follows the ARM ARM mandated ordering.
    unsafe {
        asm!(
            "msr   vttbr_el2, {vttbr}",
            "isb",
            "tlbi  vmalls12e1is",
            "dsb   ish",
            "isb",
            vttbr = in(reg) vttbr,
            options(nostack),
        );
    }
}


// ═══════════════════════════════════════════════════════════════════════
// §8  get_vttbr — Read the Current VTTBR_EL2 Value
// ═══════════════════════════════════════════════════════════════════════

/// Read the current `VTTBR_EL2` register value.
///
/// Returns the composite `(VMID << 48) | BADDR` value that was last
/// programmed by `stage2_mmu_init()`.  Used by `hw::viommu` to pass
/// the same table base to the SMMUv3 Context Descriptor (S2TTB field),
/// ensuring unified CPU and DMA address translation.
///
/// # Safety Note
///
/// Reading VTTBR_EL2 is a privileged EL2 operation but has no side
/// effects — it is safe to call at any point after `stage2_mmu_init()`.
#[inline(always)]
pub fn get_vttbr() -> u64 {
    let val: u64;
    // SAFETY: MRS from VTTBR_EL2 is a read-only privileged operation
    // at EL2.  No architectural side effects.  We are at EL2 (verified
    // by boot.S Step 1).
    unsafe {
        asm!(
            "mrs {val}, vttbr_el2",
            val = out(reg) val,
            options(nomem, nostack),
        );
    }
    val
}
