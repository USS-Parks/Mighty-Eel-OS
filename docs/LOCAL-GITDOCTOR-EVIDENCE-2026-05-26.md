# Local GitDoctor Evidence Package

Root: `C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1`

## Layer 1: Mapped Checks

Overall score: **90/100**
Checks: 58 total, 52 passed, 6 failed

This layer intentionally mirrors the Dougherty/GitDoctor finding families.

## Layer 2: Independent Implementations

These probes use mature local tools when installed. `SKIPPED` means the tool was not available or the project surface was absent; it is not a pass.

| Probe | Toolchain | Status | Title |
|---|---|---:|---|
| IND-RS-001 | Rust | PASS | cargo check workspace |
| IND-RS-002 | Rust | PASS | cargo clippy workspace |
| IND-RS-003 | Rust | FAIL | cargo test workspace |
| IND-RS-004 | Rust | FAIL | cargo audit |
| IND-RS-005 | Rust | FAIL | cargo deny |
| IND-PY-001 | Python | PASS | pytest repository tests |
| IND-PY-002 | Python | PASS | ruff lint |
| IND-PY-003 | Python | PASS | bandit security scan |
| IND-PY-004 | Python | FAIL | pip-audit dependency scan |
| IND-SEC-001 | Secrets | PASS | gitleaks secret scan |
| IND-SEC-002 | Secrets | FAIL | detect-secrets serial scan |
| IND-DOC-001 | Docker | SKIPPED | hadolint Dockerfile scan |
| IND-CPLX-001 | Complexity | PASS | tokei line-count scan |
| IND-CPLX-002 | Complexity | PASS | scc complexity scan |
| IND-CPLX-003 | Complexity | PASS | radon complexity scan |

## Layer 3: Adversarial Fixtures

| Probe | Status | Title |
|---|---:|---|
| ADV-001 | PASS | Known-bad and known-clean scanner fixtures |

## Probe Details

### ADV-001 Known-bad and known-clean scanner fixtures

Layer: `adversarial-fixture`
Toolchain: `scanner`
Status: `PASS`
Command: `C:\Python314\python.exe -m pytest tools/local_gitdoctor_tests -q`
Exit code: `0`

```text
.....                                                                    [100%]
5 passed in 0.57s
```

### IND-RS-001 cargo check workspace

Layer: `independent-implementation`
Toolchain: `Rust`
Status: `PASS`
Command: `cargo check --workspace`
Exit code: `0`

```text
 Checking clap_builder v4.6.0
   Compiling validator_derive v0.16.0
    Checking dashmap v6.2.1
   Compiling prost-build v0.13.5
    Checking miniz_oxide v0.8.9
    Checking notify-types v1.0.1
    Checking headers v0.4.1
    Checking serde_urlencoded v0.7.1
    Checking tracing-serde v0.2.0
    Checking icu_provider v2.2.0
    Checking chacha20poly1305 v0.10.1
    Checking tokio-util v0.7.18
    Checking tower v0.5.3
    Checking tracing-subscriber v0.3.23
    Checking tokio-native-tls v0.3.1
   Compiling pyo3-macros v0.22.6
   Compiling tonic-build v0.12.3
    Checking icu_normalizer v2.2.0
    Checking icu_properties v2.2.0
    Checking tokio-stream v0.1.18
    Checking h2 v0.4.14
    Checking tower v0.4.13
    Checking tokio-tungstenite v0.29.0
    Checking walkdir v2.5.0
   Compiling vswhom v0.1.0
   Compiling clap_derive v4.6.1
   Compiling winreg v0.52.0
   Compiling toml v0.8.23
    Checking hkdf v0.12.4
    Checking axum v0.7.9
    Checking windows-result v0.2.0
    Checking filetime v0.2.29
    Checking flate2 v1.1.9
    Checking windows-strings v0.1.0
    Checking notify v7.0.0
    Checking fdeflate v0.3.7
   Compiling windows-implement v0.58.0
   Compiling windows-interface v0.58.0
    Checking pxfm v0.1.29
    Checking idna_adapter v1.2.2
   Compiling mai-api v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-api)
   Compiling embed-resource v2.5.2
    Checking idna v1.1.0
    Checking bytemuck v1.25.0
    Checking png v0.18.1
    Checking byteorder-lite v0.1.0
    Checking url v2.5.8
    Checking mai-hil v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-hil)
    Checking mai-router v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-router)
    Checking windows-core v0.58.0
    Checking tower-http v0.6.11
    Checking validator v0.16.1
   Compiling mai-launcher v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\tools\mai-launcher)
    Checking clap v4.6.1
    Checking windows v0.58.0
    Checking rule-tester v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\tools\rule-tester)
    Checking mai-core v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-core)
    Checking moxcms v0.8.1
    Checking hyper v1.9.0
    Checking hyper-util v0.1.20
    Checking image v0.25.10
    Checking hyper-timeout v0.5.2
    Checking hyper-tls v0.6.0
    Checking axum v0.8.9
    Checking reqwest v0.12.28
    Checking tonic v0.12.3
    Checking mai-compliance v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-compliance)
    Checking mai-adapters v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-adapters)
    Checking mai-vault v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-vault)
    Checking mai-scheduler v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-scheduler)
    Checking mai-agent v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-agent)
    Checking mai-pkg-builder v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\tools\pkg-builder)
    Checking mai-sdk-rs v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-sdk-rs)
    Checking tonic-reflection v0.12.3
    Checking axum-extra v0.10.3
    Checking mai-admin v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\tools\mai-admin)
    Checking gen-trust-staging v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\tools\gen-trust-staging)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 09s
```

### IND-RS-002 cargo clippy workspace

Layer: `independent-implementation`
Toolchain: `Rust`
Status: `PASS`
Command: `cargo clippy --workspace -- -D warnings -A clippy::pedantic`
Exit code: `0`

