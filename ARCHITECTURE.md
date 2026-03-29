# Architecture: Monadic Hypervisor

## 1. Bare-Metal Hardware Execution Environment

The Monadic Hypervisor executes **directly on physical hardware** with no intervening operating system layer. There is no host OS, no hypervisor daemon, and no kernel module — the hypervisor binary is loaded by firmware and becomes the first privileged software running on the machine.

### 1.1 Boot Sequence

The hypervisor boots as a **UEFI payload**. Firmware (EDK II / UEFI) locates the hypervisor EFI application, loads it into memory, and hands off execution at the UEFI application entry point. The hypervisor then:

1. Consumes required UEFI boot services (memory map, ACPI/SMBIOS tables, device tree).
2. Calls `ExitBootServices()` to relinquish UEFI and take sole ownership of the hardware.
3. Transitions into the bare-metal hypervisor execution loop; no further UEFI calls are made.

This boot model provides a clean, vendor-neutral firmware interface while preserving full bare-metal control post-handoff.

### 1.2 ARM64 Exception Level 2 (EL2)

On AArch64 targets the hypervisor executes **strictly at Exception Level 2 (EL2)** — the architectural privilege level reserved for hypervisors. Execution never drops below EL2 for privileged hypervisor code paths.

| Exception Level | Role | Occupant |
|---|---|---|
| EL3 | Secure Monitor (TrustZone) | Firmware / TF-A |
| **EL2** | **Hypervisor** | **Monadic Hypervisor** |
| EL1 | Guest OS kernel | Guest operating system |
| EL0 | Guest user space | Guest applications |

Relevant EL2 system registers used:

- `HCR_EL2` — Hypervisor Configuration Register; enables Stage-2 translation, traps, and virtualisation features.
- `VTTBR_EL2` — Virtualization Translation Table Base Register; holds the physical address of the Stage-2 translation table for the currently scheduled VM context (see §2).
- `VTCR_EL2` — Virtualisation Translation Control Register; configures the Stage-2 translation granule, address size, and TLB management.
- `VMPIDR_EL2` / `VPIDR_EL2` — virtualised CPU identity registers presented to guests.
- `ICC_*_EL2` / `ICH_*_EL2` — GIC virtualisation registers for interrupt virtualisation.

### 1.3 RISC-V Hypervisor Extension (HS-mode)

On RISC-V targets the hypervisor executes in **HS-mode** (Hypervisor-extended Supervisor mode) using the RISC-V Hypervisor extension (ratified in the privileged specification). Guest operating systems run in VS-mode (Virtual Supervisor) and guest user space in VU-mode.

---

## 2. Stage-2 Memory Translation (ARM64)

Stage-2 memory translation is the cornerstone of guest memory isolation on ARM64. The Monadic Hypervisor **directly manages the `VTTBR_EL2` register** to control the Stage-2 page tables for each virtual machine.

### 2.1 Two-Stage Translation Overview

```
Guest Virtual Address (VA)
        │
        ▼
  ┌─────────────┐
  │  Stage-1    │  (controlled by Guest OS at EL1 — walks guest page tables)
  │  Translation│
  └─────────────┘
        │
        ▼  Intermediate Physical Address (IPA)
  ┌─────────────┐
  │  Stage-2    │  (controlled by Hypervisor at EL2 — walks hypervisor-owned tables)
  │  Translation│  VTTBR_EL2 points to the root of these tables
  └─────────────┘
        │
        ▼  Host Physical Address (PA)
   Physical RAM / MMIO
```

Stage-1 is entirely under guest OS control. Stage-2 is exclusively under hypervisor control; the guest cannot influence it. This enforces hard isolation: a guest cannot access physical memory outside the IPA ranges explicitly mapped by the hypervisor.

### 2.2 VTTBR_EL2 Management

`VTTBR_EL2` encodes:

| Field | Width | Description |
|---|---|---|
| `VMID` | 16 bits (with `VTCR_EL2.VS=1`) | Virtual Machine Identifier — tags TLB entries to avoid full TLB flushes on context switch |
| `BADDR` | 48 bits | Physical base address of the Stage-2 Level-0 (or Level-1) translation table |

On every VM context switch the hypervisor writes a new `VTTBR_EL2` value (new VMID + new table base). A targeted `TLBI VMALLE1IS` or `TLBI ALLE2IS` instruction is issued only when the VMID wraps around or a mapping is explicitly invalidated, preserving TLB residency across context switches for performance.

### 2.3 No Dynamic Allocation in Translation Hot Path

All Stage-2 page table memory is pre-allocated from a physically contiguous pool at VM creation time. The translation hot path (trap-and-emulate, memory fault handling) performs zero heap allocation, ensuring bounded worst-case latency.

---

## 3. Memory Safety Model

The entire hypervisor is written in **Rust `#![no_std]`** with no `unsafe` block permitted outside explicitly audited HAL (Hardware Abstraction Layer) modules. The Rust type system enforces:

- **Ownership** — each physical page frame is owned by exactly one `PageFrame` value; transferring ownership to a guest revokes hypervisor access.
- **Lifetimes** — mapped MMIO regions carry lifetimes tied to the VM they belong to; dangling MMIO pointers are a compile-time error.
- **Send/Sync** — cross-vCPU shared state is mediated through `core::sync::atomic` primitives, preventing data races without a runtime lock manager.

See [ADR-001](docs/ADR-001-Zero-Kernel-Strict-No-Std.md) for the canonical `#![no_std]` mandate.

---

## 4. Component Map

```
src/
├── main.rs              # UEFI entry point; ExitBootServices(); jump to hypervisor_main()
├── hypervisor.rs        # Top-level VM lifecycle (create, run, destroy)
├── mm/
│   ├── stage2.rs        # Stage-2 page table walker and VTTBR_EL2 management
│   └── frame_alloc.rs   # Lock-free physical frame allocator
├── vcpu/
│   ├── arm64.rs         # AArch64 vCPU state save/restore, EL2 trap handling
│   └── riscv.rs         # RISC-V vCPU state, HS-mode trap handling
├── virt/
│   ├── gic.rs           # ARM GICv3/v4 interrupt virtualisation
│   └── pcie.rs          # vfio-pci PCIe bypass device assignment
└── hal/
    ├── arm64/           # Unsafe AArch64 register access (audited)
    └── riscv/           # Unsafe RISC-V CSR access (audited)

arch/
├── arm64/boot/          # AArch64 EL2 reset vector, UEFI CRT0
└── riscv/boot/          # RISC-V HS-mode reset vector, UEFI CRT0
```
