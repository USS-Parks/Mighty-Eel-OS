# Local GitDoctor Evidence Package

Root: `C:\Users\17076\Documents\Claude\Island Mountain Mighty Eel OS\mai`

## Layer 1: Mapped Checks

Overall score: **59/100**
Checks: 58 total, 34 passed, 24 failed

This layer intentionally mirrors the Dougherty/GitDoctor finding families.

## Layer 2: Independent Implementations

These probes use mature local tools when installed. `SKIPPED` means the tool was not available or the project surface was absent; it is not a pass.

| Probe | Toolchain | Status | Title |
|---|---|---:|---|
| IND-RS-001 | Rust | PASS | cargo check workspace |
| IND-RS-002 | Rust | PASS | cargo clippy workspace |
| IND-RS-003 | Rust | PASS | cargo test workspace |
| IND-RS-004 | Rust | FAIL | cargo audit |
| IND-RS-005 | Rust | PASS | cargo deny |
| IND-PY-001 | Python | FAIL | pytest repository tests |
| IND-PY-002 | Python | PASS | ruff lint |
| IND-PY-003 | Python | PASS | bandit security scan |
| IND-PY-004 | Python | FAIL | pip-audit dependency scan |
| IND-SEC-001 | Secrets | PASS | gitleaks secret scan |
| IND-SEC-002 | Secrets | FAIL | detect-secrets scan |
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
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.46s
```

### IND-RS-002 cargo clippy workspace

Layer: `independent-implementation`
Toolchain: `Rust`
Status: `PASS`
Command: `cargo clippy --workspace -- -D warnings -A clippy::pedantic`
Exit code: `0`

```text
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.80s
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
Fetching advisory database from `https://github.com/RustSec/advisory-db.git`
error: couldn't fetch advisory database: git operation failed: failed to prepare fetch
Caused by:
  -> An IO error occurred when talking to the server
  -> error sending request for url (https://github.com/RustSec/advisory-db.git/info/refs?service=git-upload-pack)
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
Status: `FAIL`
Command: `C:\Python314\python.exe -m pytest -q --ignore=target --ignore=results`
Exit code: `1`

```text
l\\Temp\\mai-e2e-3k0odkvh'
topdown = False
onerror = <function _rmtree_unsafe.<locals>.onerror at 0x00000292596794E0>
followlinks = <object object at 0x0000029252C10210>

>   ???
E   PermissionError: [WinError 5] Access is denied: 'C:\\Users\\17076\\AppData\\Local\\Temp\\mai-e2e-3k0odkvh'

<frozen os>:377: PermissionError

During handling of the above exception, another exception occurred:

    @pytest.fixture(scope="module")
    def running_server() -> Iterator[int]:
        """Spawn mai-api in a temp working directory; yield the REST port."""
        binary = _find_binary()
        if binary is None:
            pytest.skip(
                "mai-api binary not built. Run "
                "`cargo build --release -p mai-api` before invoking this e2e.",
            )
    
        rest_port = _free_port()
        grpc_port = _free_port()
    
>       with tempfile.TemporaryDirectory(prefix="mai-e2e-") as tmpdir:
             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

tests\e2e\test_compliance_smoke.py:125: 
_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _
C:\Python314\Lib\tempfile.py:971: in __exit__
    self.cleanup()
C:\Python314\Lib\tempfile.py:975: in cleanup
    self._rmtree(self.name, ignore_errors=self._ignore_cleanup_errors)
C:\Python314\Lib\tempfile.py:955: in _rmtree
    _shutil.rmtree(name, onexc=onexc)
C:\Python314\Lib\shutil.py:852: in rmtree
    _rmtree_impl(path, dir_fd, onexc)
C:\Python314\Lib\shutil.py:689: in _rmtree_unsafe
    for dirpath, dirnames, filenames in results:
                                        ^^^^^^^
<frozen os>:413: in walk
    ???
C:\Python314\Lib\shutil.py:687: in onerror
    onexc(os.scandir, err.filename, err)
C:\Python314\Lib\tempfile.py:927: in onexc
    _resetperms(path)
C:\Python314\Lib\tempfile.py:283: in _resetperms
    _dont_follow_symlinks(_os.chmod, path, 0o700)
_ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _ _

func = <built-in function chmod>
path = 'C:\\Users\\17076\\AppData\\Local\\Temp\\mai-e2e-3k0odkvh', args = (448,)

    def _dont_follow_symlinks(func, path, *args):
        # Pass follow_symlinks=False, unless not supported on this platform.
        if func in _os.supports_follow_symlinks:
>           func(path, *args, follow_symlinks=False)
E           PermissionError: [WinError 5] Access is denied: 'C:\\Users\\17076\\AppData\\Local\\Temp\\mai-e2e-3k0odkvh'

C:\Python314\Lib\tempfile.py:272: PermissionError
=========================== short test summary info ===========================
ERROR apps/compliance-routed/tests/test_smoke.py::test_deny_blocks_dispatch_with_exit_code
ERROR apps/local-secure-inference/tests/test_smoke.py::test_deny_blocks_dispatch_with_exit_code
ERROR apps/openbao-trust-demo/tests/test_smoke.py::test_deny_blocks_dispatch_with_exit_code
ERROR apps/operator/tests/test_smoke.py::test_run_returns_5_on_core_panel_failure
ERROR apps/rag-reference/tests/test_smoke.py::test_deny_blocks_dispatch_with_exit_code
ERROR apps/tribal-sovereignty/tests/test_smoke.py::test_deny_blocks_dispatch_with_exit_code
ERROR mai-sdk-python/tests/test_config.py::test_from_file_reads_toml - Permis...
ERROR mai-sdk-python/tests/test_config.py::test_load_precedence_overrides_beat_env_beat_file
ERROR mai-sdk-python/tests/test_config.py::test_load_handles_missing_file_gracefully
ERROR tests/e2e/test_compliance_smoke.py::test_health_live_returns_status_live
ERROR tests/e2e/test_compliance_smoke.py::test_compliance_status_exposes_audit_integrity
ERROR tests/e2e/test_compliance_smoke.py::test_audit_chain_verifies_on_fresh_boot
ERROR tests/e2e/test_compliance_smoke.py::test_apply_healthcare_template_succeeds
ERROR tests/e2e/test_compliance_smoke.py::test_audit_chain_still_verifies_after_template_apply
ERROR tests/e2e/test_compliance_smoke.py::test_generate_hipaa_report_synchronously
ERROR tests/e2e/test_compliance_smoke.py::test_guest_blocked_from_view_audit_routes
669 passed, 35 skipped, 16 errors in 98.44s (0:01:38)
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
      "loc": 20908,
      "nosec": 0,
      "skipped_tests": 2
    }
  },
  "results": []
}
[main]	INFO	profile include tests: None
[main]	INFO	profile exclude tests: B310,B311,B603,B404,B607,B105,B101
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
Traceback (most recent call last):
  File "C:\Python314\Lib\pathlib\__init__.py", line 1011, in mkdir
    os.mkdir(self, mode)
    ~~~~~~~~^^^^^^^^^^^^
FileNotFoundError: [WinError 3] The system cannot find the path specified: 'C:\\Users\\17076\\AppData\\Local\\pip-audit\\Cache'

During handling of the above exception, another exception occurred:

Traceback (most recent call last):
  File "<frozen runpy>", line 198, in _run_module_as_main
  File "<frozen runpy>", line 88, in _run_code
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\pip_audit\__main__.py", line 8, in <module>
    audit()
    ~~~~~^^
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\pip_audit\_cli.py", line 452, in audit
    service = PyPIService(cache_dir=args.cache_dir, timeout=args.timeout)
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\pip_audit\_service\pypi.py", line 48, in __init__
    self.session = caching_session(cache_dir)
                   ~~~~~~~~~~~~~~~^^^^^^^^^^^
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\pip_audit\_cache.py", line 177, in caching_session
    cache=_SafeFileCache(_get_cache_dir(cache_dir, use_pip=use_pip)),
                         ~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\pip_audit\_cache.py", line 66, in _get_cache_dir
    pip_audit_cache_dir = user_cache_path("pip-audit", appauthor=False, ensure_exists=True)
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\platformdirs\__init__.py", line 587, in user_cache_path
    ).user_cache_path
      ^^^^^^^^^^^^^^^
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\platformdirs\api.py", line 268, in user_cache_path
    return Path(self.user_cache_dir)
                ^^^^^^^^^^^^^^^^^^^
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\platformdirs\windows.py", line 70, in user_cache_dir
    return self._append_parts(path, opinion_value="Cache")
           ~~~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\platformdirs\windows.py", line 47, in _append_parts
    self._optionally_create_directory(path)
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~^^^^^^
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\platformdirs\api.py", line 115, in _optionally_create_directory
    Path(path).mkdir(parents=True, exist_ok=True)
    ~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "C:\Python314\Lib\pathlib\__init__.py", line 1015, in mkdir
    self.parent.mkdir(parents=True, exist_ok=True)
    ~~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "C:\Python314\Lib\pathlib\__init__.py", line 1011, in mkdir
    os.mkdir(self, mode)
    ~~~~~~~~^^^^^^^^^^^^
PermissionError: [WinError 5] Access is denied: 'C:\\Users\\17076\\AppData\\Local\\pip-audit'
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

[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp\\pip-build-tracker-7dz7qclx"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp\\pip-build-tracker-_wi612zk"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp\\pip-build-tracker-m6wvtt9n"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp\\pip-download-e8bmr1d4"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp\\pip-ephem-wheel-cache-2yhzscxa"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp\\pip-ephem-wheel-cache-ry7mnmaf"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp\\pip-install-ucd_uonf"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp\\pip-install-ufr8h_4z"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp\\pip-target-8gvxh5gv"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp\\pip-target-dpfunxd7"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp\\pip-unpack-0pp2z_0t"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp\\pip-unpack-ge900ct0"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp\\pip-unpack-qchl1vb4"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp-2\\pip-build-tracker-ximcch1q"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp-2\\pip-ephem-wheel-cache-xi14qljh"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp-2\\pip-install-lrqjdqbm"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp-2\\pip-target-i0rc2si2"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pip-temp-2\\pip-unpack-let45qwl"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pytest\\pytest-of-17076"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m".tmp\\pytest-basetemp"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m"results\\lamprey-validation-pressure\\tmp\\pytest"
[90m2:12PM[0m [33mWRN[0m [1mskipping directory[0m [36merror=[0m[31m[1m"permission denied"[0m[0m [36mpath=[0m"results\\pytest-sdk-smoke\\run"
[90m2:12PM[0m [32mINF[0m [1mscanned ~9942868 bytes (9.94 MB) in 664ms[0m
[90m2:12PM[0m [32mINF[0m [1mno leaks found[0m
```

### IND-SEC-002 detect-secrets scan

Layer: `independent-implementation`
Toolchain: `Secrets`
Status: `FAIL`
Command: `detect-secrets scan --all-files`
Exit code: `1`

```text
Traceback (most recent call last):
  File "<frozen runpy>", line 198, in _run_module_as_main
  File "<frozen runpy>", line 88, in _run_code
  File "C:\Users\17076\.cargo\bin\detect-secrets.exe\__main__.py", line 5, in <module>
    sys.exit(main())
             ~~~~^^
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\detect_secrets\main.py", line 30, in main
    handle_scan_action(args)
    ~~~~~~~~~~~~~~~~~~^^^^^^
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\detect_secrets\main.py", line 70, in handle_scan_action
    secrets = baseline.create(
        *args.path,
    ...<2 lines>...
        num_processors=args.num_cores,
    )
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\detect_secrets\core\baseline.py", line 34, in create
    secrets.scan_files(
    ~~~~~~~~~~~~~~~~~~^
        *get_files_to_scan(*paths, should_scan_all_files=should_scan_all_files, root=root),
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
        **kwargs,
        ^^^^^^^^^
    )
    ^
  File "C:\Users\17076\AppData\Roaming\Python\Python314\site-packages\detect_secrets\core\secrets_collection.py", line 63, in scan_files
    with mp.Pool(
         ~~~~~~~^
        processes=num_processors,
        ^^^^^^^^^^^^^^^^^^^^^^^^^
        initializer=configure_settings_from_baseline,
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
        initargs=(child_process_settings,),
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    ) as p:
    ^
  File "C:\Python314\Lib\multiprocessing\context.py", line 119, in Pool
    return Pool(processes, initializer, initargs, maxtasksperchild,
                context=self.get_context())
  File "C:\Python314\Lib\multiprocessing\pool.py", line 191, in __init__
    self._setup_queues()
    ~~~~~~~~~~~~~~~~~~^^
  File "C:\Python314\Lib\multiprocessing\pool.py", line 346, in _setup_queues
    self._inqueue = self._ctx.SimpleQueue()
                    ~~~~~~~~~~~~~~~~~~~~~^^
  File "C:\Python314\Lib\multiprocessing\context.py", line 113, in SimpleQueue
    return SimpleQueue(ctx=self.get_context())
  File "C:\Python314\Lib\multiprocessing\queues.py", line 360, in __init__
    self._reader, self._writer = connection.Pipe(duplex=False)
                                 ~~~~~~~~~~~~~~~^^^^^^^^^^^^^^
  File "C:\Python314\Lib\multiprocessing\connection.py", line 614, in Pipe
    h2 = _winapi.CreateFile(
        address, access, 0, _winapi.NULL, _winapi.OPEN_EXISTING,
        _winapi.FILE_FLAG_OVERLAPPED, _winapi.NULL
        )
PermissionError: [WinError 5] Access is denied
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
 JSON                      2         1278         1278            0            0
 PowerShell                7         1174          959           91          124
 Protocol Buffers          2         1171          745          256          170
 Python                  171        26251        21226          719         4306
 Shell                    13         1901         1428          249          224
 Plain Text               17          271            0          269            2
 TOML                     57         2977         1605          987          385
 YAML                      2         1660         1539           53           68
─────────────────────────────────────────────────────────────────────────────────
 Markdown                133        31126            0        23601         7525
 |- BASH                  38          417          353           42           22
 |- HCL                    1            6            5            1            0
 |- JSON                  24          907          902            0            5
 |- PowerShell            21          164          117           27           20
 |- Python                13          244          199           14           31
 |- Rust                   8          272          206           56           10
 |- TOML                   7          203          177            2           24
 |- YAML                   1            2            2            0            0
 (Total)                            33341         1961        23743         7637
─────────────────────────────────────────────────────────────────────────────────
 Rust                    247        81788        68954         3261         9573
 |- Markdown             246        11234            8        10162         1064
 (Total)                            93022        68962        13423        10637
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
 Total                   652       163216        99755        39892        23569
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
Python                171      26,251     3,206     2,544     20,501      1,699
Markdown              139      33,945     7,768         0     26,177          0
TOML                   59       3,090       398     1,063      1,629          6
Plain Text             17         271         2         0        269          0
Shell                  17       2,190       271       287      1,632        230
Powershell              7       1,174       110        83        981        173
YAML                    6       2,307       137       155      2,015          0
Systemd                 5         197        19         8        170          0
JSON                    4       1,303         0         0      1,303          0
BASH                    2         174        30        20        124         30
Protocol Buffe…         2       1,171       170       256        745          0
Docker ignore           1          57        11        15         31          0
Dockerfile              1         170        16       102         52          5
JavaScript              1         372        32        26        314         25
License                 1          11         1         0         10          0
───────────────────────────────────────────────────────────────────────────────
Total                 680     165,744    21,723    19,038    124,983      5,505
───────────────────────────────────────────────────────────────────────────────
Estimated Cost to Develop (organic) $4,298,221
Estimated Schedule Effort (organic) 23.94 months
Estimated People Required (organic) 15.95
───────────────────────────────────────────────────────────────────────────────
Processed 6302604 bytes, 6.303 megabytes (SI)
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
