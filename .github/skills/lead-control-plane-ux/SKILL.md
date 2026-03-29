---
name: lead-control-plane-ux
description: 'Transform a raw Python CLI prototype into a Director-ready, enterprise-grade control plane harness for the Tensorplane AI Foundry dataplane-emu stack. Use when: refactoring vibe_demo_agent, overhauling CLI aesthetics with SiliconLanguage brand (cyberpunk ANSI colors, lambda ASCII art), hardening run_demo.sh / setup_demo.sh venv bootstrap, modularizing MCP client + subprocess manager + UI renderer, adding strict Python typing, graceful C++ crash handling, executive docstrings, or updating README with control-to-data-plane bridge explanation. Scope: Python control plane and shell scripts only — never modify C++ SPDK engine code.'
argument-hint: 'Describe which part to work on: aesthetics, setup scripts, code refactoring, or documentation'
disable-model-invocation: true
---

# Lead Control Plane UX — SiliconLanguage Brand

## Domain Constraints

- **In scope**: `vibe_demo_agent/` Python files, `run_demo.sh`, `setup_demo.sh`, `README.md`
- **Out of scope**: Everything under `src/`, `include/`, `tests/`, `scripts/build_spdk.sh` — do **not** touch C++ SPDK engine or NVMe-oF target code

---

## Aesthetic Standard: SiliconLanguage Brand

### ANSI Palette

| Role | Code | Usage |
|------|------|-------|
| Cyan | `\033[36m` | Primary labels, headers, section titles |
| Magenta | `\033[35m` | Prompt glyph (`λ`), highlighted values, warnings |
| Dark Gray | `\033[90m` | Timestamps, secondary info, separators |
| Reset | `\033[0m` | Always terminate color sequences |

> **Avoid** bright/light variants (`\033[9Xm`). Prefer the strict dim palette above for the ultra-minimalist look.

### ASCII Logo (startup banner)

Display on interpreter startup using `print()` calls wrapped in color. The logo must feature the lambda `λ` symbol inside a 64-bit instruction encoding block:

```
\033[36m
 ╔══════════════════════════════════════╗
 ║  0xFE DC BA98 7654 3210  [λ] TENSOR  ║
 ║  silicon-fabric :: dataplane-emu     ║
 ╚══════════════════════════════════════╝
\033[0m
```

Adjust ASCII art to taste; the key elements are: hex instruction encoding, `λ`, and the product name.

### Telemetry Output Format

Live metrics must look like a precision hardware monitor, **not** a log file:

```
\033[90m──────────────────────────────────────\033[0m
\033[36m  IOPS       \033[0m  1,245,312 ops/s
\033[36m  LATENCY    \033[0m  \033[35m4.2 μs p99\033[0m
\033[36m  ZERO-COPY  \033[0m  ✓ verified (0 memcpy)
\033[90m──────────────────────────────────────\033[0m
```

---

## Procedure

### Step 1 — Aesthetic Overhaul

1. Replace all existing color constants with the strict palette above.
2. Replace any generic print statements with formatted telemetry blocks.
3. Insert the ASCII logo at module startup (before credential checks, after imports).
4. Replace bright `\033[9Xm` codes with dim equivalents throughout.

### Step 2 — Setup Script Hardening (`run_demo.sh` / `setup_demo.sh`)

**`setup_demo.sh` must:**
- Create venv with `python3 -m venv venv` guard (skip if already exists unless `--clean` flag passed)
- Install packages: `azure-cognitiveservices-speech python-dotenv` (pin major versions in a `requirements.txt`)
- After `source venv/bin/activate`, inject the Monad prompt into `venv/bin/activate` at the end:

```bash
# SiliconLanguage prompt
export PS1='\033[90m(silicon-fabric) \033[36m\W \033[35mλ \033[0m'
```

- Export credentials from `.env` using safe parsing (no `xargs` — use `set -a; source .env; set +a` to avoid word-splitting on values with spaces)