```text
Checking mai-hil v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-hil)
   Compiling mai-api v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-api)
   Compiling mai-launcher v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\tools\mai-launcher)
    Checking mai-router v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-router)
    Checking mai-sdk-rs v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-sdk-rs)
    Checking rule-tester v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\tools\rule-tester)
    Checking mai-core v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-core)
    Checking mai-compliance v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-compliance)
    Checking mai-vault v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-vault)
    Checking mai-adapters v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-adapters)
    Checking mai-scheduler v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-scheduler)
    Checking mai-pkg-builder v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\tools\pkg-builder)
    Checking mai-agent v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-agent)
    Checking mai-admin v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\tools\mai-admin)
    Checking gen-trust-staging v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\tools\gen-trust-staging)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 40.80s
```

### IND-RS-003 cargo test workspace

Layer: `independent-implementation`
Toolchain: `Rust`
Status: `FAIL`
Command: `cargo test --workspace`
Exit code: `101`

```text
ers\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\mai-sdk-rs)
   Compiling tonic-reflection v0.12.3
   Compiling mai-pkg-builder v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\tools\pkg-builder)
   Compiling axum-extra v0.10.3
   Compiling mai-admin v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\tools\mai-admin)
   Compiling gen-trust-staging v0.1.0 (C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\tools\gen-trust-staging)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2m 09s
     Running unittests src\main.rs (target\debug\deps\gen_trust_staging-63901ec21a4fe3b0.exe)
     Running unittests src\lib.rs (target\debug\deps\mai_adapters-f9871af847c75a38.exe)
     Running tests\benchmarks.rs (target\debug\deps\benchmarks-462bd97f631b0f2d.exe)
     Running tests\e2e_integration.rs (target\debug\deps\e2e_integration-e85b2dfef23b23e5.exe)
     Running tests\integration_adapters.rs (target\debug\deps\integration_adapters-46205f85e094bb7d.exe)
     Running unittests src\lib.rs (target\debug\deps\mai_admin-04753dbbd9d99b7d.exe)
     Running unittests src\main.rs (target\debug\deps\lamprey_mai_admin-5a15dd84b31aa280.exe)
     Running tests\backup_e2e.rs (target\debug\deps\backup_e2e-ff198278b964b4f7.exe)
     Running tests\restore_e2e.rs (target\debug\deps\restore_e2e-eec8e3b748bb4809.exe)
     Running unittests src\lib.rs (target\debug\deps\mai_agent-e1ac0651f933f8e2.exe)
     Running tests\rag_pipeline_test.rs (target\debug\deps\rag_pipeline_test-b61ad18dbbcf09ae.exe)
     Running tests\task_lifecycle_test.rs (target\debug\deps\task_lifecycle_test-39aef54fb06003a7.exe)
     Running tests\tool_calling_test.rs (target\debug\deps\tool_calling_test-be822d73f289c5c2.exe)
     Running unittests src\lib.rs (target\debug\deps\mai_api-fd5d6dbc3fad1cd2.exe)
     Running unittests src\main.rs (target\debug\deps\lamprey_mai_api-96a60793d98ddd5e.exe)
     Running unittests src\bin\mai_ship_validate.rs (target\debug\deps\lamprey_mai_ship_validate-b29a9f0102d004a4.exe)
     Running tests\audit_wal.rs (target\debug\deps\audit_wal-f198c0f67acc3c64.exe)
     Running tests\auth_bypass_consistency.rs (target\debug\deps\auth_bypass_consistency-6f21d686595469ce.exe)
     Running tests\auth_gate_a.rs (target\debug\deps\auth_gate_a-7a9c054db272a970.exe)
     Running tests\compliance_integration.rs (target\debug\deps\compliance_integration-62825d59e0bafdbc.exe)
     Running tests\grpc_integration.rs (target\debug\deps\grpc_integration-42eb4277df4fc583.exe)
     Running tests\health_system_j13.rs (target\debug\deps\health_system_j13-2440b342f54986ca.exe)
     Running tests\http_integration.rs (target\debug\deps\http_integration-25f7858b2ac99bf5.exe)
     Running tests\production_guard.rs (target\debug\deps\production_guard-db4b7d15fc788c80.exe)
     Running tests\sealer_bootstrap.rs (target\debug\deps\sealer_bootstrap-a5b28becd1f2fd7d.exe)
     Running tests\sec_95_rate_limit.rs (target\debug\deps\sec_95_rate_limit-5541f5cc19541499.exe)
     Running tests\ship_07b_endpoints.rs (target\debug\deps\ship_07b_endpoints-5a2035890a6618b3.exe)
     Running tests\ship_11_observability.rs (target\debug\deps\ship_11_observability-2db5a30d1485ed89.exe)
     Running tests\ship_convergence.rs (target\debug\deps\ship_convergence-8b330b098cf8ddbd.exe)
     Running tests\ship_profile.rs (target\debug\deps\ship_profile-4bc3091ef11bf93b.exe)
     Running tests\streaming_integration.rs (target\debug\deps\streaming_integration-40b4b34838c18a1e.exe)
     Running tests\system_integration.rs (target\debug\deps\system_integration-ff72f0430ece41eb.exe)
     Running tests\trust_production.rs (target\debug\deps\trust_production-1080cc7bf94cfd78.exe)
     Running tests\vault_bootstrap.rs (target\debug\deps\vault_bootstrap-226bf4d8ff0cad8a.exe)
error: test failed, to rerun pass `-p mai-api --test vault_bootstrap`
```

### IND-RS-004 cargo audit

Layer: `independent-implementation`
Toolchain: `Rust`
Status: `FAIL`
Command: `cargo audit --db C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\.tmp\local-gitdoctor-evidence\cargo-advisory-db --stale --format json`
Exit code: `1`

