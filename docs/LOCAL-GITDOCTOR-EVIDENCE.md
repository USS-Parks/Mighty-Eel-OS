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
| IND-RS-004 | Rust | FAIL | cargo audit |
| IND-RS-005 | Rust | FAIL | cargo deny |
| IND-PY-001 | Python | FAIL | pytest repository tests |
| IND-PY-002 | Python | FAIL | ruff lint |
| IND-PY-003 | Python | FAIL | bandit security scan |
| IND-PY-004 | Python | FAIL | pip-audit dependency scan |
| IND-SEC-001 | Secrets | FAIL | gitleaks secret scan |
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
4 passed in 0.50s
```

### IND-RS-001 cargo check workspace

Layer: `independent-implementation`
Toolchain: `Rust`
Status: `PASS`
Command: `cargo check --workspace`
Exit code: `0`

```text
Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.02s
```

### IND-RS-002 cargo clippy workspace

Layer: `independent-implementation`
Toolchain: `Rust`
Status: `PASS`
Command: `cargo clippy --workspace -- -D warnings -A clippy::pedantic`
Exit code: `0`

```text
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.75s
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
Status: `FAIL`
Command: `cargo audit`
Exit code: `1`

```text
Crate:     ml-dsa
Version:   0.0.4
Title:     Timing side-channel in ML-DSA decomposition
Date:      2025-12-12
ID:        RUSTSEC-2025-0144
URL:       https://github.com/RustCrypto/signatures/security/advisories/GHSA-hcp2-x6j4-29j7
Severity:  6.4 (medium)
Solution:  Upgrade to >=0.1.0-rc.3
Dependency tree:
ml-dsa 0.0.4
├── mai-vault 0.1.0
│   └── mai-api 0.1.0
├── mai-compliance 0.1.0
│   └── mai-api 0.1.0
├── mai-api 0.1.0
└── mai-admin 0.1.0

Crate:     instant
Version:   0.1.13
Warning:   unmaintained
Title:     `instant` is unmaintained
Date:      2024-09-01
ID:        RUSTSEC-2024-0384
URL:       https://rustsec.org/advisories/RUSTSEC-2024-0384
Dependency tree:
instant 0.1.13
└── notify-types 1.0.1
    └── notify 7.0.0
        └── mai-api 0.1.0

Crate:     paste
Version:   1.0.15
Warning:   unmaintained
Title:     paste - no longer maintained
Date:      2024-10-07
ID:        RUSTSEC-2024-0436
URL:       https://rustsec.org/advisories/RUSTSEC-2024-0436
Dependency tree:
paste 1.0.15
└── pqcrypto-mldsa 0.1.2
    └── mai-vault 0.1.0
        └── mai-api 0.1.0


    Fetching advisory database from `https://github.com/RustSec/advisory-db.git`
      Loaded 1098 security advisories (from C:\Users\17076\.cargo\advisory-db)
    Updating crates.io index
    Scanning Cargo.lock for vulnerabilities (390 crate dependencies)
error: 1 vulnerability found!
warning: 2 allowed warnings found
```

### IND-RS-005 cargo deny

Layer: `independent-implementation`
Toolchain: `Rust`
Status: `FAIL`
Command: `cargo deny check`
Exit code: `5`

```text
         │   ├── mai-compliance v0.1.0 (*)
         │   ├── mai-pkg-builder v0.1.0
         │   ├── mai-scheduler v0.1.0
         │   │   └── mai-api v0.1.0 (*)
         │   └── mai-vault v0.1.0
         │       └── mai-api v0.1.0 (*)
         ├── mai-hil v0.1.0
         │   ├── mai-adapters v0.1.0 (*)
         │   ├── mai-api v0.1.0 (*)
         │   └── mai-core v0.1.0 (*)
         ├── mai-pkg-builder v0.1.0 (*)
         ├── mai-router v0.1.0
         │   └── rule-tester v0.1.0
         ├── mai-scheduler v0.1.0 (*)
         ├── mai-sdk-rs v0.1.0
         ├── mai-vault v0.1.0 (*)
         ├── reqwest v0.12.28
         │   └── (dev) mai-api v0.1.0 (*)
         └── tracing-subscriber v0.3.23
             ├── mai-admin v0.1.0 (*)
             ├── mai-api v0.1.0 (*)
             └── mai-pkg-builder v0.1.0 (*)

