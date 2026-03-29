# Monadic Hypervisor

A bare-metal, zero-kernel Type-1 hypervisor spanning the full **Device-Edge-Cloud Continuum** — from hyperscale AI foundries running thousands of GPU partitions down to deeply embedded RISC-V microcontrollers at the IoT edge.

## Overview

We architected the Monadic Hypervisor in **Rust** to achieve uncompromising bare-metal performance combined with **guaranteed memory safety**. By targeting `#![no_std]` throughout the entire codebase, the hypervisor carries zero runtime dependency on an operating system, libc, or any POSIX abstraction layer. Rust's strict ownership model and borrow checker enforce spatial and temporal memory safety at compile time, eliminating whole classes of vulnerabilities (buffer overflows, use-after-free, data races) that plague traditional C/C++ hypervisors — with zero-cost abstractions that impose no overhead versus hand-written assembly.

## Key Properties

| Property | Detail |
|---|---|
| **Architecture** | Type-1 bare-metal hypervisor (no host OS) |
| **Language** | Rust `#![no_std]` — `core` only, no `std`, no libc |
| **Target ISAs** | ARM64 (AArch64) and RISC-V (RV64GC) |
| **Privilege Level** | ARM64 Exception Level 2 (EL2); RISC-V Hypervisor extension (HS-mode) |
| **Boot Protocol** | UEFI payload; direct firmware hand-off |
| **Memory Safety** | Compile-time guarantees via Rust ownership and borrow checker |
| **Footprint** | Zero-kernel — no kernel tax, no scheduler overhead |

## Scale Invariance

The Monadic Hypervisor is designed from first principles to be **scale-invariant**:

- **Hyperscale AI Foundries** — orchestrates thousands of vCPU and GPU partitions with hardware-assisted PCIe bypass (`vfio-pci`) to deliver line-rate I/O directly into guest VMs with zero kernel copies.
- **Enterprise Edge** — lightweight VM density optimised for ARM64 server blades, running mixed-criticality workloads under strict isolation.
- **Embedded Edge** — hardware-assisted RTOS services compiled directly into RISC-V silicon, enabling deterministic real-time guarantees with microsecond interrupt latency.

## Repository Layout

```
monadic-hypervisor/
├── arch/
│   ├── arm64/boot/      # AArch64 EL2 entry stubs and UEFI hand-off
│   └── riscv/boot/      # RISC-V HS-mode entry stubs
├── src/                 # Core hypervisor Rust sources (#![no_std])
├── docs/
│   ├── ADR-001-Zero-Kernel-Strict-No-Std.md
│   └── VISION.md
├── ARCHITECTURE.md
├── LICENSE
└── README.md
```

## License

This project is released under the [BSD-2-Clause Plus Patent License](LICENSE).
