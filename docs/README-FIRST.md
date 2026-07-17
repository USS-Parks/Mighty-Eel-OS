# README FIRST

**Project:** Lamprey MAI
**Release:** RC1 (Tester Bundle)
**Date:** 2026-05-23
**Audience:** technical testers and acquirer reviewers running this
package for the first time
**Time required:** about 30 minutes for the smoke test; another 30
minutes for the demos
**Plan reference:** `source/docs/COGENT-DEPLOYMENT-ROADMAP.md` Session RC-04

If you read nothing else, read this. It is the only document in this
package that assumes you have not yet looked at the rest of the repo.

---

## 1. What This Is

This bundle is the **Release Candidate 1 (RC1) tester package** for
the Lamprey MAI inference engine and the Lamprey compliance
governance stack that sits above it.

In one sentence: **MAI runs local AI inference; Lamprey decides what
that inference is allowed to do under HIPAA, ITAR/EAR, and OCAP
(tribal data sovereignty) rules, and signs an audit chain that proves
the decisions were made the way the policy said they would be.**

The freeze point you are testing is git commit `dceaabc` on `main`.
See `source/docs/RC1-FREEZE-NOTES.md` for the exact scope, including the
SHIP-01..17 hardening lane that sits on top of the Session 1-46
mainline build.

## 2. What This Is NOT

- **Not a customer installer.** No one should hand this bundle to a
  hospital, defence prime, or tribal data office and expect them to
  run it in production. That is what the Production Appliance tier
  is for; it is a later release in the roadmap.
- **Not safe for real regulated data.** Run it on a test machine
  with synthetic data only. Do not point it at a real EHR, real
  ITAR-controlled tech data, or real tribal records.
- **Not a one-click install.** A fresh-machine rehearsal is Session
  RC-06 of the roadmap, not RC1. You should expect to read this
  document and execute a small number of commands by hand.
- **Not a benchmark of a specific deployment.** The performance
  numbers in `source/docs/acquisition/READY.md` are reproducible against
  this freeze, but they are bench numbers, not field numbers from a
  customer deployment.

If something in the rest of this package contradicts the above, the
above wins. File an issue (section 8).

## 3. Minimum Hardware and Software

This RC1 is verifiable on a laptop. The smoke test does not need a
GPU. The integration demos do not need a GPU.

| Resource | Minimum | Reason |
|---|---|---|
| CPU | x86_64, 4 cores | Cargo workspace builds + Lamprey audit chain run cleanly on a modern laptop |
| RAM | 8 GB | Test suite peaks around 2-3 GB; 8 GB leaves room for IDE / browser |
| Free disk | 5 GB for binary-only path; **60 GB** for the source-build path | `target/` for a full release build of the workspace is large - the same 54 GB pattern we saw on the build host. Plan for it. |
| OS (binary path) | Windows 10 / 11 x86_64 | RC1 v2 prebuilt binaries are Windows MSVC only. Linux glibc reissue is RC2 work. |
| OS (source path) | Windows / macOS / Linux | Any platform with the toolchain below |
| Rust toolchain | `rustc 1.85` or newer (MSRV); RC1 was built with `1.95.0` | `Cargo.toml` pins `rust-version = "1.85"`; newer is fine |
| Python | `3.12` or newer | `pyproject.toml` pins `requires-python = ">=3.12"`; the SDK and dashboard need it |
| `git` | any recent version | needed only if you want to verify the freeze commit `dceaabc` |
| `curl` or PowerShell `Invoke-WebRequest` | any | for the health-endpoint check |

No GPU drivers are required. If `nvidia-smi` is absent the daemon
logs a single warning and falls back to a flat topology; that is
expected on a tester laptop.

**Python path note.** If you run any `pytest` invocation — the SDK,
the dashboard, or the application scaffolds (including the OpenBao
trust-demo) — set `PYTHONPATH` to the bundled SDK source root first.
Without it, pytest fails during collection with
`ModuleNotFoundError: No module named 'mai'`.

```
# POSIX shell, from Lamprey-MAI-RC1/:
export PYTHONPATH="$PWD/source/mai-sdk-python/src"
# PowerShell, from Lamprey-MAI-RC1\:
$env:PYTHONPATH = "$PWD\source\mai-sdk-python\src"
```

The cargo paths (§5.B, §6) do **not** need `PYTHONPATH`; it only
matters for `pytest`.

## 4. Layout

After you unpack the bundle:

