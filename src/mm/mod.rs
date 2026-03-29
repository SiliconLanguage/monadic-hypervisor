/*
 * src/mm/ — Memory Management subsystem
 *
 * Stage-2 IPA→PA translation tables and physical frame allocation.
 * All table memory is statically pre-allocated in .bss — zero dynamic
 * allocation in the translation hot path (ARCHITECTURE.md §2.3).
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026  SiliconLanguage — Monadic Hypervisor Project
 */

pub mod stage2;