error[unmaintained]: `instant` is unmaintained
    ┌─ C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai/Cargo.lock:123:1
    │
123 │ instant 0.1.13 registry+https://github.com/rust-lang/crates.io-index
    │ ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ unmaintained advisory detected
    │
    ├ ID: RUSTSEC-2024-0384
    ├ Advisory: https://rustsec.org/advisories/RUSTSEC-2024-0384
    ├ This crate is no longer maintained, and the author recommends using the maintained [`web-time`] crate instead.
      
      [`web-time`]: https://crates.io/crates/web-time
    ├ Solution: No safe upgrade is available!
    ├ instant v0.1.13
      └── notify-types v1.0.1
          └── notify v7.0.0
              └── mai-api v0.1.0

error[vulnerability]: Timing side-channel in ML-DSA decomposition
    ┌─ C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai/Cargo.lock:157:1
    │
157 │ ml-dsa 0.0.4 registry+https://github.com/rust-lang/crates.io-index
    │ ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ security vulnerability detected
    │
    ├ ID: RUSTSEC-2025-0144
    ├ Advisory: https://rustsec.org/advisories/RUSTSEC-2025-0144
    ├ ### Summary
      
      A timing side-channel was discovered in the Decompose algorithm which is used during ML-DSA signing to generate hints for the signature.
      
      ### Details
      
      The analysis was performed using a constant-time analyzer that examines compiled assembly code for instructions with data-dependent timing behavior. The analyzer flags:
      
      - **UDIV/SDIV instructions**: Hardware division instructions have early termination optimizations where execution time depends on operand values.
      
      The `decompose` function used a hardware division instruction to compute `r1.0 / TwoGamma2::U32`. This function is called during signing through `high_bits()` and `low_bits()`, which process values derived from secret key components:
      
      - `(&w - &cs2).low_bits()` where `cs2` is derived from secret key component `s2`
      - `Hint::new()` calls `high_bits()` on values derived from secret key component `t0`
      
      **Original Code**:
      ```rust
      fn decompose<TwoGamma2: Unsigned>(self) -> (Elem, Elem) {
          // ...
          let mut r1 = r_plus - r0;
          r1.0 /= TwoGamma2::U32;  // Variable-time division on secret-derived data
          (r1, r0)
      }
      ```
      
      ### Impact
      
      The dividend (`r1.0`) is derived from secret key material. An attacker with precise timing measurements could extract information about the signing key by observing timing variations in the division operation.
      
      ### Mitigation
      
      Integer division was replaced with a constant-time Barrett reduction.
    ├ Announcement: https://github.com/RustCrypto/signatures/security/advisories/GHSA-hcp2-x6j4-29j7
    ├ Solution: Upgrade to >=0.1.0-rc.3 (try `cargo update -p ml-dsa`)
    ├ ml-dsa v0.0.4
      ├── mai-admin v0.1.0
      ├── (dev) mai-api v0.1.0
      ├── mai-compliance v0.1.0
      │   └── mai-api v0.1.0 (*)
      └── mai-vault v0.1.0
          └── mai-api v0.1.0 (*)
```

### IND-PY-001 pytest repository tests

Layer: `independent-implementation`
Toolchain: `Python`
Status: `FAIL`
Command: `C:\Python314\python.exe -m pytest -q --ignore=target --ignore=results`
Exit code: `2`

```text
ule
    return _bootstrap._gcd_import(name[level:], package, level)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
E   ModuleNotFoundError: No module named 'tests.test_config'
____________ ERROR collecting mai-sdk-python/tests/test_errors.py _____________
ImportError while importing test module 'C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai\mai-sdk-python\tests\test_errors.py'.
Hint: make sure your test modules/packages have valid Python names.
Traceback:
C:\Python314\Lib\importlib\__init__.py:88: in import_module
    return _bootstrap._gcd_import(name[level:], package, level)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
