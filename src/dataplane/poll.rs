/*
 * src/dataplane/poll.rs — Lock-Free SPSC Ring Buffer & NVMe Polling Engine
 *
 * This module implements the terminal state of the Monadic Hypervisor:
 * an infinite zero-copy polling loop that drains NVMe Completion Queues
 * via a cache-line-isolated Single-Producer Single-Consumer (SPSC) ring
 * buffer.
 *
 * # Architectural Pillars
 *
 *   0-Kernel  — No syscalls, no interrupts, no context switches.
 *               The poll loop runs at EL2 on bare metal.
 *   0-Copy    — Completions are indices into DMA-mapped buffers.
 *               No memcpy in the hot path.
 *
 * # Memory Ordering (ARMv8.1 LSE)
 *
 *   The SPSC contract requires only Acquire/Release pairs — not SeqCst.
 *   On Neoverse N2/V2 cores with LSE, `Acquire` loads compile to
 *   `LDAR` and `Release` stores compile to `STLR`.  These are
 *   single-instruction barriers — no standalone `DMB` required.
 *
 *   Why not SeqCst:
 *     • SeqCst emits `DMB ISH` + `STLR` on AArch64 — a full barrier
 *       that serialises the entire store buffer (~10–15 ns penalty on
 *       Neoverse V2).
 *     • SPSC needs only per-variable ordering (producer publishes head
 *       after writing the slot; consumer publishes tail after reading
 *       the slot).  Acquire/Release is sufficient and optimal.
 *
 * # Cache-Line Isolation
 *
 *   Neoverse N1/N2/V1/V2 L1D cache line = 64 bytes.
 *   The `head` (producer) and `tail` (consumer) atomics are placed in
 *   separate `#[repr(C, align(64))]` wrappers so they occupy distinct
 *   cache lines.  This eliminates false-sharing coherence traffic
 *   (MOESI invalidations) between the producer and consumer cores.
 *
 * # Energy-Efficient Yielding
 *
 *   When the queue is empty (head == tail), the consumer emits `WFE`
 *   (Wait For Event).  On Neoverse V2 this clock-gates the execution
 *   units, dropping core power to near-idle.  The core wakes on:
 *     • SEV from the producer core after a push()
 *     • Physical IRQ/FIQ (e.g., Device-nGnRE NVMe MSI-X)
 *     • Debug halt
 *
 *   The producer calls `SEV` after every push() to wake the consumer.
 *   This SEV→WFE handshake replaces legacy SPDK's 100% CPU busy-poll.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026  SiliconLanguage — Monadic Hypervisor Project
 */

use core::arch::asm;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};

// ═══════════════════════════════════════════════════════════════════════
// §1  Cache-Line Aligned Atomic Index
// ═══════════════════════════════════════════════════════════════════════
//
// Each index (head / tail) gets its own 64-byte cache line.
//
// Layout (64 bytes):
//   [0..7]   AtomicUsize (8 bytes on AArch64)
//   [8..63]  Padding (56 bytes) — never touched, prevents false sharing
//
// On Neoverse V2, the L1D operates on 64-byte lines.  If head and tail
// shared a line, every push() would invalidate the consumer's copy and
// vice-versa — a coherence ping-pong costing ~40–80 ns per round-trip
// on a cross-cluster MOESI snoop.

/// A `usize` atomic padded to a full 64-byte Neoverse cache line.
///
/// This prevents the producer's `head` and consumer's `tail` from
/// sharing an L1D cache line, eliminating false-sharing coherence
/// traffic on multi-core Neoverse topologies.
#[repr(C, align(64))]
struct CacheLineAtomicUsize {
    value: AtomicUsize,
    // Compiler inserts 56 bytes of padding to reach align(64).
}

