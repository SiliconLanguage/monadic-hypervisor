# Monadic Hypervisor

A bare-metal, **zero-kernel** Type-1 hypervisor written in Rust `#![no_std]`.
Executes at ARM64 Exception Level 2 (EL2) with no host OS, no libc, and no
POSIX — eliminating the Kernel Tax across the Device-Edge-Cloud Continuum.

## Architectural Pillars

| Pillar | Guarantee |
|--------|-----------|
| **0-Kernel** | Runs at EL2 bare metal. No syscalls, no scheduler, no context switches. |
| **0-Copy** | Lock-free SPSC ring buffers with `Acquire`/`Release` atomics. No `memcpy` in the data-plane hot path. |
| **True PCIe Bypass** | Stage-2 Device-nGnRE MMIO mapping gives guest VMs direct NVMe BAR0 access via `vfio-pci` / vIOMMU. |
| **Hardware-Enlightened** | WFE/SEV energy-efficient polling. Cache-line isolated atomics (`align(64)`). LSE `LDAR`/`STLR` — zero standalone DMB barriers. |

## Hardware Targets

| Platform | Core | Instance |
|----------|------|----------|
| AWS Graviton4 | Neoverse V2 | c8g, r8g, m8g |
| AWS Graviton3 | Neoverse V1 | c7g, r7g, m7g |
| Azure Cobalt 100 | Neoverse N2 | Dpsv6, Epsv6 |
| AWS Graviton2 | Neoverse N1 | c6g, r6g, m6g |
| RISC-V (planned) | MemPool/TeraPool | — |

## Repository Layout

```
monadic-hypervisor/
├── .cargo/
│   └── config.toml              # Cross-compilation: aarch64-unknown-none
├── Cargo.toml                   # Package + release profile (LTO, panic=abort)
├── Makefile                     # build / run / debug targets
├── linker.ld                    # ELF layout: ORIGIN=0x40000000, .text.boot first
├── arch/
│   ├── arm64/boot/
│   │   └── boot.S               # EL2 entry: park secondaries, HCR/VTCR/VTTBR, SP
│   └── riscv/boot/              # (planned) HS-mode entry
├── src/
│   ├── main.rs                  # #![no_std] entry: hypervisor_main() -> !
│   ├── dataplane/
│   │   ├── mod.rs
│   │   └── poll.rs              # SPSC queue + NVMe WFE/SEV polling engine
│   ├── hw/
│   │   ├── mod.rs
│   │   └── viommu.rs            # PCIe bypass + SMMUv3 DMA binding (stub)
│   └── mm/
│       ├── mod.rs
│       └── stage2.rs            # LPAE Stage-2 translation tables (4KB TG0)
├── docs/
│   ├── ADR-001-Zero-Kernel-Strict-No-Std.md
│   ├── PROGRESS_LEDGER.md
│   ├── SILICON_OBSERVATIONS.md  # Microarchitectural analysis & lessons learned
│   └── VISION.md
├── scripts/
│   ├── setup-toolchain.sh       # One-command prerequisite installer
│   └── spdk-aws/                # Graviton SPDK provisioning scripts
├── ARCHITECTURE.md
└── LICENSE
```

## Prerequisites

- **Rust** — nightly or stable with `aarch64-unknown-none` target
- **QEMU** — `qemu-system-aarch64` 8.0+ (9.0+ for `-cpu neoverse-v2` Graviton4 model)
- **GDB** — optional, for `make debug`

> **Why QEMU from source?** Amazon Linux 2023 (the default Graviton AMI) does
> not ship `qemu-system-aarch64` in its repos. QEMU 9.0+ is also needed for
> the `-cpu neoverse-v2` model that accurately simulates Graviton4 (Neoverse V2)
> microarchitecture — including LSE atomics, VHE, and the full ARMv8.5 feature
> set. The setup script below builds 9.2.2 from source as a fallback when no
> system package is available.

Install everything at once:

```bash
./scripts/setup-toolchain.sh
```

Or install components individually:

```bash
./scripts/setup-toolchain.sh rust   # Rust + aarch64-unknown-none target
./scripts/setup-toolchain.sh qemu   # QEMU (package manager, source fallback)
./scripts/setup-toolchain.sh gdb    # GDB with AArch64 support
```

See [scripts/setup-toolchain.sh](scripts/setup-toolchain.sh) for details
(distro detection, QEMU source build, version overrides via `QEMU_VERSION`).

## Usage

All commands are run from the repository root.

### Build

Cross-compile the `#![no_std]` hypervisor to a bare-metal AArch64 ELF binary:

```bash
make build
```

This runs `cargo build --release` targeting `aarch64-unknown-none` with:
- Full LTO (link-time optimisation)
- Single codegen unit (maximum inlining)
- `panic = "abort"` (no unwinding at EL2)
- Custom `linker.ld` (ORIGIN `0x40000000`, `.text.boot` first)

The output binary is `target/aarch64-unknown-none/release/monadic-hypervisor`.

### Run

Boot the hypervisor in a QEMU ARM64 virtual machine at Exception Level 2:

```bash
make run
```

QEMU launches with:
- `-machine virt,virtualization=on` — activates EL2 (without this, QEMU starts at EL1 and `boot.S` parks the core)
- `-cpu max` — enables LSE atomics, VHE, all ARMv8 extensions
- `-m 2G` — 2 GiB DRAM (`0x40000000`–`0xBFFFFFFF`)
- `-nographic` — UART0 on stdio