```text
read beyond the end of the `&str` data and potentially leak contents of the out-of-bounds read (by raising a Python exception containing a copy of the data including the overflow).\n\nIn PyO3 0.24.1 this function will now allocate a `CString` to guarantee a terminating nul bytes. PyO3 0.25 will likely offer an alternative API which takes `&CStr` arguments.","date":"2025-04-01","aliases":["GHSA-pph8-gcv7-4qj5"],"related":[],"collection":"crates","categories":["memory-exposure"],"keywords":["buffer-overflow"],"cvss":null,"informational":null,"references":[],"source":null,"url":"https://github.com/PyO3/pyo3/issues/5005","withdrawn":null,"license":"CC0-1.0","expect-deleted":false},"versions":{"patched":[">=0.24.1"],"unaffected":[]},"affected":{"arch":[],"os":[],"functions":{"pyo3::types::PyString::from_object":["<0.24.1"],"pyo3::types::PyString::from_object_bound":["<0.24.1",">=0.21.0"]}},"package":{"name":"pyo3","version":"0.22.6","source":"registry+https://github.com/rust-lang/crates.io-index","checksum":"f402062616ab18202ae8319da13fa4279883a2b8a9d9f83f20dbade813ce1884","dependencies":[{"name":"cfg-if","version":"1.0.4","source":"registry+https://github.com/rust-lang/crates.io-index"},{"name":"indoc","version":"2.0.7","source":"registry+https://github.com/rust-lang/crates.io-index"},{"name":"libc","version":"0.2.186","source":"registry+https://github.com/rust-lang/crates.io-index"},{"name":"memoffset","version":"0.9.1","source":"registry+https://github.com/rust-lang/crates.io-index"},{"name":"once_cell","version":"1.21.4","source":"registry+https://github.com/rust-lang/crates.io-index"},{"name":"portable-atomic","version":"1.13.1","source":"registry+https://github.com/rust-lang/crates.io-index"},{"name":"pyo3-build-config","version":"0.22.6","source":"registry+https://github.com/rust-lang/crates.io-index"},{"name":"pyo3-ffi","version":"0.22.6","source":"registry+https://github.com/rust-lang/crates.io-index"},{"name":"pyo3-macros","version":"0.22.6","source":"registry+https://github.com/rust-lang/crates.io-index"},{"name":"unindent","version":"0.2.4","source":"registry+https://github.com/rust-lang/crates.io-index"}],"replace":null}}]},"warnings":{"unmaintained":[{"kind":"unmaintained","package":{"name":"proc-macro-error","version":"1.0.4","source":"registry+https://github.com/rust-lang/crates.io-index","checksum":"da25490ff9892aab3fcf7c36f08cfb902dd3e71ca0f9f9517bea02a73a5ce38c","dependencies":[{"name":"proc-macro-error-attr","version":"1.0.4","source":"registry+https://github.com/rust-lang/crates.io-index"},{"name":"proc-macro2","version":"1.0.106","source":"registry+https://github.com/rust-lang/crates.io-index"},{"name":"quote","version":"1.0.45","source":"registry+https://github.com/rust-lang/crates.io-index"},{"name":"syn","version":"1.0.109","source":"registry+https://github.com/rust-lang/crates.io-index"},{"name":"version_check","version":"0.9.5","source":"registry+https://github.com/rust-lang/crates.io-index"}],"replace":null},"advisory":{"id":"RUSTSEC-2024-0370","package":"proc-macro-error","title":"proc-macro-error is unmaintained","description":"proc-macro-error's maintainer seems to be unreachable, with no commits for 2 years, no releases pushed for 4 years, and no activity on the GitLab repo or response to email.\n\nproc-macro-error also depends on `syn 1.x`, which may be bringing duplicate dependencies into dependant build trees.\n\n## Possible Alternative(s)\n\n- [manyhow](https://crates.io/crates/manyhow)\n- [proc-macro-error2](https://crates.io/crates/proc-macro-error2)\n- [proc-macro2-diagnostics](https://github.com/SergioBenitez/proc-macro2-diagnostics)","date":"2024-09-01","aliases":[],"related":[],"collection":"crates","categories":[],"keywords":[],"cvss":null,"informational":"unmaintained","references":[],"source":null,"url":"https://gitlab.com/CreepySkeleton/proc-macro-error/-/issues/20","withdrawn":null,"license":"CC0-1.0","expect-deleted":false},"affected":null,"versions":{"patched":[],"unaffected":[]}}]}}
```

### IND-RS-005 cargo deny

Layer: `independent-implementation`
Toolchain: `Rust`
Status: `FAIL`
Command: `cargo deny check`
Exit code: `1`