E   ModuleNotFoundError: No module named 'tests.test_errors'
_____________ ERROR collecting mai-sdk-python/tests/test_retry.py _____________
ImportError while importing test module 'C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai\mai-sdk-python\tests\test_retry.py'.
Hint: make sure your test modules/packages have valid Python names.
Traceback:
C:\Python314\Lib\importlib\__init__.py:88: in import_module
    return _bootstrap._gcd_import(name[level:], package, level)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
E   ModuleNotFoundError: No module named 'tests.test_retry'
____________ ERROR collecting mai-sdk-python/tests/test_version.py ____________
ImportError while importing test module 'C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai\mai-sdk-python\tests\test_version.py'.
Hint: make sure your test modules/packages have valid Python names.
Traceback:
C:\Python314\Lib\importlib\__init__.py:88: in import_module
    return _bootstrap._gcd_import(name[level:], package, level)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
E   ModuleNotFoundError: No module named 'tests.test_version'
_____________ ERROR collecting tests/e2e/test_compliance_smoke.py _____________
ImportError while importing test module 'C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai\tests\e2e\test_compliance_smoke.py'.
Hint: make sure your test modules/packages have valid Python names.
Traceback:
C:\Python314\Lib\importlib\__init__.py:88: in import_module
    return _bootstrap._gcd_import(name[level:], package, level)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
E   ModuleNotFoundError: No module named 'tests.e2e'
___________ ERROR collecting tests/integrity/test_assertion_gate.py ___________
ImportError while importing test module 'C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai\tests\integrity\test_assertion_gate.py'.
Hint: make sure your test modules/packages have valid Python names.
Traceback:
C:\Python314\Lib\importlib\__init__.py:88: in import_module
    return _bootstrap._gcd_import(name[level:], package, level)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
E   ModuleNotFoundError: No module named 'tests.integrity'
=========================== short test summary info ===========================
ERROR apps/local-secure-inference/tests/test_integration.py
ERROR apps/local-secure-inference/tests/test_smoke.py
ERROR apps/openbao-trust-demo/tests/test_integration.py
ERROR apps/openbao-trust-demo/tests/test_smoke.py
ERROR apps/rag-reference/tests/test_integration.py
ERROR apps/rag-reference/tests/test_smoke.py
ERROR apps/tribal-sovereignty/tests/test_integration.py
ERROR apps/tribal-sovereignty/tests/test_smoke.py
ERROR compliance-dashboard/tests/test_dashboard.py
ERROR mai-sdk-python/tests/test_async_client_methods.py
ERROR mai-sdk-python/tests/test_cli.py
ERROR mai-sdk-python/tests/test_client_methods.py
ERROR mai-sdk-python/tests/test_config.py
ERROR mai-sdk-python/tests/test_errors.py
ERROR mai-sdk-python/tests/test_retry.py
ERROR mai-sdk-python/tests/test_version.py
ERROR tests/e2e/test_compliance_smoke.py
ERROR tests/integrity/test_assertion_gate.py
!!!!!!!!!!!!!!!!!! Interrupted: 18 errors during collection !!!!!!!!!!!!!!!!!!!
18 errors in 1.62s
```

### IND-PY-002 ruff lint

Layer: `independent-implementation`
Toolchain: `Python`
Status: `FAIL`
Command: `C:\Python314\python.exe -m ruff check .`
Exit code: `1`

```text
  |
help: Remove unnecessary `encoding` argument

RET504 Unnecessary assignment to `digest` before `return` statement
  --> tools\trace-tools\anonymize.py:45:12
   |
43 |         f"{salt}|{value}".encode("utf-8"), digest_size=16
44 |     ).hexdigest()
45 |     return digest
   |            ^^^^^^
   |
help: Remove unnecessary assignment

PLW2901 `for` loop variable `line` overwritten by assignment target
  --> tools\trace-tools\anonymize.py:72:13
   |
70 |     ) as dst:
71 |         for line_no, line in enumerate(src, start=1):
72 |             line = line.strip()
   |             ^^^^
73 |             if not line:
74 |                 continue
   |

