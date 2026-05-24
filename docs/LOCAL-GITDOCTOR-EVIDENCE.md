# Local GitDoctor Evidence Package

Root: `C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai`

## Layer 1: Mapped Checks

Overall score: **55/100**
Checks: 58 total, 32 passed, 26 failed

This layer intentionally mirrors the Dougherty/GitDoctor finding families.

## Layer 2: Independent Implementations

These probes use mature local tools when installed. `SKIPPED` means the tool was not available or the project surface was absent; it is not a pass.

| Probe | Toolchain | Status | Title |
|---|---|---:|---|
| IND-RS-001 | Rust | PASS | cargo check workspace |
| IND-RS-002 | Rust | PASS | cargo clippy workspace |
| IND-RS-003 | Rust | PASS | cargo test workspace |
| IND-RS-004 | Rust | PASS | cargo audit |
| IND-RS-005 | Rust | PASS | cargo deny |
| IND-PY-001 | Python | PASS | pytest repository tests |
| IND-PY-002 | Python | PASS | ruff lint |
| IND-PY-003 | Python | PASS | bandit security scan |
| IND-PY-004 | Python | FAIL | pip-audit dependency scan |
| IND-SEC-001 | Secrets | PASS | gitleaks secret scan |
| IND-SEC-002 | Secrets | PASS | detect-secrets scan |
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
....                                                                     [100%]
4 passed in 0.36s
```

### IND-RS-001 cargo check workspace

Layer: `independent-implementation`
Toolchain: `Rust`
Status: `PASS`
Command: `cargo check --workspace`
Exit code: `0`

```text
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.34s
```

### IND-RS-002 cargo clippy workspace

Layer: `independent-implementation`
Toolchain: `Rust`
Status: `PASS`
Command: `cargo clippy --workspace -- -D warnings -A clippy::pedantic`
Exit code: `0`

```text
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.56s
```

### IND-RS-003 cargo test workspace

Layer: `independent-implementation`
Toolchain: `Rust`
Status: `PASS`
Command: `cargo test --workspace`
Exit code: `0`

```text
_pipeline_test-6010627a34f4d738.exe)
     Running tests\task_lifecycle_test.rs (target\debug\deps\task_lifecycle_test-875c45471aa65546.exe)
     Running tests\tool_calling_test.rs (target\debug\deps\tool_calling_test-12ce0b9f367b7538.exe)
     Running unittests src\lib.rs (target\debug\deps\mai_api-9b4beda981db52e2.exe)
     Running unittests src\main.rs (target\debug\deps\mai_api-d7ca5cb2dc168d19.exe)
     Running unittests src\bin\mai_ship_validate.rs (target\debug\deps\mai_ship_validate-b82f5a34f094b258.exe)
     Running tests\audit_wal.rs (target\debug\deps\audit_wal-d7c215707b11e239.exe)
     Running tests\auth_bypass_consistency.rs (target\debug\deps\auth_bypass_consistency-51e824e2cbcf1395.exe)
     Running tests\auth_gate_a.rs (target\debug\deps\auth_gate_a-bf27e036c5890f19.exe)
     Running tests\compliance_integration.rs (target\debug\deps\compliance_integration-6b035356379cd1ff.exe)
     Running tests\grpc_integration.rs (target\debug\deps\grpc_integration-9ffa632423d2ee85.exe)
     Running tests\http_integration.rs (target\debug\deps\http_integration-1ef8ed1c0e3864de.exe)
     Running tests\production_guard.rs (target\debug\deps\production_guard-403a429c41fbbfab.exe)
     Running tests\sealer_bootstrap.rs (target\debug\deps\sealer_bootstrap-e39cdd31014e3455.exe)
     Running tests\ship_07b_endpoints.rs (target\debug\deps\ship_07b_endpoints-e9f05c0c85c9caab.exe)
     Running tests\ship_11_observability.rs (target\debug\deps\ship_11_observability-52b4cf4fd3825c07.exe)
     Running tests\ship_convergence.rs (target\debug\deps\ship_convergence-7495bda579093444.exe)
     Running tests\ship_profile.rs (target\debug\deps\ship_profile-3a717207a4d07197.exe)
     Running tests\streaming_integration.rs (target\debug\deps\streaming_integration-90bb88d809d7cabe.exe)
     Running tests\system_integration.rs (target\debug\deps\system_integration-0fd03a66cd68b2fc.exe)
     Running tests\trust_production.rs (target\debug\deps\trust_production-4a1005b233250029.exe)
     Running tests\vault_bootstrap.rs (target\debug\deps\vault_bootstrap-2c1bc51ce836936f.exe)
     Running unittests src\lib.rs (target\debug\deps\mai_compliance-6b37f67c59a43cf9.exe)
     Running tests\compliance_demos.rs (target\debug\deps\compliance_demos-6077891ff57c9cf5.exe)
     Running tests\compliance_perf.rs (target\debug\deps\compliance_perf-591ad891fd863dc1.exe)
     Running tests\phi_perf.rs (target\debug\deps\phi_perf-9e80e56a42f54a16.exe)
     Running unittests src\lib.rs (target\debug\deps\mai_core-24dab66f41fd198d.exe)
     Running tests\integration_lifecycle.rs (target\debug\deps\integration_lifecycle-108e823103ccca70.exe)
     Running unittests src\lib.rs (target\debug\deps\mai_hil-441ca364441dae53.exe)
     Running tests\integration.rs (target\debug\deps\integration-976dddba66ea4867.exe)
     Running unittests src\main.rs (target\debug\deps\mai_pkg_builder-bb898097cd33c19b.exe)
     Running unittests src\lib.rs (target\debug\deps\mai_router-758b447630fe0560.exe)
     Running tests\baseline_policy_load.rs (target\debug\deps\baseline_policy_load-b470826a057a927d.exe)
     Running tests\latency_budget.rs (target\debug\deps\latency_budget-46769399d6f3f6a0.exe)
     Running unittests src\lib.rs (target\debug\deps\mai_scheduler-a62a3a77de511e95.exe)
     Running tests\gate_c_session33.rs (target\debug\deps\gate_c_session33-18169a2423c8a64a.exe)
     Running tests\topology_integration.rs (target\debug\deps\topology_integration-de26ca9010e05a98.exe)
     Running unittests src\lib.rs (target\debug\deps\mai_sdk_rs-fc74994f0d6bf62e.exe)
     Running unittests src\lib.rs (target\debug\deps\mai_vault-09eb82cc15d715b6.exe)
     Running unittests src\main.rs (target\debug\deps\rule_tester-c405b667d1612033.exe)
   Doc-tests mai_adapters
   Doc-tests mai_admin
   Doc-tests mai_agent
   Doc-tests mai_api
   Doc-tests mai_compliance
   Doc-tests mai_core
   Doc-tests mai_hil
   Doc-tests mai_router
   Doc-tests mai_scheduler
   Doc-tests mai_sdk_rs
   Doc-tests mai_vault