```text
o introduce a DNS entry (and TLS certificate) for an `xn--`-masked name that turns into the name of the target when processed by `idna` 0.5.0 or earlier.
      
      ## Remedy
      
      Upgrade to `idna` 1.0.3 or later, if depending on `idna` directly, or to `url` 2.5.4 or later, if depending on `idna` via `url`. (This issue was fixed in `idna` 1.0.0, but versions earlier than 1.0.3 are not recommended for other reasons.)
      
      When upgrading, please take a moment to read about [alternative Unicode back ends for `idna`](https://docs.rs/crate/idna_adapter/latest).
      
      If you are using Rust earlier than 1.81 in combination with SQLx 0.8.2 or earlier, please also read an [issue](https://github.com/servo/rust-url/issues/992) about combining them with `url` 2.5.4 and `idna` 1.0.3.
      
      ## Additional information
      
      This issue resulted from `idna` 0.5.0 and earlier implementing the UTS 46 specification literally on this point and the specification having this bug. The specification bug has been fixed in [revision 33 of UTS 46](https://www.unicode.org/reports/tr46/tr46-33.html#Modifications).
      
      ## Acknowledgements
      
      Thanks to kageshiron for recognizing the security implications of this behavior.
    ├ Announcement: https://bugzilla.mozilla.org/show_bug.cgi?id=1887898
    ├ Solution: Upgrade to >=1.0.0 (try `cargo update -p idna`)
    ├ idna v0.4.0
      └── validator v0.16.1
          └── mai-api v0.1.0

error[unmaintained]: proc-macro-error is unmaintained
    ┌─ C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1/Cargo.lock:214:1
    │
214 │ proc-macro-error 1.0.4 registry+https://github.com/rust-lang/crates.io-index
    │ ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ unmaintained advisory detected
    │
    ├ ID: RUSTSEC-2024-0370
    ├ Advisory: https://rustsec.org/advisories/RUSTSEC-2024-0370
    ├ proc-macro-error's maintainer seems to be unreachable, with no commits for 2 years, no releases pushed for 4 years, and no activity on the GitLab repo or response to email.
      
      proc-macro-error also depends on `syn 1.x`, which may be bringing duplicate dependencies into dependant build trees.
      
      ## Possible Alternative(s)
      
      - [manyhow](https://crates.io/crates/manyhow)
      - [proc-macro-error2](https://crates.io/crates/proc-macro-error2)
      - [proc-macro2-diagnostics](https://github.com/SergioBenitez/proc-macro2-diagnostics)
    ├ Announcement: https://gitlab.com/CreepySkeleton/proc-macro-error/-/issues/20
    ├ Solution: No safe upgrade is available!
    ├ proc-macro-error v1.0.4
      └── validator_derive v0.16.0
          └── validator v0.16.1
              └── mai-api v0.1.0

error[vulnerability]: Risk of buffer overflow in `PyString::from_object`
    ┌─ C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1/Cargo.lock:222:1
    │
222 │ pyo3 0.22.6 registry+https://github.com/rust-lang/crates.io-index
    │ ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ security vulnerability detected
    │
    ├ ID: RUSTSEC-2025-0020
    ├ Advisory: https://rustsec.org/advisories/RUSTSEC-2025-0020
    ├ `PyString::from_object` took `&str` arguments and forwarded them directly to the Python C API without checking for terminating nul bytes. This could lead the Python interpreter to read beyond the end of the `&str` data and potentially leak contents of the out-of-bounds read (by raising a Python exception containing a copy of the data including the overflow).
      
      In PyO3 0.24.1 this function will now allocate a `CString` to guarantee a terminating nul bytes. PyO3 0.25 will likely offer an alternative API which takes `&CStr` arguments.
    ├ Announcement: https://github.com/PyO3/pyo3/issues/5005
    ├ Solution: Upgrade to >=0.24.1 (try `cargo update -p pyo3`)
    ├ pyo3 v0.22.6
      └── mai-adapters v0.1.0
          └── mai-api v0.1.0
```

### IND-PY-001 pytest repository tests

Layer: `independent-implementation`
Toolchain: `Python`
Status: `PASS`
Command: `C:\Python314\python.exe -m pytest -q --ignore=target --ignore=results`
Exit code: `0`

```text
....................................................sssssss............. [  5%]
............ssssss.....................................sssss............ [ 10%]
..........ssssss..............................................ssssss.... [ 16%]
.........................................ssssss......................... [ 21%]
......................ssssss............................................ [ 27%]
.............................sssss...................................... [ 32%]
.................................................ssssss................. [ 38%]
........................................................................ [ 43%]
sssss.....................................................sssssss....... [ 49%]
........................................................................ [ 54%]
........................................................................ [ 60%]
........................................................................ [ 65%]
........................................................................ [ 71%]
...............................................................s......s. [ 76%]
.................ssssssssssss.sssssssss................................. [ 82%]
........................................................................ [ 87%]
........................................................................ [ 93%]
........................................................................ [ 98%]
..............                                                           [100%]
1222 passed, 88 skipped in 113.18s (0:01:53)
```

### IND-PY-002 ruff lint

Layer: `independent-implementation`
Toolchain: `Python`
Status: `PASS`
Command: `C:\Python314\python.exe -m ruff check .`
Exit code: `0`

```text
All checks passed!
```

### IND-PY-003 bandit security scan

Layer: `independent-implementation`
Toolchain: `Python`
Status: `PASS`
Command: `C:\Python314\python.exe -m bandit -r . -f json -c pyproject.toml`
Exit code: `0`