**`run_demo.sh` must:**
- Source `setup_demo.sh` (not run it as a sub-process; use `source ./setup_demo.sh` so env vars propagate)
- Validate `venv/bin/activate` exists before sourcing
- Trap `ERR` to print a magenta error banner and exit cleanly on any failure
- `clear` after venv activated, then launch `python3 vibe_demo_agent.py "$@"`

### Step 3 — Python Code Refactoring

Split `vibe_demo_agent.py` into three logical classes within the same file (or separate modules if the file exceeds ~300 lines):

#### `MCPClient`
- Wraps all Model Context Protocol interactions (prompt dispatch, response parsing)
- Constructor takes `speech_key`, `speech_region`
- Method: `speak(text: str, index: int) -> None` (synthesis + playback)
- Handles `speechsdk.ResultReason` failures with a typed exception `MCPSpeechError`

#### `DataplaneSubprocess`
- Wraps `subprocess.Popen` lifecycle for the C++ `spdk_tgt` or demo binary
- Method: `start(binary_path: str, args: list[str]) -> None`
- Method: `stop() -> None` — sends `SIGTERM`, waits with timeout, then `SIGKILL`
- Method: `is_alive() -> bool`
- Raises `DataplaneStartError` on non-zero exit within the first 2 seconds (fast-fail)
- Raises `NVMeBindError` if stderr contains `"bind failed"` or `"address already in use"`

#### `UIRenderer`
- Owns all terminal output
- Method: `banner() -> None` — prints ASCII logo
- Method: `telemetry() -> None` — reads metrics from the POSIX shared memory ring buffer (see **Telemetry IPC Contract** below) and renders the hardware-monitor block to stdout; runs at 10 Hz in a background thread
- Method: `error(msg: str) -> None` — magenta error with `[FAULT]` prefix
- Method: `info(msg: str) -> None` — cyan informational line

#### Telemetry IPC Contract — POSIX Shared Memory Ring Buffer

**Rationale:** MCP/JSON-RPC is designed for control-plane orchestration and tool discovery. It must **not** be used for streaming high-frequency data-plane metrics — the serialization overhead corrupts cycle-accurate latency measurements. Instead, use direct shared memory reads: the same zero-copy IPC pattern used in HFT pipelines and DeepSeek 3FS.

**Shared memory layout** (C++ side writes, Python side reads — one cache line, 64 bytes):

```
Segment name : "/dataplane_telemetry"   (→ /dev/shm/dataplane_telemetry on Linux)
Total size   : 64 bytes
Offset  0–7  : uint64_t  seq          // seqlock counter: odd = writer active
Offset  8–15 : uint64_t  iops         // ops/s snapshot
Offset 16–23 : double    latency_us   // p99 latency in microseconds
Offset 24    : uint8_t   zero_copy    // 1 = no memcpy verified, 0 = copy detected
Offset 25–63 : reserved / padding
```

**Python read loop** (implement inside `UIRenderer`, called from `telemetry()` thread):

```python
import mmap
import struct
import time

SHM_PATH = "/dev/shm/dataplane_telemetry"
SHM_SIZE = 64
# little-endian: seq(u64), iops(u64), latency_us(f64), zero_copy(bool) + padding
FMT = "<QQd?"

def _poll_shm(self) -> None:
    with open(SHM_PATH, "rb") as f:
        mm = mmap.mmap(f.fileno(), SHM_SIZE, access=mmap.ACCESS_READ)
        while self._running:
            # Seqlock read: retry on torn read (odd seq or seq changed)
            mm.seek(0)
            seq1, = struct.unpack_from("<Q", mm, 0)
            if seq1 & 1:          # writer holds lock
                continue
            seq, iops, latency_us, zero_copy = struct.unpack_from(FMT, mm, 0)
            seq2, = struct.unpack_from("<Q", mm, 0)
            if seq1 != seq2:      # torn read — retry
                continue
            self._render_block(iops, latency_us, bool(zero_copy))
            time.sleep(0.1)       # 10 Hz UI refresh — never blocks data-plane hot path
        mm.close()
```