PLW2901 `for` loop variable `line` overwritten by assignment target
  --> tools\trace-tools\calibrate.py:44:13
   |
42 |     with path.open("r", encoding="utf-8") as src:
43 |         for line_no, line in enumerate(src, start=1):
44 |             line = line.strip()
   |             ^^^^
45 |             if not line:
46 |                 continue
   |

B905 `zip()` without an explicit `strict=` parameter
  --> tools\trace-tools\reconstruct.py:43:40
   |
41 |         items.sort(key=lambda ev: ev.get("timestamp", ""))
42 |         times = [parse_timestamp(ev["timestamp"]) for ev in items]
43 |         gaps_secs = [b - a for a, b in zip(times, times[1:])] if len(times) > 1 else []
   |                                        ^^^^^^^^^^^^^^^^^^^^^
44 |         total_input = sum(int(ev.get("input_tokens", 0)) for ev in items)
45 |         total_output = sum(int(ev.get("output_tokens", 0)) for ev in items)
   |
help: Add explicit value for parameter `strict=`

RUF007 Prefer `itertools.pairwise()` over `zip()` when iterating over successive pairs
  --> tools\trace-tools\reconstruct.py:43:40
   |
41 |         items.sort(key=lambda ev: ev.get("timestamp", ""))
42 |         times = [parse_timestamp(ev["timestamp"]) for ev in items]
43 |         gaps_secs = [b - a for a, b in zip(times, times[1:])] if len(times) > 1 else []
   |                                        ^^^
44 |         total_input = sum(int(ev.get("input_tokens", 0)) for ev in items)
45 |         total_output = sum(int(ev.get("output_tokens", 0)) for ev in items)
   |
help: Replace `zip()` with `itertools.pairwise()`

PLW2901 `for` loop variable `line` overwritten by assignment target
  --> tools\trace-tools\reconstruct.py:72:13
   |
70 |     with input_path.open("r", encoding="utf-8") as src:
71 |         for line_no, line in enumerate(src, start=1):
72 |             line = line.strip()
   |             ^^^^
73 |             if not line:
74 |                 continue
   |

ANN202 Missing return type annotation for private function `_load`
  --> tools\trace-tools\tests\test_trace_tools.py:20:5
   |
20 | def _load(name: str):
   |     ^^^^^
21 |     spec = importlib.util.spec_from_file_location(name, TRACE_TOOLS / f"{name}.py")
22 |     assert spec and spec.loader, f"could not load {name}"
   |
help: Add return type annotation

PT018 Assertion should be broken down into multiple parts
  --> tools\trace-tools\tests\test_trace_tools.py:22:5
   |
20 | def _load(name: str):
21 |     spec = importlib.util.spec_from_file_location(name, TRACE_TOOLS / f"{name}.py")
22 |     assert spec and spec.loader, f"could not load {name}"
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
23 |     module = importlib.util.module_from_spec(spec)
24 |     sys.modules[name] = module
   |
help: Break down assertion into multiple parts

PT011 `pytest.raises(ValueError)` is too broad, set the `match` parameter or use a more specific exception
   --> tools\trace-tools\tests\test_trace_tools.py:109:24
    |
108 |     bad = {"timestamp": "x", "request_id": "y", "session_id_hash": "z", "leaked": True}
109 |     with pytest.raises(ValueError):
    |                        ^^^^^^^^^^
110 |         anonymize.validate(bad)
    |

Found 402 errors.
[*] 59 fixable with the `--fix` option (39 hidden fixes can be enabled with the `--unsafe-fixes` option).
```

### IND-PY-003 bandit security scan

Layer: `independent-implementation`
Toolchain: `Python`
Status: `FAIL`
Command: `C:\Python314\python.exe -m bandit -r .`
Exit code: `1`

```text
Working... ---------------------------------------- 100% 0:00:02