```text
NCE.HIGH": 0,
      "CONFIDENCE.LOW": 0,
      "CONFIDENCE.MEDIUM": 0,
      "CONFIDENCE.UNDEFINED": 0,
      "SEVERITY.HIGH": 0,
      "SEVERITY.LOW": 0,
      "SEVERITY.MEDIUM": 0,
      "SEVERITY.UNDEFINED": 0,
      "loc": 110,
      "nosec": 0,
      "skipped_tests": 0
    },
    ".\\tools\\trace-tools\\reconstruct.py": {
      "CONFIDENCE.HIGH": 0,
      "CONFIDENCE.LOW": 0,
      "CONFIDENCE.MEDIUM": 0,
      "CONFIDENCE.UNDEFINED": 0,
      "SEVERITY.HIGH": 0,
      "SEVERITY.LOW": 0,
      "SEVERITY.MEDIUM": 0,
      "SEVERITY.UNDEFINED": 0,
      "loc": 89,
      "nosec": 0,
      "skipped_tests": 0
    },
    ".\\tools\\trace-tools\\tests\\test_trace_tools.py": {
      "CONFIDENCE.HIGH": 0,
      "CONFIDENCE.LOW": 0,
      "CONFIDENCE.MEDIUM": 0,
      "CONFIDENCE.UNDEFINED": 0,
      "SEVERITY.HIGH": 0,
      "SEVERITY.LOW": 0,
      "SEVERITY.MEDIUM": 0,
      "SEVERITY.UNDEFINED": 0,
      "loc": 132,
      "nosec": 0,
      "skipped_tests": 0
    },
    "_totals": {
      "CONFIDENCE.HIGH": 0,
      "CONFIDENCE.LOW": 0,
      "CONFIDENCE.MEDIUM": 0,
      "CONFIDENCE.UNDEFINED": 0,
      "SEVERITY.HIGH": 0,
      "SEVERITY.LOW": 0,
      "SEVERITY.MEDIUM": 0,
      "SEVERITY.UNDEFINED": 0,
      "loc": 33002,
      "nosec": 0,
      "skipped_tests": 2
    }
  },
  "results": []
}
[main]	INFO	profile include tests: None
[main]	INFO	profile exclude tests: B310,B607,B105,B311,B603,B404,B101
[main]	INFO	cli include tests: None
[main]	INFO	cli exclude tests: None
[manager]	WARNING	Test in comment: HTML is not a test name or id, ignoring
[manager]	WARNING	Test in comment: template is not a test name or id, ignoring
[manager]	WARNING	Test in comment: every is not a test name or id, ignoring
[manager]	WARNING	Test in comment: var is not a test name or id, ignoring
[manager]	WARNING	Test in comment: passes is not a test name or id, ignoring
[manager]	WARNING	Test in comment: through is not a test name or id, ignoring
[manager]	WARNING	Test in comment: html is not a test name or id, ignoring
[manager]	WARNING	Test in comment: escape is not a test name or id, ignoring
[manager]	WARNING	Test in comment: not is not a test name or id, ignoring
[manager]	WARNING	Test in comment: a is not a test name or id, ignoring
[manager]	WARNING	Test in comment: SQL is not a test name or id, ignoring
[manager]	WARNING	Test in comment: statement is not a test name or id, ignoring
[tester]	WARNING	nosec encountered (B608), but no failed test on file .\compliance-dashboard\app.py:273
[tester]	WARNING	nosec encountered (B608), but no failed test on file .\compliance-dashboard\app.py:273
[tester]	WARNING	nosec encountered (B608), but no failed test on file .\compliance-dashboard\app.py:274
[tester]	WARNING	nosec encountered (B608), but no failed test on file .\compliance-dashboard\app.py:274
[tester]	WARNING	nosec encountered (B608), but no failed test on file .\compliance-dashboard\app.py:277
[tester]	WARNING	nosec encountered (B608), but no failed test on file .\compliance-dashboard\app.py:278
[tester]	WARNING	nosec encountered (B608), but no failed test on file .\compliance-dashboard\app.py:285
[manager]	WARNING	Test in comment: test is not a test name or id, ignoring
[manager]	WARNING	Test in comment: asserts is not a test name or id, ignoring
[manager]	WARNING	Test in comment: ABSENCE is not a test name or id, ignoring
[manager]	WARNING	Test in comment: of is not a test name or id, ignoring
[manager]	WARNING	Test in comment: 0 is not a test name or id, ignoring
[manager]	WARNING	Test in comment: 0 is not a test name or id, ignoring
[manager]	WARNING	Test in comment: 0 is not a test name or id, ignoring
[manager]	WARNING	Test in comment: 0 is not a test name or id, ignoring
[manager]	WARNING	Test in comment: never is not a test name or id, ignoring
[manager]	WARNING	Test in comment: binds is not a test name or id, ignoring
[tester]	WARNING	nosec encountered (B104), but no failed test on file .\tools\packaging_tests\test_systemd_units.py:167
```

### IND-PY-004 pip-audit dependency scan

Layer: `independent-implementation`
Toolchain: `Python`
Status: `FAIL`
Command: `C:\Python314\python.exe -m pip_audit --cache-dir C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\.tmp\local-gitdoctor-evidence\pip-audit-cache --progress-spinner off --format json`
Exit code: `1`

