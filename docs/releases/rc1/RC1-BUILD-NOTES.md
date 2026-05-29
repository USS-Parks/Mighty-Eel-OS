# RC1 Build Notes

**Project:** Lamprey MAI
**Release:** RC1 (Tester Bundle)
**Date:** 2026-05-23
**Audience:** release engineers preparing the RC1 tarball; RC1 testers
verifying the prebuilt binary if one is supplied
**Plan reference:** `docs/COGENT-DEPLOYMENT-ROADMAP.md` Session RC-03
**Freeze point:** `dceaabc` (see `docs/RC1-FREEZE-NOTES.md`)
**Build commit (HEAD at build time):** `44f7bc4` (RC-02 docs commit; one
commit ahead of the freeze; pure-docs delta — same binary output as
building at `dceaabc`)

This document captures the release build of `mai-api`, the resulting
artifact, the host environment it was built on, and the smoke test
that proved the binary boots, prints a first-boot admin key, and
serves the health endpoints.

---

## 1. Build vs. Ship Decision (updated)

RC-02 reserved an optional `bin/` slot in the RC1 layout pending the
RC-03 build outcome. RC-03 succeeded with a clean 3m 14s release
build on Windows MSVC. **Updated decision:** RC1 v2 may ship with
`bin/lamprey-mai-api.exe` + `bin/lamprey-mai-ship-validate.exe` + a SHA-256 sidecar.
Whether to actually ship the binaries is left to the release engineer
at packaging time:

- **Source-only RC1 (default, v1):** testers build from source per
  the RC-04 quickstart. Smaller bundle (~15-20 MB sans `.git/`).
- **Source + binaries RC1 (v2):** add `bin/` populated from this
  build. Larger bundle (~25-30 MB sans `.git/`), no cargo build
  needed for a smoke-test tester.

Either shape is supported by the §2 layout from RC-02. The build
itself is reproducible against the freeze commit `dceaabc` with the
toolchain recorded in §4.

## 2. Build Invocation

```
cd mai/
cargo build --release -p mai-api
```

Result:

| Field | Value |
|---|---|
| Command | `cargo build --release -p mai-api` |
| Profile | release (optimised) |
| Wall clock | **3 m 14 s** (cold build, full workspace deps) |
| Warnings | **0** |
| Errors | **0** |
| Build start | 2026-05-23 20:32 (host clock, PDT) |
| Build finish | 2026-05-23 20:35 |
| Working tree | clean at build time except for RC-01/RC-02 outputs (since landed as commits `414ed97` + `44f7bc4`) |
| Started from | `cargo 1.95.0`, fresh on a populated `target/` (incremental cache; cold for the release profile because no prior `cargo build --release` had been run) |

## 3. Binary Details

Two binaries land under `mai/target/release/` because `mai-api` is a
two-binary crate (`src/main.rs` plus `src/bin/mai_ship_validate.rs`).

| Binary | Size | SHA-256 | Notes |
|---|---|---|---|
| `lamprey-mai-api.exe` | 10 361 856 B (9.9 MB) | `4e201a8498d3e46361c83fc4eff6e04c1021fca3187b04a4d9f55f398b1462b6` | The HTTP/REST + gRPC server. The "smallest practical runnable API artifact" the RC-03 goal asks for. |
| `lamprey-mai-ship-validate.exe` | 1 750 016 B (1.7 MB) | `a32ddc2891a7690cb015a9d1ed06cb84d4160f92976e61ac50cb14069e9ae8f8` | The SHIP-12 / SHIP-17 offline profile validator. Useful in RC2 hardening; included here because it is built in the same `cargo build -p mai-api` step. |

File type (both): `PE32+ executable for MS Windows 6.00 (console),
x86-64, 5 sections`.

`target/release/` total footprint at end of build: **761 MB** (mostly
intermediate `.rlib` and `.rmeta` artifacts under
`target/release/deps/`). `target/` overall is **54 GB** because prior
debug builds and prior tooling builds also live there; debug-state
hygiene is an RC-05 concern, not RC-03.

Future optimisation (deferred — RC-03 goal is "practical", not
"minimal"):

- `strip = "symbols"` in the release profile would likely cut
  `lamprey-mai-api.exe` by ~15-25 %.
- `lto = "fat"` + `codegen-units = 1` would trade build time for
  another ~10-15 %.
- A second pass with `opt-level = "z"` is rarely worth it for a
  server binary; skip.

None of these are blockers for RC1.