impl CacheLineAtomicUsize {
    const fn new(v: usize) -> Self {
        Self {
            value: AtomicUsize::new(v),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// §2  SPSC Ring Buffer
// ═══════════════════════════════════════════════════════════════════════
//
// A bounded, lock-free, single-producer single-consumer queue.
//
// Invariants:
//   • N must be a power of two (enforced at init time via assert).
//     This allows index wrapping via bitwise AND instead of modulo.
//   • Only one thread calls push() (the producer / NVMe CQ handler).
//   • Only one thread calls pop() (the consumer / dataplane_poll_loop).
//   • No dynamic allocation — the buffer is `[MaybeUninit<T>; N]`,
//     embedded inline.
//
// Memory ordering contract:
//   • push():  Write slot → store head with Release.
//              The Release fence ensures the slot write is visible
//              to the consumer before the updated head is.
//   • pop():   Load head with Acquire → read slot → store tail with
//              Release.  Acquire ensures we see the producer's slot
//              write; Release ensures the producer sees our tail
//              advance before reusing the slot.

/// A fixed-capacity, lock-free, single-producer single-consumer queue.
///
/// `T` must be `Copy` — we store completions by value (typically a
/// 16-byte NVMe CQE index or a 64-bit descriptor pointer).
///
/// `N` must be a power of two.  This is verified at construction time.
///
/// # Cache-Line Layout
///
/// ```text
/// Offset  Field       Size    Cache Line
/// ──────  ─────       ────    ──────────
/// 0x000   head        64B     Line 0  (producer-owned)
/// 0x040   tail        64B     Line 1  (consumer-owned)
/// 0x080   buffer[N]   N×T     Lines 2..  (shared, read-only per role)
/// ```
///
/// The `head` and `tail` are in separate cache lines to prevent
/// false-sharing MOESI invalidations between cores.
#[repr(C)]
pub struct SpscQueue<T: Copy, const N: usize> {
    /// Producer write index.  Only the producer calls `push()`.
    /// Stored in its own 64-byte cache line (Line 0).
    head: CacheLineAtomicUsize,

    /// Consumer read index.  Only the consumer calls `pop()`.
    /// Stored in its own 64-byte cache line (Line 1).
    tail: CacheLineAtomicUsize,

    /// Fixed-capacity ring buffer.  Slots are written by the producer
    /// and read by the consumer.  `MaybeUninit` avoids requiring
    /// `Default` and prevents the compiler from reading uninitialised
    /// memory.
    buffer: UnsafeCell<[MaybeUninit<T>; N]>,
}

// SAFETY: SpscQueue is safe to share between exactly two threads
// (one producer, one consumer) because:
//   1. head is only written by the producer.
//   2. tail is only written by the consumer.
//   3. Each slot is written by the producer (before head advance)
//      and read by the consumer (before tail advance), with
//      Acquire/Release ordering ensuring visibility.
//   4. No slot is accessed concurrently — the head/tail protocol
//      ensures exclusive access windows.
unsafe impl<T: Copy, const N: usize> Sync for SpscQueue<T, N> {}

impl<T: Copy, const N: usize> SpscQueue<T, N> {
    /// Create a new empty SPSC queue.
    ///
    /// # Panics
    ///
    /// Panics if `N` is not a power of two (required for branchless
    /// index wrapping via `& (N - 1)`).
    pub const fn new() -> Self {
        // const assert: N must be a power of two and non-zero.
        assert!(N > 0 && (N & (N - 1)) == 0);

        Self {
            head: CacheLineAtomicUsize::new(0),
            tail: CacheLineAtomicUsize::new(0),
            // SAFETY: MaybeUninit array does not require initialisation.
            buffer: UnsafeCell::new(unsafe {
                MaybeUninit::<[MaybeUninit<T>; N]>::uninit().assume_init()
            }),
        }
    }

    /// Attempt to enqueue a value (producer side).
    ///
    /// Returns `true` if the value was enqueued, `false` if the queue
    /// is full.
    ///
    /// # Memory Ordering
    ///
    /// 1. Load `tail` with `Acquire` to see the consumer's latest
    ///    slot release.
    /// 2. Write the slot (no atomic needed — exclusive producer access).
    /// 3. Store `head` with `Release` to publish the new slot to the
    ///    consumer.
    /// 4. `SEV` — wake any consumer core parked in WFE.
    ///
    /// On Neoverse V2 (LSE):
    ///   • Acquire load → `LDAR`  (single instruction, no barrier)
    ///   • Release store → `STLR` (single instruction, no barrier)
    pub fn push(&self, value: T) -> bool {
        let head = self.head.value.load(Ordering::Relaxed);
        let tail = self.tail.value.load(Ordering::Acquire);

        // Full check: if advancing head would collide with tail,
        // the queue is full.  One slot is always wasted to
        // disambiguate full vs. empty.
        if head.wrapping_sub(tail) >= N {
            return false;
        }

        let slot = head & (N - 1);

        // SAFETY:
        //   • `slot` is in [0, N) — bounded by power-of-two mask.
        //   • Only the producer writes to this slot.
        //   • The consumer cannot read it until head is published.
        unsafe {
            let buf = &mut *self.buffer.get();
            buf[slot] = MaybeUninit::new(value);
        }

        // Publish the new head.  Release ordering ensures the slot
        // write above is visible to the consumer before this store.
        self.head.value.store(head.wrapping_add(1), Ordering::Release);

        // Wake any consumer core parked in WFE.
        //
        // SEV (Send Event) sets the Event Register on all PEs in the
        // shareability domain.  On Neoverse, this is a single-cycle
        // hint instruction — negligible overhead even if the consumer
        // is already awake.
        //
        // SAFETY: SEV is a hint with no architectural side effects
        // beyond setting the Event Register.
        unsafe {
            asm!("sev", options(nomem, nostack));
        }

        true
    }

    /// Attempt to dequeue a value (consumer side).
    ///
    /// Returns `Some(T)` if a value was available, `None` if the queue
    /// is empty.
    ///
    /// # Memory Ordering
    ///
    /// 1. Load `head` with `Acquire` to see the producer's latest
    ///    slot publication.
    /// 2. Read the slot (no atomic needed — exclusive consumer access).
    /// 3. Store `tail` with `Release` to release the slot back to the
    ///    producer.
    ///
    /// On Neoverse V2 (LSE):
    ///   • Acquire load → `LDAR`
    ///   • Release store → `STLR`
    pub fn pop(&self) -> Option<T> {
        let tail = self.tail.value.load(Ordering::Relaxed);
        let head = self.head.value.load(Ordering::Acquire);

        // Empty check: if head == tail, no items available.
        if head == tail {
            return None;
        }

        let slot = tail & (N - 1);

        // SAFETY:
        //   • `slot` is in [0, N) — bounded by power-of-two mask.
        //   • The producer has written this slot (head > tail).
        //   • Acquire on head ensures the slot write is visible.
        let value = unsafe {
            let buf = &*self.buffer.get();
            buf[slot].assume_init()
        };

        // Release the slot.  The producer can now reuse it after
        // observing our updated tail.
        self.tail.value.store(tail.wrapping_add(1), Ordering::Release);

        Some(value)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// §3  NVMe Completion Entry (Stub)
// ═══════════════════════════════════════════════════════════════════════
//
// A minimal representation of an NVMe CQE for the polling engine.
// The full 16-byte NVMe CQE will be defined when we implement the
// NVMe driver; for now we use a 64-bit completion token that carries
// the submission queue ID and command ID — sufficient for the SPSC
// pipeline proof-of-concept.

/// NVMe completion token — a lightweight handle into the DMA-mapped
/// completion queue.
///
/// Layout (8 bytes, fits in a single register on AArch64):
///   [63:32]  Reserved (zero)
///   [31:16]  Submission Queue ID (SQID)
///   [15:0]   Command ID (CID)
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct NvmeCompletionToken(u64);

impl NvmeCompletionToken {
    /// Extract the 16-bit Command ID.
    #[inline(always)]
    pub const fn cid(self) -> u16 {
        self.0 as u16
    }

    /// Extract the 16-bit Submission Queue ID.
    #[inline(always)]
    pub const fn sqid(self) -> u16 {
        (self.0 >> 16) as u16
    }
}

// ═══════════════════════════════════════════════════════════════════════
// §4  Static SPSC Queue Instance
// ═══════════════════════════════════════════════════════════════════════
//
// The queue lives in .bss (zero-initialised, zero image cost).
// 256 slots = 8 cache lines of NvmeCompletionToken (256 × 8B = 2 KiB)
// plus 2 cache lines for head + tail = 2,176 bytes total.
//
// 256 is a natural NVMe CQ depth (matches common NVMe controller
// MAX_CQ_ENTRIES and is a power of two for branchless masking).

/// NVMe CQ polling depth — must be a power of two.
const CQ_DEPTH: usize = 256;

/// Static SPSC queue for the NVMe completion polling engine.
///
/// Producer: NVMe CQ interrupt handler / DMA completion path.
/// Consumer: `dataplane_poll_loop()` below.
static CQ_RING: SpscQueue<NvmeCompletionToken, CQ_DEPTH> = SpscQueue::new();

// ═══════════════════════════════════════════════════════════════════════
// §5  Data-Plane Polling Loop (Terminal State)
// ═══════════════════════════════════════════════════════════════════════
//
// This is the final function in the hypervisor boot sequence.
// It never returns — the core lives here forever, polling lock-free
// SPSC rings and yielding via WFE when idle.
//
// Boot path: _start → hypervisor_main → dataplane_poll_loop (here)

/// Enter the infinite zero-copy NVMe polling loop.
///
/// This function **never returns** (`-> !`).  It is the terminal state
/// of the hypervisor on the primary core.
///
/// # Algorithm
///
/// ```text
/// loop {
///     match cq_ring.pop() {
///         Some(token) → process_completion(token)
///         None        → WFE (energy-efficient park)
///     }
/// }
/// ```
///
/// # Pillar Compliance
///
///   **0-Kernel** — No syscalls.  Runs at EL2 on bare metal.
///   **0-Copy**   — Completions are 8-byte tokens (register-width).
///                  No buffer copies in the hot path.
///
/// # Wake Mechanism
///
///   The producer (NVMe CQ handler / future MSI-X path) calls `SEV`
///   after every `push()`.  This sets the Event Register on all PEs,
///   waking our `WFE`.  Worst-case wake latency on Neoverse V2:
///   ~10–20 ns (event propagation through the coherence fabric).
#[inline(never)]
pub fn dataplane_poll_loop() -> ! {
    loop {
        match CQ_RING.pop() {
            Some(token) => {
                // ── Process NVMe Completion ──────────────────────
                //
                // In the full implementation this path will:
                //   1. Look up the SQ submission context by token.cid()
                //   2. Release the DMA buffer back to the pool
                //   3. Advance the CQ doorbell (MMIO write to
                //      Device-nGnRE mapped NVMe BAR0 + 0x1000)
                //   4. Signal the guest vCPU via virtual MSI-X
                //
                // For now: consume the token to prove the pipeline
                // works end-to-end.  The `read_volatile` prevents
                // the compiler from optimising away the read.
                //
                // SAFETY: Reading a u16 from a Copy type has no
                // side effects.  `read_volatile` is used purely to
                // prevent dead-code elimination of the pop() path.
                unsafe {
                    core::ptr::read_volatile(&token.cid());
                }
            }
            None => {
                // ── Energy-Efficient Yield ───────────────────────
                //
                // Queue is empty — no completions to process.
                //
                // WFE (Wait For Event) parks the Neoverse V2 core:
                //   • Clock-gates execution units (near-idle power)
                //   • Core wakes on: SEV, IRQ, FIQ, debug halt
                //   • Latency to resume: ~10–20 ns on V2
                //
                // This replaces SPDK's 100% CPU busy-poll with a
                // hardware-assisted idle state that consumes <1%
                // of the core's TDP when the queue is drained.
                //
                // SAFETY: WFE is a hint instruction with no
                // architectural side effects.
                unsafe {
                    asm!("wfe", options(nomem, nostack));
                }
            }
        }
    }
}