```
Lamprey-MAI-RC1/
|-- README-FIRST.md           <- you are here
|-- source/                   <- the mai/ workspace (filtered, see RC1-PACKAGE-MANIFEST.md)
|   |-- docs/                 <- RC1-FREEZE-NOTES.md, RC1-PACKAGE-MANIFEST.md, RC1-BUILD-NOTES.md, runbooks/, acquisition/, ...
|   |-- mai-api/, mai-compliance/, ...
|   `-- ...
|-- bin/                      <- OPTIONAL - prebuilt binaries if this is an RC1 v2 bundle
|   |-- lamprey-mai-api.exe
|   |-- lamprey-mai-ship-validate.exe
|   `-- SHA256SUMS
`-- test-evidence/            <- captured logs from the RC-05 test re-run
```

`bin/` is present only if your bundle is an RC1 v2 (source + binaries).
If your bundle is RC1 v1 (source only), `bin/` is absent and you take
the source-build path in section 5.

## 5. First-Run Steps

Two paths. Pick one. The success criteria at the end are the same.

### 5.A Binary path (RC1 v2 only)

**Windows PowerShell:**

```
cd Lamprey-MAI-RC1
# (Optional) verify the binary against the published hashes:
Get-FileHash bin\lamprey-mai-api.exe -Algorithm SHA256
# Expect: 4E201A8498D3E46361C83FC4EFF6E04C1021FCA3187B04A4D9F55F398B1462B6

# Run from a clean working directory (no config\ subfolder) so the
# first-boot path fires and prints a fresh admin key.
mkdir mai-test-run; cd mai-test-run
..\bin\lamprey-mai-api.exe
```

**Hash case note.** `Get-FileHash` prints SHA-256 in upper-case hex;
`bin/SHA256SUMS` lists the same hashes in lower-case (Unix
convention). The two are equivalent — hex compares are
case-insensitive. The `Expect:` line above is upper-case so the
PowerShell output reads as an exact match.

### 5.B Source path (RC1 v1 or v2)

**POSIX shell:**

```
cd Lamprey-MAI-RC1/source
cargo build --release -p mai-api
# Expect: "Finished `release` profile [optimized] target(s) in N s"

cd ..; mkdir mai-test-run; cd mai-test-run
../source/target/release/lamprey-mai-api      # or lamprey-mai-api.exe on Windows
```

A cold release build of the workspace took **3 m 14 s** on our
reference machine (rustc 1.95.0, Windows MSVC, 4-core laptop). Cold
builds on other platforms are in the same order of magnitude.

### 5.C What success looks like

Whichever path you took, the daemon emits a JSON-formatted info log
stream and a single boxed banner block — **both on stdout** at the
RC1 freeze. If you want the logs separable from the banner, pipe
stdout through `grep -v '^==='` (POSIX) or `Select-String -Pattern
'^===' -NotMatch` (PowerShell) to drop the box. A future RC will
route logs to stderr; until then, do not configure log routing
expecting that split. The banner is the **first-boot admin key**:

```
========================================
  MAI FIRST-BOOT: Admin API Key
========================================
  Key:  im-<64 hex chars>
  Hash: <64 hex chars>
  Role: admin
  Permissions: *
========================================
```

The log stream ends with a line like:

```
MAI server ready — REST on 127.0.0.1:8420, gRPC on 127.0.0.1:8421
```

Boot to ready takes about **60 ms** on a modern laptop. If it takes
several seconds, something is wrong - see section 8.

**Capture the Key line right now.** Copy it into a password manager
or a tester-only scratch file. It is the only plaintext copy you
will ever see. If you lose it, the daemon's first-boot recovery path
is "wipe state and reboot" (see `source/docs/FIRST-BOOT.md`).

The Hash line is fine to keep in plain notes - it is a one-way
fingerprint. The banner also prints a ready-to-paste TOML block;
that block is what you would persist into `config/auth_keys.toml`
on a real install, but for the smoke test you can skip persistence.

### 5.D Health check

Open a second terminal in `Lamprey-MAI-RC1/`. With the daemon still
running from 5.A or 5.B:

**curl:**

```
curl http://127.0.0.1:8420/v1/health
```

**PowerShell:**

```
Invoke-WebRequest -Uri http://127.0.0.1:8420/v1/health |
  Select-Object -ExpandProperty Content