## 4. Build Environment

| Field | Value |
|---|---|
| Host OS | Windows 11 Home, build 26200 |
| Kernel | `MINGW64_NT-10.0-26200 Fire-Starter 3.6.7-fb42d713.x86_64 2026-03-29 11:44 UTC` |
| Shell | MSYS / MINGW64 (Git Bash) |
| Architecture | x86_64 |
| Rust toolchain | `rustc 1.95.0 (59807616e 2026-04-14)` |
| Cargo | `cargo 1.95.0 (f2d3ce0bd 2026-03-21)` |
| LLVM (rustc) | 22.1.2 |
| Target triple | `x86_64-pc-windows-msvc` |
| Linker | MSVC (default for the Windows MSVC toolchain) |
| Free disk at build start | 652 GB (workspace volume) |
| Free disk at build end | ~650 GB (estimate; release artifacts add ~1 GB net to `target/`) |

A Linux `glibc` reissue (for the `ship` profile's systemd-managed
deployment) requires a separate build pass under
`x86_64-unknown-linux-gnu` and is out of scope for RC1 v1. Tracked
for RC2 hardening.

## 5. Smoke Test

Acceptance for RC-03 requires the binary to start locally and the
health endpoint to respond. Both passed.

### 5.1 Invocation

The smoke test ran from a clean temporary CWD (`/tmp/mai-rc03-smoke/`)
that had no `config/` subdirectory, so the binary exercised the
first-boot path (mints + prints an admin key) rather than reading the
existing dev `config/auth_keys.toml` in the workspace.

```
cd /tmp/mai-rc03-smoke/
lamprey-mai-api.exe > mai-api-stdout.log 2> mai-api-stderr.log &
```

### 5.2 Boot timing

Process start to "MAI server ready" was **~57 ms** wall-clock
(JSON-log timestamps `03:36:36.044` → `03:36:36.097`). The server
came up under Scout tier defaults (no `MAI_SHIP_PROFILE` set,
`load_auth_state(None)` legacy bring-up path).

Boot sequence observed (paraphrased from log lines):

1. `Island Mountain MAI API Server version=0.1.0`
2. `No config file specified, using Scout tier defaults`
3. `MAI server starting tier=Scout rest_port=8420 grpc_port=8421 bind=127.0.0.1`
4. `Running air-gap startup verification` → `Air-gap verification passed`
5. `No scheduler config file found, using defaults`
6. `No topology config file found, using defaults`
7. `nvidia-smi unavailable, using flat topology` (WARN — expected on this dev host)
8. `GPU topology loaded gpus=1 nvlink_cliques=0`
9. `No KV config file found, using defaults`
10. `No scoring config found; scheduler will use least-loaded scoring` (WARN — expected default)
11. **First-boot banner printed on stdout** (see §6)
12. `First-boot admin key generated (printed to stdout, NOT logged)`
13. `No adapter config file found, starting without adapters`
14. `Adapters discovered count=0`
15. `All components initialized, building servers`
16. `REST server listening addr=127.0.0.1:8420`
17. `gRPC server listening addr=127.0.0.1:8421`
18. `MAI server ready — REST on 127.0.0.1:8420, gRPC on 127.0.0.1:8421`

### 5.3 Health endpoints

Three GET requests against `127.0.0.1:8420` all returned HTTP 200
with shape-correct JSON:

| Endpoint | HTTP | Body (excerpt) |
|---|---|---|
| `/v1/health` | 200 | `{"status":"healthy","alert_level":"Normal","adapters":[],"hardware":{"gpus":[],"air_gap_status":"compliant"},...}` |
| `/v1/health/system` | 200 | `{"disk_utilization_percent":0.0,"ram_utilization_percent":0.0,"cpu_utilization_percent":0.0}` |
| `/v1/health/adapters` | 200 | `{"adapters":[],"healthy":0,"total":0}` |

The aggregate health reports `air_gap_status: compliant` because the
Scout tier defaults treat local loopback as compliant (no external
connectivity expected without explicit config).

### 5.4 Shutdown

The process was stopped with `Stop-Process -Id <pid> -Force` from
PowerShell. Port 8420 freed within 2 seconds. No persistent state on
disk because the smoke-test CWD held no `config/auth_keys.toml` and
no writes landed under `~/.mai/` (the appliance state dir is
`/var/lib/mai/` on the ship profile; Scout defaults persist nothing).

## 6. First-Boot Admin Key Instructions

