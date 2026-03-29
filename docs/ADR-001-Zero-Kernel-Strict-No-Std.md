# ADR-001: Zero-Kernel Strict `#![no_std]`

**Status:** Accepted  
**Date:** 2026-03-29  
**Deciders:** Monadic Hypervisor Core Team  

---

## Context

The Monadic Hypervisor executes at bare metal — there is no host operating system, no kernel, and no standard C runtime beneath it. Any code that implicitly assumes the presence of a POSIX environment (file descriptors, virtual memory managed by a host kernel, dynamic linking, thread-local storage backed by the OS, etc.) will either fail to link, produce undefined behaviour at runtime, or introduce undeclared dependencies on a Linux host that we explicitly do not have.

Rust's standard library (`std`) is a thin, safe wrapper around the platform's C runtime and POSIX/Win32 syscall interface. Linking `std` into the hypervisor would pull in `libc`, `libpthread`, dynamic TLS, `__stack_chk_guard`, and a large body of initialisation code — none of which has a valid execution environment at EL2 on bare metal.

This ADR establishes the canonical, non-negotiable `#![no_std]` mandate for every source file in this repository.

---

## Decision

### 1. Mandatory `#![no_std]` Crate Attribute

Every Rust source file that forms part of the Monadic Hypervisor binary **must** carry the crate-level attribute:

```rust
#![no_std]
```

This attribute must appear as the **first non-comment item** in every `lib.rs`, `main.rs`, and any crate root. There are no exceptions.

### 2. Prohibited Imports and Dependencies

The following are **strictly prohibited** in any source file under this repository:

| Prohibited Item | Reason |
|---|---|
| `use std::*` | Requires a POSIX/OS runtime; unavailable at EL2 bare metal |
| `use std::io::*` | Assumes file descriptors; no FD table exists at EL2 |
| `use std::fs::*` | Assumes a VFS; no filesystem layer is present |
| `use std::net::*` | Assumes a host TCP/IP stack; no socket layer exists |
| `use std::thread::*` | Assumes a host scheduler and TLS; vCPU threads are managed directly |
| `use std::sync::Mutex` / `RwLock` | Uses `pthread_mutex`; no pthreads available |
| `extern crate std` | Equivalent to `use std::*` at the crate level |
| POSIX syscalls (`libc::read`, `libc::write`, `syscall!(...)`) | No Linux kernel is present to handle syscalls |
| `println!` / `eprintln!` / `dbg!` macros | Depend on `std::io`; must not be used |
| `libc` crate (any version) | Wraps C runtime and POSIX syscalls |
| `std::alloc` global allocator (unless replaced with a `#[global_allocator]` backed by our own frame allocator) | Default allocator calls `malloc`/`free` via libc |

### 3. Permitted `core` Primitives

All code **must** use only the `core` crate (Rust's libcore — the subset of `std` that has no OS dependencies). The following `core` modules are the standard building blocks for bare-metal hypervisor code:

| Module | Purpose |
|---|---|
| `core::sync::atomic` | Lock-free inter-vCPU synchronisation (`AtomicU64`, `AtomicBool`, `fence`) |
| `core::ptr` | Raw pointer reads/writes for MMIO and system register access (`read_volatile`, `write_volatile`) |
| `core::arch::asm!` | Inline assembly for system register access (`mrs`, `msr`, `isb`, `dsb`, `tlbi`, `csrr`, `csrw`) |
| `core::mem` | `size_of`, `align_of`, `MaybeUninit` for struct layout and uninitialised memory |
| `core::slice` | Safe slice construction over physically-mapped memory regions |
| `core::fmt` | `Write` trait for a custom UART-backed debug formatter (no allocation required) |
| `core::option` / `core::result` | Standard error propagation without exceptions |
| `core::panic::PanicInfo` | Custom `#[panic_handler]` — halts all vCPUs and logs via UART |

### 4. `unsafe` Encapsulation Policy

Because bare-metal code inevitably requires unsafe operations (register access, physical memory mapping), all `unsafe` code **must** be:

1. Confined to HAL modules under `src/hal/arm64/` and `src/hal/riscv/`.
2. Accompanied by a `// SAFETY:` comment explaining precisely why the invariants required by the `unsafe` block are upheld at the call site.
3. Reviewed by a second engineer before merge (enforced via GitHub CODEOWNERS).

No `unsafe` block is permitted outside the HAL modules without an explicit ADR amendment.

### 5. Custom Panic Handler

Because `#![no_std]` eliminates the default panic handler, every binary crate must provide a `#[panic_handler]`:

```rust
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Log via UART using core::fmt::Write (no allocation)
    // Halt all CPUs via platform-specific spin-wait or WFI
    loop {}
}
```

The panic handler must never allocate, call `std`, or invoke any POSIX function. It must be `-> !` (diverging).

### 6. No Dynamic Linking

The hypervisor binary is a **statically linked, position-independent ELF** or **UEFI PE/COFF** image with no dynamic section. All dependencies are compiled from source and linked at build time. The `lld` linker is used with an explicit linker script for each target architecture.

---

## Consequences

### Positive

- **Zero kernel tax** — no syscall overhead, no context switches into a host kernel.
- **Deterministic latency** — no hidden allocations or OS scheduler interference.
- **Minimal attack surface** — no POSIX API surface exposed; the hypervisor cannot be exploited via syscall fuzzing.
- **Portability** — the same codebase compiles for ARM64 and RISC-V without any OS-specific conditional compilation.
- **Compile-time safety** — Rust's type system catches entire classes of concurrency and memory bugs before the code ever runs on hardware.

### Negative / Mitigations

- **No `format!` / `String`** — mitigated by `core::fmt::Write` with a stack-allocated `arrayvec`-style buffer for debug output.
- **No standard collections** — mitigated by custom `no_std` data structures (intrusive linked lists, fixed-capacity ring buffers) implemented over `core::ptr` and `core::mem`.
- **Steeper learning curve** — mitigated by this ADR and inline `// SAFETY:` documentation.

---

## Compliance

Any pull request that introduces a `use std::` import, a POSIX syscall, or removes the `#![no_std]` attribute from any crate root will be **automatically rejected** by the CI pipeline (`cargo check --target aarch64-unknown-none` and `cargo check --target riscv64gc-unknown-none-elf`). These targets have no `std` support and will fail to compile if any `std` dependency is introduced.
