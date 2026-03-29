/*
 * src/hw/ — Hardware Subsystems (PCIe Bypass, IOMMU, Device Assignment)
 *
 * Modules under hw/ implement the Hardware-Enlightened Pillar:
 * direct device assignment to guest VMs via Stage-2 MMIO mapping,
 * SMMUv3 stream-table programming, and PCIe BAR passthrough.
 *
 * All code is #![no_std] / ADR-001 compliant — core crate only.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026  SiliconLanguage — Monadic Hypervisor Project
 */

pub mod viommu;