[main]	INFO	profile include tests: None
[main]	INFO	profile exclude tests: None
[main]	INFO	cli include tests: None
[main]	INFO	cli exclude tests: None
[main]	INFO	running on Python 3.14.4
Traceback (most recent call last):
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\bandit\core\manager.py", line 186, in output_results
    report_func(
    ~~~~~~~~~~~^
        self,
        ^^^^^
    ...<3 lines>...
        lines=lines,
        ^^^^^^^^^^^^
    )
    ^
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\bandit\formatters\text.py", line 197, in report
    wrapped_file.write(result)
    ~~~~~~~~~~~~~~~~~~^^^^^^^^
  File "C:\Python314\Lib\encodings\cp1252.py", line 19, in encode
    return codecs.charmap_encode(input,self.errors,encoding_table)[0]
           ~~~~~~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
UnicodeEncodeError: 'charmap' codec can't encode character '\u2192' in position 152391: character maps to <undefined>

During handling of the above exception, another exception occurred:

Traceback (most recent call last):
  File "<frozen runpy>", line 198, in _run_module_as_main
  File "<frozen runpy>", line 88, in _run_code
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\bandit\__main__.py", line 17, in <module>
    main.main()
    ~~~~~~~~~^^
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\bandit\cli\main.py", line 682, in main
    b_mgr.output_results(
    ~~~~~~~~~~~~~~~~~~~~^
        args.context_lines,
        ^^^^^^^^^^^^^^^^^^^
    ...<4 lines>...
        args.msg_template,
        ^^^^^^^^^^^^^^^^^^
    )
    ^
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\bandit\core\manager.py", line 195, in output_results
    raise RuntimeError(
    ...<2 lines>...
    )
RuntimeError: Unable to output report using 'txt' formatter: 'charmap' codec can't encode character '\u2192' in position 152391: character maps to <undefined>
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
Status: `FAIL`
Command: `gitleaks detect --source . --no-git --redact`
Exit code: `1`

