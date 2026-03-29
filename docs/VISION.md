# Vision: The Device-Edge-Cloud Continuum

## Scale-Invariant Virtualisation

The Monadic Hypervisor is built around a single unifying principle: **a single, coherent hypervisor architecture that operates without modification across every tier of the compute hierarchy** — from warehouse-scale AI silicon to millimetre-scale RISC-V microcontrollers soldered onto sensor PCBs.

We call this the **Device-Edge-Cloud Continuum**.

---

## The Three Tiers

### ☁️ Cloud: Hyperscale AI Foundries

At the top of the continuum sit the world's largest AI training and inference clusters — racks of high-memory GPU nodes, NVLink fabrics, and 400 GbE interconnects processing petabytes of model activations per day.

In this environment, the hypervisor's primary mandate is **throughput with isolation**: thousands of GPU partitions running simultaneously, each belonging to a different tenant, must achieve near-native PCIe bandwidth while remaining cryptographically isolated from one another.

#### True PCIe Bypass via `vfio-pci`

The cloud data plane achieves this through **True PCIe Bypass** using the `vfio-pci` device assignment model:

- The hypervisor programs the **IOMMU** (AMD-Vi / Intel VT-d / ARM SMMU v3) with a per-VM DMA translation table, mirroring the Stage-2 memory map. The IOMMU enforces that a PCIe device assigned to VM A cannot DMA into the physical frames belonging to VM B.
- The guest VM's device driver communicates directly with the PCIe endpoint using **MMIO BAR mappings** that resolve through Stage-2 translation — no hypervisor is in the critical path for data-plane I/O.
- **Interrupt virtualisation** is handled by GICv4 LPIs (ARM) or MSI injection (x86), delivering interrupts from the PCIe device directly to the guest vCPU without a hypervisor exit.

The result: DMA bandwidth from NVMe SSDs, InfiniBand HCAs, or GPU memory channels reaches the guest at **line rate**, with the hypervisor overhead limited to the control plane (VM creation, device assignment, live migration) rather than the data plane.

---

### 🏭 Edge: Enterprise and Industrial Edge Nodes

The middle tier encompasses ARM64 server blades in telco central offices, industrial PLCs, autonomous vehicle compute platforms, and smart infrastructure nodes. Workloads are mixed-criticality: a safety-critical motor-control loop must meet hard real-time deadlines while sharing the same silicon with a Linux-based management plane.

Here the hypervisor enforces **temporal isolation** (guaranteed CPU time slices for real-time partitions) alongside spatial memory isolation. The ARM64 Hypervisor Generic Timer virtualisation (`CNTHP_*` / `CNTHV_*` registers) provides per-VM virtualised time, and the GICv3 List Registers allow the hypervisor to inject timer interrupts directly into real-time guest partitions without perturbing neighbouring VMs.

---

### 📡 Embedded Edge: RISC-V Silicon

At the bottom of the continuum sit deeply embedded devices: environmental sensors, wearable health monitors, smart grid endpoints, and autonomous drone flight controllers. These devices run on RISC-V cores with tens to hundreds of kilobytes of SRAM — far below the minimum footprint for a Linux kernel.

#### Hardware-Assisted RTOS Services in RISC-V Silicon

The Monadic Hypervisor addresses this tier through a fundamentally different deployment model: **RTOS services compiled directly into RISC-V silicon**.

Rather than running a separate RTOS binary, the hypervisor's scheduler, interrupt controller abstraction, and memory protection primitives are compiled as a **static library linked directly into the application firmware image**. The RISC-V Physical Memory Protection (PMP) unit is configured at HS-mode to enforce isolation between the "hypervisor library" regions and the application code, providing hardware-enforced safety boundaries without the overhead of a full OS context switch.

Key properties of the embedded tier deployment:

- **Sub-microsecond interrupt latency** — no OS scheduling jitter; interrupt vectors are direct hardware entry points.
- **Deterministic stack usage** — all stack frames are bounded at compile time; no dynamic allocation in interrupt paths.
- **Compile-time RTOS configuration** — task priorities, stack sizes, and memory regions are encoded as Rust `const` generics, verified by the compiler rather than a runtime configurator.
- **Zero-copy sensor pipelines** — DMA descriptors are built at compile time from linker-section addresses, eliminating runtime descriptor construction overhead.

---

## Architectural Properties Enabling Continuum Scaling

| Property | Cloud Impact | Edge Impact | Embedded Impact |
|---|---|---|---|
| `#![no_std]` Rust | Minimal binary, no libc bloat | Predictable memory footprint | Fits in 64 KB SRAM |
| Stage-2 / PMP isolation | Multi-tenant GPU partitioning | Mixed-criticality isolation | Application sandboxing |
| `vfio-pci` PCIe bypass | Line-rate DMA to guests | NVMe/NIC bypass on edge blades | N/A |
| RISC-V H-extension / ARM EL2 | ARM64 cloud servers | ARM64 edge nodes | RISC-V embedded SoCs |
| Lock-free `core::sync::atomic` | High-core-count SMP vCPU scheduling | Real-time IPI handling | ISR-to-task notification |
| Static memory allocation | Predictable VM creation latency | Hard real-time guarantees | No heap fragmentation |
| UEFI boot payload | Standard server firmware | EDK II on Arm platforms | Custom HS-mode entry stub |

---

## The Monad Abstraction

The name "Monadic" reflects the design goal of a **single compositional abstraction** — the VM — that composes identically at every tier. A VM at cloud scale is a large collection of vCPUs, Stage-2 page tables, and PCIe-assigned devices. A VM at embedded scale is a pair of PMP regions and a hardware timer slot. The hypervisor core logic for creating, scheduling, and destroying VMs is identical across tiers; only the HAL (Hardware Abstraction Layer) differs between `arch/arm64/` and `arch/riscv/`.

This monadic composition means that a security fix, a scheduling improvement, or a new isolation primitive developed for the cloud tier propagates automatically to the edge and embedded tiers — there is no fork, no port, no divergent maintenance burden.

---

## Roadmap

| Milestone | Tier | Feature |
|---|---|---|
| M1 | Cloud / Edge | ARM64 EL2 boot, Stage-2 paging, UEFI payload |
| M2 | Cloud | GICv3 virtualisation, vCPU scheduling |
| M3 | Cloud | `vfio-pci` PCIe bypass, IOMMU Stage-2 |
| M4 | Edge | RISC-V H-extension boot, PMP isolation |
| M5 | Embedded | Static RTOS library mode for RV32/RV64 |
| M6 | All | Live VM migration, snapshot/restore |
| M7 | All | Formal verification of Stage-2 isolation invariants (Kani / Creusot) |