Notes for implementer:
- `/dev/shm` is the Linux tmpfs mount for POSIX shared memory. `shm_open(3)` in C++ creates `/dev/shm/<name>`; Python's `mmap` accesses it with zero per-read syscall overhead after `open()`.
- `multiprocessing.shared_memory.SharedMemory(name="dataplane_telemetry")` is a valid higher-level alternative but introduces a small Python object overhead.
- `self._running` is a `threading.Event` cleared by `UIRenderer.stop()` to tear down the poll thread gracefully when `DataplaneSubprocess.stop()` is called.

**C++ side contract** (reference only — do **not** implement; shown so the Python implementer understands the seqlock write protocol):

```cpp
// After each batch in DataplaneRing / SqCq hot path:
std::atomic_thread_fence(std::memory_order_release);
shm->seq++;                      // odd: write begins
shm->iops        = measured_iops;
shm->latency_us  = p99_latency_us;
shm->zero_copy   = 1;
std::atomic_thread_fence(std::memory_order_release);
shm->seq++;                      // even: write complete
```

**Strict typing:**
- All functions must have full `-> ReturnType` annotations
- Use `from __future__ import annotations` at top of file
- Use `dataclasses.dataclass` for config/state structs where appropriate

**Error handling checklist:**
- [ ] `MCPSpeechError` on synthesis failure
- [ ] `DataplaneStartError` on subprocess non-zero exit (fast-fail)
- [ ] `NVMeBindError` on bind failure message in stderr
- [ ] `KeyboardInterrupt` at top-level main → graceful `DataplaneSubprocess.stop()` + exit 0
- [ ] Missing `.env` credentials → print `UIRenderer.error(...)` + `sys.exit(1)` (already in place — keep it)

### Step 4 — Documentation

**Executive docstrings** — each class must have a docstring of this structure:
```python
"""
<Class name>: <one-line role>.

Architecture:
    <2-3 sentences on where this fits in the control-to-data-plane bridge>

Attributes:
    <key attributes>

Raises:
    <exceptions it can raise>
"""
```

**`README.md` update** — add a section titled `## Control-to-Data Plane Bridge` that explains:
1. How Python orchestrates the C++ SPDK engine (process lifecycle, IPC, telemetry polling)
2. The three-class architecture (MCPClient → DataplaneSubprocess → UIRenderer)
3. How to run the demo end-to-end (`setup_demo.sh` → `run_demo.sh`)

---

## Quality Checklist

Before declaring work done, verify:

- [ ] `python3 -m py_compile vibe_demo_agent.py` exits 0
- [ ] `mypy vibe_demo_agent.py --strict` reports no errors (install mypy in venv if needed)
- [ ] `bash -n run_demo.sh` and `bash -n setup_demo.sh` exit 0 (syntax check)
- [ ] `.env` parsing uses `set -a; source .env; set +a` instead of `export $(xargs)`
- [ ] No `\033[9Xm` bright color codes remain
- [ ] ASCII logo renders correctly in a 80-column terminal
- [ ] All three classes have executive docstrings
- [ ] `README.md` contains `## Control-to-Data Plane Bridge` section
- [ ] `UIRenderer` telemetry loop uses `mmap` on `/dev/shm/dataplane_telemetry` — no MCP/JSON-RPC calls in the metrics path
- [ ] Seqlock retry logic present (odd-seq spin + seq1 != seq2 torn-read guard)
- [ ] `UIRenderer.stop()` clears `_running` flag to cleanly join the poll thread

---

## Reference Files

- Source prototype: [vibe_demo_agent/vibe_demo_agent.py](../../../vibe_demo_agent/vibe_demo_agent.py)
- Setup script: [vibe_demo_agent/setup_demo.sh](../../../vibe_demo_agent/setup_demo.sh)
- Run harness: [vibe_demo_agent/run_demo.sh](../../../vibe_demo_agent/run_demo.sh)
- Original prompt spec: [prompts/LeadControlPlaneUXEngineer.md](../../../prompts/LeadControlPlaneUXEngineer.md)