```text
○
    │╲
    │ ○
    ○ ░
    ░    gitleaks

[90m12:28PM[0m [32mINF[0m [1mscanned ~21929429122 bytes (21.93 GB) in 4m59s[0m
[90m12:28PM[0m [33mWRN[0m [1mleaks found: 20[0m
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
        "line_number": 72
      }
    ],
    "tools\\gpu_release_tests\\test_bundle_scripts.py": [
      {
        "type": "Hex High Entropy String",
        "filename": "tools\\gpu_release_tests\\test_bundle_scripts.py",
        "hashed_secret": "26019c2e7b54c3d5b828190796fa49f2ae4b1a43",
        "is_verified": false,
        "line_number": 98
      },
      {
        "type": "Hex High Entropy String",
        "filename": "tools\\gpu_release_tests\\test_bundle_scripts.py",
        "hashed_secret": "158b484ae1f6f64f89da22397d25fbdafad02252",
        "is_verified": false,
        "line_number": 126
      },
      {
        "type": "Hex High Entropy String",
        "filename": "tools\\gpu_release_tests\\test_bundle_scripts.py",
        "hashed_secret": "ff998abc1ce6d8f01a675fa197368e44c8916e9c",
        "is_verified": false,
        "line_number": 185
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
  "generated_at": "2026-05-24T19:30:03Z"
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
 JSON                      2         1298         1298            0            0
 PowerShell                7         1174          959           91          124
 Protocol Buffers          2         1171          745          256          170
 Python                  171        26248        21216          709         4323
 Shell                    13         1901         1428          249          224
 Plain Text               17          271            0          269            2
 TOML                     56         2778         1533          873          372
 YAML                      2         1660         1539           53           68
─────────────────────────────────────────────────────────────────────────────────
 Markdown                132        31046            0        23537         7509
 |- BASH                  38          417          353           42           22
 |- HCL                    1            6            5            1            0
 |- JSON                  24          907          902            0            5
 |- PowerShell            21          163          116           27           20
 |- Python                13          244          199           14           31
 |- Rust                   9          278          211           57           10
 |- TOML                   7          203          177            2           24
 |- YAML                   1            2            2            0            0
 (Total)                            33266         1965        23680         7621
─────────────────────────────────────────────────────────────────────────────────
 Rust                    247        81788        68954         3261         9573
 |- Markdown             246        11234            8        10162         1064
 (Total)                            93022        68962        13423        10637
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Total                   650       162959        99697        39705        23557
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
Python                171      26,248     3,222     2,529     20,497      1,729
Markdown              138      33,863     7,750         0     26,113          0
TOML                   56       2,778       372       873      1,533          5
Plain Text             17         271         2         0        269          0
Shell                  17       2,190       271       287      1,632        230
Powershell              7       1,174       110        83        981        173
YAML                    6       2,307       137       155      2,015          0
Systemd                 5         197        19         8        170          0
JSON                    4       1,323         0         0      1,323          0
BASH                    2         174        30        20        124         30
Protocol Buffe…         2       1,171       170       256        745          0
Docker ignore           1          57        11        15         31          0
Dockerfile              1         170        16       102         52          5
JavaScript              1         372        32        26        314         25
License                 1          11         1         0         10          0
───────────────────────────────────────────────────────────────────────────────
Total                 676     165,367    21,695    18,833    124,839      5,534
───────────────────────────────────────────────────────────────────────────────
Estimated Cost to Develop (organic) $4,293,022
Estimated Schedule Effort (organic) 23.93 months
Estimated People Required (organic) 15.94
───────────────────────────────────────────────────────────────────────────────
Processed 6288781 bytes, 6.289 megabytes (SI)
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
    M 139:4 BatchAwareKvManager.__init__ - A
    M 148:4 BatchAwareKvManager.set_active_batch - A
tools\simulator\metrics.py
    M 64:4 MetricsCollector.report - B
    M 53:4 MetricsCollector.percentile - A
    C 6:0 MetricsCollector - A
    M 7:4 MetricsCollector.__init__ - A
    M 20:4 MetricsCollector.record_latency - A
    M 23:4 MetricsCollector.record_token_rate - A
    M 26:4 MetricsCollector.record_batch - A
    M 29:4 MetricsCollector.record_queue_depth - A
    M 32:4 MetricsCollector.record_eviction - A
    M 35:4 MetricsCollector.record_admission - A
    M 38:4 MetricsCollector.record_request - A
    M 41:4 MetricsCollector.record_completion - A
    M 44:4 MetricsCollector.record_thrash - A
    M 47:4 MetricsCollector.record_kv_utilization - A
    M 50:4 MetricsCollector.record_violation - A
    M 85:4 MetricsCollector.report_json - A
    M 88:4 MetricsCollector.reset - A
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
    M 25:4 ChatWorkload.generate - B
    C 10:0 ChatWorkload - A
    C 79:0 MixedWorkload - A
    C 52:0 BatchWorkload - A
    M 80:4 MixedWorkload.__init__ - A
    M 87:4 MixedWorkload.generate - A
    C 6:0 WorkloadGenerator - A
    M 62:4 BatchWorkload.generate - A
    M 7:4 WorkloadGenerator.generate - A
    M 11:4 ChatWorkload.__init__ - A
    M 53:4 BatchWorkload.__init__ - A
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
    F 60:0 test_trace_generator_preserves_inter_request_gaps - B
    F 112:0 test_hybrid_emits_spike_during_window - B
    F 86:0 test_trace_generator_marks_continuations - A
    F 100:0 test_trace_generator_time_scale_compresses_timeline - A
    F 15:0 _load - A
    F 33:0 _write_trace - A
    F 54:0 _to_iso - A
    F 139:0 test_spike_config_validates - A
tools\smoke\smoke_client.py
    F 43:0 run - B
    F 28:0 get - A
    F 89:0 main - A
tools\trace-tools\anonymize.py
    F 48:0 anonymize_event - A
    F 66:0 process - A
    F 87:0 main - A
    F 60:0 validate - A
    F 40:0 rehash - A
tools\trace-tools\calibrate.py
    F 54:0 calibrate - B
    F 40:0 load_sessions - A
    F 106:0 main - A
    F 89:0 clamp - A
    F 93:0 render_toml - A
tools\trace-tools\reconstruct.py
    F 30:0 reconstruct - C
    F 68:0 process - A
    F 23:0 parse_timestamp - A
    F 88:0 main - A
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