```

### IND-RS-004 cargo audit

Layer: `independent-implementation`
Toolchain: `Rust`
Status: `PASS`
Command: `cargo audit`
Exit code: `0`

```text
Fetching advisory database from `https://github.com/RustSec/advisory-db.git`
      Loaded 1098 security advisories (from C:\Users\17076\.cargo\advisory-db)
    Updating crates.io index
    Scanning Cargo.lock for vulnerabilities (390 crate dependencies)
```

### IND-RS-005 cargo deny

Layer: `independent-implementation`
Toolchain: `Rust`
Status: `PASS`
Command: `cargo deny check`
Exit code: `0`

```text
321 │ │ wit-bindgen 0.57.1 registry+https://github.com/rust-lang/crates.io-index
    │ ╰────────────────────────────────────────────────────────────────────────┘ lock entries
    │  
    ├ wit-bindgen v0.51.0
      └── wasip3 v0.4.0+wasi-0.3.0-rc-2026-01-06
          └── getrandom v0.4.2
              ├── tempfile v3.27.0
              │   ├── (dev) mai-admin v0.1.0
              │   ├── (dev) mai-api v0.1.0
              │   ├── (dev) mai-vault v0.1.0
              │   │   └── mai-api v0.1.0 (*)
              │   ├── native-tls v0.2.18
              │   │   ├── hyper-tls v0.6.0
              │   │   │   └── reqwest v0.12.28
              │   │   │       └── (dev) mai-api v0.1.0 (*)
              │   │   ├── reqwest v0.12.28 (*)
              │   │   └── tokio-native-tls v0.3.1
              │   │       ├── hyper-tls v0.6.0 (*)
              │   │       └── reqwest v0.12.28 (*)
              │   └── prost-build v0.13.5
              │       ├── (build) mai-api v0.1.0 (*)
              │       └── tonic-build v0.12.3
              │           └── (build) mai-api v0.1.0 (*)
              └── uuid v1.23.1
                  ├── mai-adapters v0.1.0
                  │   └── mai-api v0.1.0 (*)
                  ├── mai-agent v0.1.0
                  ├── mai-api v0.1.0 (*)
                  ├── mai-core v0.1.0
                  │   ├── mai-adapters v0.1.0 (*)
                  │   ├── mai-agent v0.1.0 (*)
                  │   ├── mai-api v0.1.0 (*)
                  │   ├── mai-compliance v0.1.0
                  │   │   └── mai-api v0.1.0 (*)
                  │   ├── mai-pkg-builder v0.1.0
                  │   ├── mai-scheduler v0.1.0
                  │   │   └── mai-api v0.1.0 (*)
                  │   └── mai-vault v0.1.0 (*)
                  ├── mai-scheduler v0.1.0 (*)
                  └── mai-vault v0.1.0 (*)
    ├ wit-bindgen v0.57.1
      └── wasip2 v1.0.3+wasi-0.2.9
          ├── getrandom v0.3.4
          │   └── rand_core v0.9.5
          │       ├── rand v0.9.4
          │       │   └── tungstenite v0.29.0
          │       │       └── tokio-tungstenite v0.29.0
          │       │           └── axum v0.8.9
          │       │               ├── axum-extra v0.10.3
          │       │               │   └── mai-api v0.1.0
          │       │               └── mai-api v0.1.0 (*)
          │       └── rand_chacha v0.9.0
          │           └── rand v0.9.4 (*)
          └── getrandom v0.4.2
              ├── tempfile v3.27.0
              │   ├── (dev) mai-admin v0.1.0
              │   ├── (dev) mai-api v0.1.0 (*)
              │   ├── (dev) mai-vault v0.1.0
              │   │   └── mai-api v0.1.0 (*)
              │   ├── native-tls v0.2.18
              │   │   ├── hyper-tls v0.6.0
              │   │   │   └── reqwest v0.12.28
              │   │   │       └── (dev) mai-api v0.1.0 (*)
              │   │   ├── reqwest v0.12.28 (*)
              │   │   └── tokio-native-tls v0.3.1
              │   │       ├── hyper-tls v0.6.0 (*)
              │   │       └── reqwest v0.12.28 (*)
              │   └── prost-build v0.13.5
              │       ├── (build) mai-api v0.1.0 (*)
              │       └── tonic-build v0.12.3
              │           └── (build) mai-api v0.1.0 (*)
              └── uuid v1.23.1
                  ├── mai-adapters v0.1.0
                  │   └── mai-api v0.1.0 (*)
                  ├── mai-agent v0.1.0
                  ├── mai-api v0.1.0 (*)
                  ├── mai-core v0.1.0
                  │   ├── mai-adapters v0.1.0 (*)
                  │   ├── mai-agent v0.1.0 (*)
                  │   ├── mai-api v0.1.0 (*)
                  │   ├── mai-compliance v0.1.0
                  │   │   └── mai-api v0.1.0 (*)
                  │   ├── mai-pkg-builder v0.1.0
                  │   ├── mai-scheduler v0.1.0
                  │   │   └── mai-api v0.1.0 (*)
                  │   └── mai-vault v0.1.0 (*)
                  ├── mai-scheduler v0.1.0 (*)
                  └── mai-vault v0.1.0 (*)