```

**What success looks like.** A JSON object beginning
`{"status":"healthy","alert_level":"Normal",...}` with `"air_gap_status":"compliant"`. HTTP 200.

If you see `"status":"healthy"` and HTTP 200, the smoke test passes.
Stop the daemon with `Ctrl-C` in the terminal where it is running,
or from a third terminal:

```
# POSIX:
kill <pid>
# PowerShell:
Stop-Process -Id <pid> -Force
```

Port 8420 frees within two seconds.

## 6. Demo Steps

These prove that the Lamprey compliance governance stack actually
makes the decisions the docs say it makes. Six tests, all under one
cargo command. None need a GPU.

```
cd Lamprey-MAI-RC1/source
cargo test -p mai-compliance --test compliance_demos
```

**What success looks like.**

```
running 6 tests
test test_hipaa_workflow ... ok
test test_itar_workflow ... ok
test test_ocap_workflow ... ok
test test_multi_domain ... ok
test test_audit_tamper ... ok
test test_trust_manifold_disconnected_and_expired ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured
```

Each test exercises a different acquisition scenario:

| Test | What it proves | Narrative |
|---|---|---|
| `test_hipaa_workflow` | PHI request → BAA deny → local route → HIPAA report | `source/docs/acquisition/demos/healthcare.md` |
| `test_itar_workflow` | ITAR-controlled tech data + non-US actor → DenyExport → ITAR report | `source/docs/acquisition/demos/defense.md` |
| `test_ocap_workflow` | Tribal data + Council role → RouteLocal → OCAP report | `source/docs/acquisition/demos/tribal.md` |
| `test_multi_domain` | HIPAA + ITAR + OCAP composed in one request; OCAP > ITAR > HIPAA precedence | `source/docs/acquisition/demos/multi-domain.md` |
| `test_audit_tamper` | Hash-chain `verify_chain` detects a mutated audit row and escalates Critical | embedded in the test |
| `test_trust_manifold_disconnected_and_expired` | **Trust Manifold dry-run** — air-gap path refuses to route when the trust bundle is expired or the revocation status is unknown | plan §A.14 |

**Performance baselines (optional, takes ~30 seconds extra).**

```
cargo test -p mai-compliance --test compliance_perf --release
```

Expected three-line summary in the test output:

- `composer_p99_under_5ms` - measured ~1.5 us on our reference host
- `audit_append_throughput_over_1000_per_sec` - measured ~9,003/s
- `report_generation_under_10_seconds` - measured ~16.7 ms

Your numbers will vary; the assertion is the budget, not the exact
value.

## 7. What You Are Not Expected to Do in RC1

- **Do not point this at real PHI, ITAR data, or tribal records.**
  This is a tester bundle.
- **Do not benchmark on a customer-representative workload.** RC2 is
  the right release for that.
- **Do not run on a host that is exposed to a network.** The Scout
  defaults bind to `127.0.0.1` only; if you change that, you are
  outside the RC1 scope.
- **Do not skip the `mai-ship-validate` step** if you are checking
  the ship-profile path (RC2 territory; instructions live in
  `source/docs/SHIP-PROFILE.md`).
- **Do not edit committed config files** to "fix" something during
  testing. Note it in your issue report instead (section 8) so the fix
  lands in the next RC1 reissue rather than on your machine.

## 8. What to Send Back If It Fails

We would rather hear about a small problem than discover it later.
For each issue, send:

1. **Which path you took** - binary (5.A) or source (5.B).
2. **Your platform** - OS + version, CPU model, RAM, free disk.
3. **The exact command** that failed (copy-paste from your shell).
4. **The full stderr output** of the failing command. For the daemon,
   that is the JSON-log stream. For `cargo`, that is the build or
   test output. Do not redact paths; do redact any plaintext admin
   keys you happened to capture.
5. **What you expected vs. what you saw.** "Expected HTTP 200 from
   /v1/health, got connection refused" is a complete report.
6. **The freeze commit you tested.** It should be `dceaabc` - run
   `git rev-parse HEAD` from `Lamprey-MAI-RC1/source/` (or
   `cd source; git log -1 --format=%H`) to confirm.

Open an issue at
[github.com/USS-Parks/Mighty-Eel-OS/issues](https://github.com/USS-Parks/Mighty-Eel-OS/issues)
with the above. If you do not have access to that repo, email the
release engineer who sent you this bundle and attach the same
information.

## 9. Where to Go After the Smoke Test Passes

- `source/docs/RC1-FREEZE-NOTES.md` - what is in this release and
  what is intentionally excluded.
- `source/docs/RC1-PACKAGE-MANIFEST.md` - exact folder layout and
  inclusion / exclusion rules.
- `source/docs/RC1-BUILD-NOTES.md` - the reference release build and
  smoke test that produced these binaries.
- `source/docs/FIRST-BOOT.md` - the full design of the first-boot
  key flow, including failure-recovery options.
- `source/docs/runbooks/` - fourteen numbered operator runbooks from
  SHIP-15 covering first-boot, rotate keys, install policy bundle,
  verify audit chain, generate compliance report, back-up / restore,
  recover-from-failed-upgrade, adapter crash loop, trust-bundle
  expired, audit-WAL tamper, air-gap violation, disk-almost-full.
- `source/docs/acquisition/` - the Gate D evidence package for
  acquirer reviewers, including architecture, competitive landscape,
  IP position, and integration patterns.