The hypervisor executes the full boot sequence:
1. `_start` (boot.S) → park secondaries, verify EL2, configure HCR_EL2/VTCR_EL2/VTTBR_EL2, load SP
2. `hypervisor_main` (main.rs) → Stage-2 MMU init, vIOMMU PCIe bypass, enter poll loop
3. `dataplane_poll_loop` (poll.rs) → SPSC ring poll with WFE yield

Exit QEMU with **Ctrl-A X**.

### Debug

Launch QEMU halted at the first instruction with a GDB remote stub on TCP port 1234:

```bash
make debug
```

In a second terminal, attach GDB:

```bash
gdb -ex "file target/aarch64-unknown-none/release/monadic-hypervisor" \
    -ex "target remote :1234"
```

Useful GDB commands for EL2 bare-metal debugging:

```gdb
(gdb) info registers pc cpsr          # Verify EL2: CPSR bits[3:2] = 0b10
(gdb) x/4i $pc                        # Disassemble at current PC
(gdb) bt                              # Backtrace: _start → hypervisor_main → poll_loop
(gdb) break *0x40000000               # Break at _start (DRAM base)
(gdb) continue                        # Resume execution
(gdb) stepi 100                       # Step 100 instructions
```

### QEMU Monitor (Interactive Inspection)

After `make run`, the console appears blank — the hypervisor emits nothing on
UART and enters its WFE poll loop. QEMU `-nographic` multiplexes a monitor on
stdio. Press **Ctrl-A C** to toggle into the QEMU monitor:

```
(qemu) info registers              # Full register dump — verify EL2 from CPSR
(qemu) info cpus                   # vCPU state (halted/running) and current PC
(qemu) xp /16xw 0x40000000        # Hex dump .text.boot (16 × 32-bit words)
(qemu) xp /8xg 0x40000000         # Hex dump (8 × 64-bit giant words)
(qemu) info mtree                  # Physical address map (GIC, UART, PCIe ECAM, DRAM)
(qemu) info qtree                  # Device tree — every virtio/PCI device
(qemu) system_reset                # Warm-reset vCPU back to _start
```

> **Note:** `$pc` is GDB syntax. In the QEMU monitor, run `info registers` to
> read the PC value, then pass the literal hex address: `xp /16xw 0x400010f0`.

> **Note:** `xp /4i <addr>` (instruction disassembly) requires QEMU built with
> Capstone (`--enable-capstone`). If you see `Asm output not supported on this
> arch`, use GDB or `llvm-objdump` for disassembly instead.

Press **Ctrl-A C** again to return to the serial console. **Ctrl-A H** prints
the escape-key help. **Ctrl-A X** quits QEMU.

### Clean

```bash
make clean
```

## Boot Path

```
QEMU / Graviton UEFI
        │
        ▼
   _start (boot.S @ 0x40000000)
   ├─ Park secondary cores (MPIDR_EL1.Aff0 ≠ 0 → WFE)
   ├─ Verify CurrentEL == EL2
   ├─ HCR_EL2  = 0x80000001  (RW=1, VM=1)
   ├─ VTCR_EL2 = 0x00023558  (4KB TG0, 40-bit IPA, SL0=L1)
   ├─ VTTBR_EL2 = 0           (fail-closed)
   ├─ SP = __stack_top & ~0x3F (64B cache-line aligned)
   └─ bl hypervisor_main
        │
        ▼
   hypervisor_main (main.rs)
   ├─ Phase 1: stage2_mmu_init()        → program VTTBR_EL2
   ├─ Phase 2: viommu_pcie_bypass_init() → NVMe BAR0 Stage-2 map
   └─ Phase 3: dataplane_poll_loop()     → SPSC + WFE/SEV (never returns)
```

## Documentation

| Document | Description |
|----------|-------------|
| [ARCHITECTURE.md](ARCHITECTURE.md) | High-level system architecture |
| [docs/ADR-001](docs/ADR-001-Zero-Kernel-Strict-No-Std.md) | Architecture Decision Record: mandatory `#![no_std]` |
| [docs/VISION.md](docs/VISION.md) | Product vision and scale-invariance thesis |
| [docs/SILICON_OBSERVATIONS.md](docs/SILICON_OBSERVATIONS.md) | Microarchitectural analysis: LDAR/STLR, MOESI isolation, WFE/SEV |
| [docs/PROGRESS_LEDGER.md](docs/PROGRESS_LEDGER.md) | Session-by-session engineering log |

## Troubleshooting

### `make: cargo: No such file or directory`

`make` runs under `/bin/sh` which does not source `~/.bashrc` or `~/.cargo/env`.
The Makefile uses `$(HOME)/.cargo/bin/cargo` as the full path. If you installed
Rust to a non-default location, edit the `CARGO` variable in the Makefile.

### `qemu-system-aarch64: failed to find romfile "efi-virtio.rom"`

A source-built QEMU does not know its ROM search path at runtime. The Makefile
passes `-L` pointing to the build tree. If your QEMU is installed to a different
prefix, override `QEMU_ROMDIR`:

```bash
make run QEMU_ROMDIR=/usr/local/share/qemu
```

### `Asm output not supported on this arch` in QEMU monitor

The `xp /i` instruction-disassembly format requires Capstone. Either rebuild
QEMU with `--enable-capstone`, or use GDB / `llvm-objdump` for disassembly:

```bash
llvm-objdump -d target/aarch64-unknown-none/release/monadic-hypervisor | head -60
```

## License

This project is released under the [BSD-2-Clause Plus Patent License](LICENSE).