```

### IND-PY-001 pytest repository tests

Layer: `independent-implementation`
Toolchain: `Python`
Status: `PASS`
Command: `C:\Python314\python.exe -m pytest -q --ignore=target --ignore=results`
Exit code: `0`

```text
...................................................ssssss............... [ 10%]
.....ssssss............................................................. [ 20%]
........................................................................ [ 30%]
........................................................................ [ 40%]
........................................................................ [ 50%]
..................................................s......s.............. [ 60%]
....ssssssssssss.sssssssss.............................................. [ 70%]
........................................................................ [ 80%]
........................................................................ [ 90%]
........................................................................ [100%]
685 passed, 35 skipped in 43.23s
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
      "loc": 20907,
      "nosec": 0,
      "skipped_tests": 2
    }
  },
  "results": []
}
[main]	INFO	profile include tests: None
[main]	INFO	profile exclude tests: B603,B105,B404,B310,B311,B101,B607
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
Command: `C:\Python314\python.exe -m pip_audit`
Exit code: `1`

```text
Name Version ID            Fix Versions
---- ------- ------------- ------------
pip  26.0.1  CVE-2026-3219 26.1
pip  26.0.1  CVE-2026-6357 26.1
Name    Skip Reason
------- ----------------------------------------------------------------------
mai-sdk Dependency not found on PyPI and could not be audited: mai-sdk (0.2.0)

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

[90m2:02PM[0m [32mINF[0m [1mscanned ~9941088 bytes (9.94 MB) in 547ms[0m
[90m2:02PM[0m [32mINF[0m [1mno leaks found[0m
```