```text
mai-sdk (0.2.0)"}, {"name": "mando", "version": "0.7.1", "vulns": []}, {"name": "markdown-it-py", "version": "4.2.0", "vulns": []}, {"name": "mdurl", "version": "0.1.2", "vulns": []}, {"name": "msgpack", "version": "1.1.2", "vulns": []}, {"name": "mypy", "version": "2.1.0", "vulns": []}, {"name": "mypy-extensions", "version": "1.1.0", "vulns": []}, {"name": "packageurl-python", "version": "0.17.6", "vulns": []}, {"name": "packaging", "version": "26.2", "vulns": []}, {"name": "pathspec", "version": "1.1.1", "vulns": []}, {"name": "pillow", "version": "12.2.0", "vulns": []}, {"name": "pip", "version": "26.0.1", "vulns": [{"id": "CVE-2026-3219", "fix_versions": ["26.1"], "aliases": ["GHSA-58qw-9mgm-455v"], "description": "pip handles concatenated tar and ZIP files as ZIP files regardless of filename or whether a file is both a tar and ZIP file. This behavior could result in confusing installation behavior, such as installing \"incorrect\" files according to the filename of the archive. New behavior only proceeds with installation if the file identifies uniquely as a ZIP or tar archive, not as both."}, {"id": "CVE-2026-6357", "fix_versions": ["26.1"], "aliases": ["GHSA-jp4c-xjxw-mgf9"], "description": "pip prior to version 26.1 would run self-update check functionality after installing wheel files which required importing well-known Python modules names. These module imports were intentionally deferred to increase startup time of the pip CLI. The patch changes self-update functionality to run before wheels are installed to prevent newly-installed modules from being imported shortly after the installation of a wheel package. Users should still review package contents prior to installation."}]}, {"name": "pip-api", "version": "0.0.34", "vulns": []}, {"name": "pip-audit", "version": "2.10.0", "vulns": []}, {"name": "pip-requirements-parser", "version": "32.0.1", "vulns": []}, {"name": "pip-tools", "version": "7.5.3", "vulns": []}, {"name": "platformdirs", "version": "4.9.6", "vulns": []}, {"name": "pluggy", "version": "1.6.0", "vulns": []}, {"name": "py-serializable", "version": "2.1.0", "vulns": []}, {"name": "pycparser", "version": "3.0", "vulns": []}, {"name": "pydantic", "version": "2.13.4", "vulns": []}, {"name": "pydantic-core", "version": "2.46.4", "vulns": []}, {"name": "pygments", "version": "2.20.0", "vulns": []}, {"name": "pyparsing", "version": "3.3.2", "vulns": []}, {"name": "pypdf", "version": "6.12.1", "vulns": []}, {"name": "pyproject-hooks", "version": "1.2.0", "vulns": []}, {"name": "pytest", "version": "9.0.3", "vulns": []}, {"name": "pytest-asyncio", "version": "1.3.0", "vulns": []}, {"name": "python-docx", "version": "1.2.0", "vulns": []}, {"name": "python-dotenv", "version": "1.2.2", "vulns": []}, {"name": "python-multipart", "version": "0.0.29", "vulns": []}, {"name": "pyyaml", "version": "6.0.3", "vulns": []}, {"name": "radon", "version": "6.0.1", "vulns": []}, {"name": "requests", "version": "2.34.2", "vulns": []}, {"name": "rich", "version": "15.0.0", "vulns": []}, {"name": "ruff", "version": "0.15.14", "vulns": []}, {"name": "setuptools", "version": "82.0.1", "vulns": []}, {"name": "six", "version": "1.17.0", "vulns": []}, {"name": "sortedcontainers", "version": "2.4.0", "vulns": []}, {"name": "starlette", "version": "1.0.1", "vulns": []}, {"name": "stevedore", "version": "5.8.0", "vulns": []}, {"name": "structlog", "version": "25.5.0", "vulns": []}, {"name": "tomli", "version": "2.4.1", "vulns": []}, {"name": "tomli-w", "version": "1.2.0", "vulns": []}, {"name": "typing-extensions", "version": "4.15.0", "vulns": []}, {"name": "typing-inspection", "version": "0.4.2", "vulns": []}, {"name": "urllib3", "version": "2.7.0", "vulns": []}, {"name": "uvicorn", "version": "0.47.0", "vulns": []}, {"name": "watchfiles", "version": "1.2.0", "vulns": []}, {"name": "websockets", "version": "16.0", "vulns": []}, {"name": "wheel", "version": "0.47.0", "vulns": []}], "fixes": []}

Found 2 known vulnerabilities in 1 package
```

### IND-SEC-001 gitleaks secret scan

Layer: `independent-implementation`
Toolchain: `Secrets`
Status: `PASS`
Command: `gitleaks detect --source . --no-git --redact`
Exit code: `0`

```text
○
    │╲
    │ ○
    ○ ░
    ░    gitleaks

[90m9:25PM[0m [32mINF[0m [1mscanned ~13041108 bytes (13.04 MB) in 866ms[0m
[90m9:25PM[0m [32mINF[0m [1mno leaks found[0m
```

### IND-SEC-002 detect-secrets serial scan

Layer: `independent-implementation`
Toolchain: `Secrets`
Status: `FAIL`
Command: `C:\Python314\python.exe C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai-worktrees\mai-GOV-1\tools\detect_secrets_serial_scan.py --all-files --fail-on-findings --exclude-files (^|[\\/])(\.git|\.mypy_cache|\.pytest_cache|\.pytest-tmp|\.ruff_cache|\.tmp|node_modules|py_tmp_dir|pytest-cache-files-[^\\/]+|pytest_temp|results|target|temp_test_dir|test-evidence)([\\/]|$)|^docs[\\/]LOCAL-GITDOCTOR-EVIDENCE\.(json|md)$|^docs[\\/]LOCAL-GITDOCTOR-REPORT\.(json|md)$ .`
Exit code: `1`