This is the operator-facing summary. The full design lives in
`docs/FIRST-BOOT.md` and the on-the-spot procedure in
`docs/runbooks/01-first-boot-and-key-capture.md`.

**What happens.** When `mai-api` starts and finds no
`config/auth_keys.toml` (Scout/dev mode) or no `[[keys]]` entries in
the file referenced by `profile.auth.auth_keys_path` (ship/production
mode), it mints one admin key, prints a banner on stdout, and then
either continues serving (Scout) or exits before binding any socket
(ship/production, per SHIP-17).

**What the banner looks like.** A single contiguous block:

```
========================================
  MAI FIRST-BOOT: Admin API Key
========================================
  Key:  im-<64 hex chars>          ← capture this NOW; never persisted
  Hash: <64 hex chars>             ← persist this in auth_keys.toml
  Role: admin
  Permissions: *
========================================
```

**What the tester must do.**

1. Capture the **Key** line. It is the only plaintext copy of the
   admin credential. The daemon does not log it (the line is emitted
   before the JSON-log sink reconfigures). If the tester loses it,
   the recovery path is "wipe state and reboot" — see
   `docs/FIRST-BOOT.md` §"What happens if the key is lost".
2. Persist the **Hash** to `config/auth_keys.toml` as a `[[keys]]`
   entry. The banner emits a ready-to-paste TOML block immediately
   below the hash line.
3. (Ship/production only) Restart the daemon. It now finds a
   `[[keys]]` entry, skips the mint step, and binds sockets.
   (Scout/dev only) The daemon already kept running — no restart
   needed. Use the captured key as the `X-IM-Auth-Token` (or the
   SDK's `api_key=` argument) for all subsequent calls.

**What the banner looked like during the RC-03 smoke test.** Hash
`0ce15d1807d01dafd3edf36a50f8a087016b0d712f6575bc81a569692fa5594f`,
plaintext key redacted from this doc (the smoke-test process was
killed; the key is dead). The smoke-test stdout log at
`/tmp/mai-rc03-smoke/mai-api-stdout.log` on the build machine holds
the unredacted banner if the build engineer needs it for a one-time
inspection; do not promote that file into `test-evidence/` or any
committed location.

## 7. Acceptance Checklist (RC-03)

| Criterion | Status |
|---|---|
| Release build succeeds | §2 (3 m 14 s, 0 warnings, 0 errors) |
| Release binary exists under `mai/target/release/` | §3 (`lamprey-mai-api.exe` 9.9 MB, `lamprey-mai-ship-validate.exe` 1.7 MB) |
| Binary size recorded | §3 (bytes + MiB) |
| Build environment recorded | §4 (rustc, cargo, host OS, target triple) |
| API starts locally | §5.1, §5.2 (Scout tier, ~57 ms to ready) |
| Basic health endpoint responds | §5.3 (`/v1/health` returns 200) |
| First-boot key instructions captured in human language | §6 |
| Notes written to `docs/RC1-BUILD-NOTES.md` | this file |

## 8. Notes and Caveats

- The build was run on Windows MSVC. RC1 v1 binaries (if shipped)
  are Windows-only. A Linux glibc reissue for the `ship` systemd
  deployment is RC2 hardening work.
- Two binaries are produced by the same `cargo build` invocation
  because the `mai-api` crate has two `[[bin]]` targets. Both belong
  in `bin/` if RC1 v2 ships binaries; `mai-ship-validate` is the
  offline profile validator used by SHIP-12 / SHIP-17 acceptance.
- The Scout tier first-boot path is the dev/bring-up path. The
  ship/production first-boot is stricter (fails closed on missing
  keys file, refuses to bind). Testers running the binary against
  the ship profile must follow `runbooks/01-first-boot-and-key-capture.md`.
- `cargo build --release` did not re-run any tests. RC-05 ("Test
  Evidence Refresh") is the dedicated session that re-runs the full
  test suite against the freeze commit.
- The `et HEAD` tree anomaly flagged in `RC1-PACKAGE-MANIFEST.md` §5
  is still present in the working tree at this build. It does not
  affect the binary in any way; cleanup is still pending before RC1
  cut.

## 9. Next Session

Session RC-04 (Beginner Quickstart) consumes this binary + first-boot
narrative and produces `README-FIRST.md` for the RC1 package — the
tester-facing "start here" document that explains what the software
is and is not, minimum hardware, first-run steps, demo steps, and
what success looks like at each step.