### IND-SEC-002 detect-secrets scan

Layer: `independent-implementation`
Toolchain: `Secrets`
Status: `PASS`
Command: `detect-secrets scan --all-files`
Exit code: `0`

```text
06475255a18c4744d4a1fa9c23a4470841",
        "is_verified": false,
        "line_number": 62
      }
    ],
    "scripts\\build-package.ps1": [
      {
        "type": "Base64 High Entropy String",
        "filename": "scripts\\build-package.ps1",
        "hashed_secret": "20a14ede53d8f5ab25cbbea16357ffdd887e101d",
        "is_verified": false,
        "line_number": 138
      }
    ],
    "target\\CACHEDIR.TAG": [
      {
        "type": "Hex High Entropy String",
        "filename": "target\\CACHEDIR.TAG",
        "hashed_secret": "e8f8c345877b2411a59897798e422b15b0c16d76",
        "is_verified": false,
        "line_number": 1
      }
    ],
    "target\\debug\\.fingerprint\\pqcrypto-internals-0fa37d9e1e6f58ef\\run-build-script-build-script-build.json": [
      {
        "type": "Base64 High Entropy String",
        "filename": "target\\debug\\.fingerprint\\pqcrypto-internals-0fa37d9e1e6f58ef\\run-build-script-build-script-build.json",
        "hashed_secret": "aa78dc17c565e61687d49ba560f1cc80a90bc872",
        "is_verified": false,
        "line_number": 1
      }
    ],
    "target\\release\\.fingerprint\\getrandom-143fd6584b092c1f\\run-build-script-build-script-build.json": [
      {
        "type": "Base64 High Entropy String",
        "filename": "target\\release\\.fingerprint\\getrandom-143fd6584b092c1f\\run-build-script-build-script-build.json",
        "hashed_secret": "ca89dcdbaf768810854a9e840c0bdde540b0333d",
        "is_verified": false,
        "line_number": 1
      }
    ],
    "target\\release\\.fingerprint\\parking_lot_core-d9f114c9f73c887e\\run-build-script-build-script-build.json": [
      {
        "type": "Base64 High Entropy String",
        "filename": "target\\release\\.fingerprint\\parking_lot_core-d9f114c9f73c887e\\run-build-script-build-script-build.json",
        "hashed_secret": "12e5eb241db7c070750e8e8fd9a980413abac9c0",
        "is_verified": false,
        "line_number": 1
      }
    ],
    "target\\sdk-config-validation.toml": [
      {
        "type": "Secret Keyword",
        "filename": "target\\sdk-config-validation.toml",
        "hashed_secret": "3acfb2c2b433c0ea7ff107e33df91b18e52f960f",
        "is_verified": false,
        "line_number": 2
      }
    ],
    "test-evidence\\rc-06\\bundle-first-boot-stdout.log": [
      {
        "type": "Hex High Entropy String",
        "filename": "test-evidence\\rc-06\\bundle-first-boot-stdout.log",
        "hashed_secret": "3b3320fddd54ca6fc2d81fd25e74f71e63f0d49f",
        "is_verified": false,
        "line_number": 22
      }
    ],
    "tests\\sdk_integration.py": [
      {
        "type": "Secret Keyword",
        "filename": "tests\\sdk_integration.py",
        "hashed_secret": "fb0b56ad02475c3b749709ebb14436d12270e1eb",
        "is_verified": false,
        "line_number": 71
      }
    ],
    "tools\\gpu_release_tests\\test_bundle_scripts.py": [
      {
        "type": "Hex High Entropy String",
        "filename": "tools\\gpu_release_tests\\test_bundle_scripts.py",
        "hashed_secret": "26019c2e7b54c3d5b828190796fa49f2ae4b1a43",
        "is_verified": false,
        "line_number": 97
      },
      {
        "type": "Hex High Entropy String",
        "filename": "tools\\gpu_release_tests\\test_bundle_scripts.py",
        "hashed_secret": "158b484ae1f6f64f89da22397d25fbdafad02252",
        "is_verified": false,
        "line_number": 125
      },
      {
        "type": "Hex High Entropy String",
        "filename": "tools\\gpu_release_tests\\test_bundle_scripts.py",
        "hashed_secret": "ff998abc1ce6d8f01a675fa197368e44c8916e9c",
        "is_verified": false,
        "line_number": 184
      }
    ],
    "tools\\mai-admin\\src\\audit.rs": [
      {
        "type": "Hex High Entropy String",
        "filename": "tools\\mai-admin\\src\\audit.rs",
        "hashed_secret": "c0174d8dfe9687a8f29297449712d6ba12ed2bc3",
        "is_verified": false,
        "line_number": 19
      }
    ]
  },
  "generated_at": "2026-05-24T21:04:37Z"
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
 Dockerfile                1          170           52          102           16
 JSON                      2         1302         1302            0            0
 PowerShell                7         1174          959           91          124
 Protocol Buffers          2         1171          745          256          170
 Python                  171        26250        21225          719         4306
 Shell                    13         1901         1428          249          224
 Plain Text               17          271            0          269            2
 TOML                     57         2977         1605          987          385
 YAML                      2         1660         1539           53           68
─────────────────────────────────────────────────────────────────────────────────
 Markdown                133        31201            0        23643         7558
 |- BASH                  38          417          353           42           22
 |- HCL                    1            6            5            1            0
 |- JSON                  24          907          902            0            5
 |- PowerShell            21          164          117           27           20
 |- Python                13          244          199           14           31
 |- Rust                   9          278          211           57           10
 |- TOML                   7          203          177            2           24
 |- YAML                   1            2            2            0            0
 (Total)                            33422         1966        23786         7670
─────────────────────────────────────────────────────────────────────────────────
 Rust                    247        81788        68954         3261         9573
 |- Markdown             246        11234            8        10162         1064
 (Total)                            93022        68962        13423        10637
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Total                   652       163320        99783        39935        23602
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
Rust                  247      93,061     9,552    14,479     69,030      3,337
Python                171      26,250     3,206     2,544     20,500      1,699
Markdown              139      34,025     7,800         0     26,225          0
TOML                   59       3,090       398     1,063      1,629          6
Plain Text             17         271         2         0        269          0
Shell                  17       2,190       271       287      1,632        230
Powershell              7       1,174       110        83        981        173
YAML                    6       2,307       137       155      2,015          0
Systemd                 5         197        19         8        170          0
JSON                    4       1,327         0         0      1,327          0
BASH                    2         174        30        20        124         30
Protocol Buffe…         2       1,171       170       256        745          0
Docker ignore           1          57        11        15         31          0
Dockerfile              1         170        16       102         52          5
JavaScript              1         372        32        26        314         25
License                 1          11         1         0         10          0
───────────────────────────────────────────────────────────────────────────────
Total                 680     165,847    21,755    19,038    125,054      5,505
───────────────────────────────────────────────────────────────────────────────
Estimated Cost to Develop (organic) $4,300,785
Estimated Schedule Effort (organic) 23.94 months
Estimated People Required (organic) 15.96
───────────────────────────────────────────────────────────────────────────────
Processed 6320135 bytes, 6.320 megabytes (SI)
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