```text
Documents\\Claude\\Island Mountain Mighty Eel OS\\mai-worktrees\\mai-GOV-1\\mai-sdk-python\\docs\\authentication.md",
        "hashed_secret": "e59482cfc1d81e2c1fd4ad767bd8be6a8ef9cff3",
        "is_verified": false,
        "line_number": 11,
        "type": "Secret Keyword"
      }
    ],
    "mai-sdk-python\\docs\\quickstart.md": [
      {
        "filename": "C:\\Users\\17076\\Documents\\Claude\\Island Mountain Mighty Eel OS\\mai-worktrees\\mai-GOV-1\\mai-sdk-python\\docs\\quickstart.md",
        "hashed_secret": "e59482cfc1d81e2c1fd4ad767bd8be6a8ef9cff3",
        "is_verified": false,
        "line_number": 25,
        "type": "Secret Keyword"
      }
    ],
    "mai-sdk-python\\src\\mai\\config.py": [
      {
        "filename": "C:\\Users\\17076\\Documents\\Claude\\Island Mountain Mighty Eel OS\\mai-worktrees\\mai-GOV-1\\mai-sdk-python\\src\\mai\\config.py",
        "hashed_secret": "e59482cfc1d81e2c1fd4ad767bd8be6a8ef9cff3",
        "is_verified": false,
        "line_number": 100,
        "type": "Secret Keyword"
      }
    ],
    "mai-sdk-python\\tests\\test_config.py": [
      {
        "filename": "C:\\Users\\17076\\Documents\\Claude\\Island Mountain Mighty Eel OS\\mai-worktrees\\mai-GOV-1\\mai-sdk-python\\tests\\test_config.py",
        "hashed_secret": "931adbe8ed8b0d58fad8c003a89eb53a44bf719f",
        "is_verified": false,
        "line_number": 57,
        "type": "Secret Keyword"
      },
      {
        "filename": "C:\\Users\\17076\\Documents\\Claude\\Island Mountain Mighty Eel OS\\mai-worktrees\\mai-GOV-1\\mai-sdk-python\\tests\\test_config.py",
        "hashed_secret": "3e0f4f09b45944720e4c995bf92f3103902ca2d6",
        "is_verified": false,
        "line_number": 65,
        "type": "Secret Keyword"
      },
      {
        "filename": "C:\\Users\\17076\\Documents\\Claude\\Island Mountain Mighty Eel OS\\mai-worktrees\\mai-GOV-1\\mai-sdk-python\\tests\\test_config.py",
        "hashed_secret": "3ff12f06475255a18c4744d4a1fa9c23a4470841",
        "is_verified": false,
        "line_number": 72,
        "type": "Secret Keyword"
      }
    ],
    "scripts\\build-package.ps1": [
      {
        "filename": "C:\\Users\\17076\\Documents\\Claude\\Island Mountain Mighty Eel OS\\mai-worktrees\\mai-GOV-1\\scripts\\build-package.ps1",
        "hashed_secret": "20a14ede53d8f5ab25cbbea16357ffdd887e101d",
        "is_verified": false,
        "line_number": 142,
        "type": "Base64 High Entropy String"
      }
    ],
    "tools\\gpu_release_tests\\test_bundle_scripts.py": [
      {
        "filename": "C:\\Users\\17076\\Documents\\Claude\\Island Mountain Mighty Eel OS\\mai-worktrees\\mai-GOV-1\\tools\\gpu_release_tests\\test_bundle_scripts.py",
        "hashed_secret": "26019c2e7b54c3d5b828190796fa49f2ae4b1a43",
        "is_verified": false,
        "line_number": 97,
        "type": "Hex High Entropy String"
      },
      {
        "filename": "C:\\Users\\17076\\Documents\\Claude\\Island Mountain Mighty Eel OS\\mai-worktrees\\mai-GOV-1\\tools\\gpu_release_tests\\test_bundle_scripts.py",
        "hashed_secret": "158b484ae1f6f64f89da22397d25fbdafad02252",
        "is_verified": false,
        "line_number": 125,
        "type": "Hex High Entropy String"
      },
      {
        "filename": "C:\\Users\\17076\\Documents\\Claude\\Island Mountain Mighty Eel OS\\mai-worktrees\\mai-GOV-1\\tools\\gpu_release_tests\\test_bundle_scripts.py",
        "hashed_secret": "ff998abc1ce6d8f01a675fa197368e44c8916e9c",
        "is_verified": false,
        "line_number": 184,
        "type": "Hex High Entropy String"
      }
    ],
    "tools\\mai-admin\\src\\audit.rs": [
      {
        "filename": "C:\\Users\\17076\\Documents\\Claude\\Island Mountain Mighty Eel OS\\mai-worktrees\\mai-GOV-1\\tools\\mai-admin\\src\\audit.rs",
        "hashed_secret": "c0174d8dfe9687a8f29297449712d6ba12ed2bc3",
        "is_verified": false,
        "line_number": 19,
        "type": "Hex High Entropy String"
      }
    ]
  },
  "version": "1.5.0"
}
```

### IND-DOC-001 hadolint Dockerfile scan

Layer: `independent-implementation`
Toolchain: `Docker`
Status: `SKIPPED`
Command: `hadolint Dockerfile`
Reason: tool not installed: hadolint

### IND-CPLX-001 tokei line-count scan

Layer: `independent-implementation`
Toolchain: `Complexity`
Status: `PASS`
Command: `tokei .`
Exit code: `0`

```text
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Language              Files        Lines         Code     Comments       Blanks
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Dockerfile                1          167           52           99           16
 INI                       1           31           21            6            4
 JSON                      2         1293         1293            0            0
 PowerShell                9         1403         1118          127          158
 Protocol Buffers          2         1171          745          256          170
 Python                  244        41053        33492         1207         6354
 Shell                    14         1988         1478          277          233
 Plain Text               18          304            0          288           16
 TOML                     59         3103         1680         1023          400
 YAML                      2         1660         1539           53           68
─────────────────────────────────────────────────────────────────────────────────
 Markdown                187        37614            0        28499         9115
 |- BASH                  39          418          354           42           22
 |- HCL                    1            6            5            1            0
 |- JSON                  24          919          914            0            5
 |- Markdown               1           21            0           11           10
 |- PowerShell            36          253          189           43           21
 |- Python                13          244          199           14           31
 |- Rust                   9          276          210           56           10
 |- TOML                   7          203          177            2           24
 |- YAML                   1            2            2            0            0
 (Total)                            39956         2050        28668         9238
─────────────────────────────────────────────────────────────────────────────────
 Rust                    265        87352        73856         3398        10098
 |- Markdown             262        11606           20        10476         1110
 (Total)                            98958        73876        13874        11208
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Total                   804       191087       117344        45878        27865
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

### IND-CPLX-002 scc complexity scan

Layer: `independent-implementation`
Toolchain: `Complexity`
Status: `PASS`
Command: `scc .`
Exit code: `0`

```text
───────────────────────────────────────────────────────────────────────────────
Language            Files       Lines    Blanks  Comments       Code Complexity
───────────────────────────────────────────────────────────────────────────────
Rust                  265      99,003    10,080    14,988     73,935      3,591
Python                244      41,053     4,920     3,941     32,192      3,046
Markdown              194      40,622     9,386         0     31,236          0
TOML                   61       3,226       414     1,106      1,706          6
Plain Text             18         304        16         0        288          0
Shell                  18       2,277       280       315      1,682        240
Powershell             10       1,443       153       122      1,168        200
YAML                    9       2,457       156       193      2,108          0
Systemd                 5         197        19         8        170          0
JSON                    4       1,319         0         0      1,319          0
JavaScript              4         446        39        41        366         29
BASH                    3         232        39        36        157         36
Protocol Buffe…         2       1,171       170       256        745          0
Docker ignore           1          57        11        15         31          0
Dockerfile              1         167        16        99         52          5
INI                     1          31         4         6         21          0
License                 1          11         1         0         10          0
Windows Resour…         1           7         0         6          1          0
───────────────────────────────────────────────────────────────────────────────
Total                 842     194,023    25,704    21,132    147,187      7,153
───────────────────────────────────────────────────────────────────────────────
Estimated Cost to Develop (organic) $5,103,384
Estimated Schedule Effort (organic) 25.55 months
Estimated People Required (organic) 17.74
───────────────────────────────────────────────────────────────────────────────
Processed 7438793 bytes, 7.439 megabytes (SI)
───────────────────────────────────────────────────────────────────────────────
```

### IND-CPLX-003 radon complexity scan

Layer: `independent-implementation`
Toolchain: `Complexity`
Status: `PASS`
Command: `C:\Python314\python.exe -m radon cc .`
Exit code: `0`

```text
_init__ - A
    M 140:4 BatchAwareKvManager.__init__ - A
    M 149:4 BatchAwareKvManager.set_active_batch - A
tools\simulator\metrics.py
    M 65:4 MetricsCollector.report - B
    M 54:4 MetricsCollector.percentile - A
    C 7:0 MetricsCollector - A
    M 8:4 MetricsCollector.__init__ - A
    M 21:4 MetricsCollector.record_latency - A
    M 24:4 MetricsCollector.record_token_rate - A
    M 27:4 MetricsCollector.record_batch - A
    M 30:4 MetricsCollector.record_queue_depth - A
    M 33:4 MetricsCollector.record_eviction - A
    M 36:4 MetricsCollector.record_admission - A
    M 39:4 MetricsCollector.record_request - A
    M 42:4 MetricsCollector.record_completion - A
    M 45:4 MetricsCollector.record_thrash - A
    M 48:4 MetricsCollector.record_kv_utilization - A
    M 51:4 MetricsCollector.record_violation - A
    M 86:4 MetricsCollector.report_json - A
    M 89:4 MetricsCollector.reset - A
tools\simulator\replay_compare.py
    F 45:0 run_trace_replay - B
    F 136:0 compare_policies_on_trace - A
    F 163:0 main - A
tools\simulator\report.py
    F 39:0 render_markdown - B
    F 118:0 main - A
    F 33:0 _fmt - A
    F 84:0 find_best - A
    F 114:0 render_json - A
tools\simulator\trace_generator.py
    F 29:0 load_trace - A
    C 45:0 TraceGenerator - A
    M 72:4 TraceGenerator._compute_offsets - A
    M 92:4 TraceGenerator.generate - A
    F 23:0 _parse_timestamp - A
    M 59:4 TraceGenerator.__init__ - A
    M 102:4 TraceGenerator._materialize - A
    M 82:4 TraceGenerator.remaining - A
    M 86:4 TraceGenerator.total - A
    M 89:4 TraceGenerator.reset - A
tools\simulator\workload.py
    M 26:4 ChatWorkload.generate - B
    C 11:0 ChatWorkload - A
    C 80:0 MixedWorkload - A
    C 53:0 BatchWorkload - A
    M 81:4 MixedWorkload.__init__ - A
    M 88:4 MixedWorkload.generate - A
    C 7:0 WorkloadGenerator - A
    M 63:4 BatchWorkload.generate - A
    M 8:4 WorkloadGenerator.generate - A
    M 12:4 ChatWorkload.__init__ - A
    M 54:4 BatchWorkload.__init__ - A
tools\simulator\tests\test_replay_compare.py
    F 59:0 test_run_trace_replay_emits_required_fields - B
    F 120:0 test_report_markdown_includes_all_policies - A
    F 138:0 test_report_find_best_picks_max_throughput_and_min_p95 - A
    F 83:0 test_run_trace_replay_is_deterministic - A
    F 112:0 test_compare_policies_runs_all_when_unspecified - A
    F 20:0 _load - A
    F 39:0 _write_trace - A
    F 131:0 test_report_markdown_handles_empty_comparison - A
    F 35:0 _iso - A
    F 103:0 test_run_trace_replay_rejects_unknown_policy - A
tools\simulator\tests\test_simulator_extensions.py
    F 61:0 test_trace_generator_preserves_inter_request_gaps - B
    F 113:0 test_hybrid_emits_spike_during_window - B
    F 87:0 test_trace_generator_marks_continuations - A
    F 101:0 test_trace_generator_time_scale_compresses_timeline - A
    F 16:0 _load - A
    F 34:0 _write_trace - A
    F 55:0 _to_iso - A
    F 140:0 test_spike_config_validates - A
tools\smoke\smoke_client.py
    F 43:0 run - B
    F 28:0 get - A
    F 89:0 main - A
tools\trace-tools\anonymize.py
    F 46:0 anonymize_event - A
    F 64:0 process - A
    F 85:0 main - A
    F 58:0 validate - A
    F 39:0 rehash - A
tools\trace-tools\calibrate.py
    F 54:0 calibrate - B
    F 40:0 load_sessions - A
    F 106:0 main - A
    F 89:0 clamp - A
    F 93:0 render_toml - A
tools\trace-tools\reconstruct.py
    F 31:0 reconstruct - C
    F 69:0 process - A
    F 24:0 parse_timestamp - A
    F 89:0 main - A
tools\trace-tools\tests\test_trace_tools.py
    F 113:0 test_reconstruct_groups_events_by_session_and_computes_gaps - B
    F 71:0 test_anonymize_strips_disallowed_fields_and_rehashes - B
    F 146:0 test_calibrate_increases_alpha_with_repeat_traffic - B
    F 159:0 test_calibrate_render_toml_includes_coefficients - A
    F 139:0 test_calibrate_returns_defaults_for_empty_input - A
    F 20:0 _load - A
    F 34:0 _event - A
    F 64:0 _write_ndjson - A
    F 105:0 test_anonymize_validate_rejects_extra_fields - A
```
