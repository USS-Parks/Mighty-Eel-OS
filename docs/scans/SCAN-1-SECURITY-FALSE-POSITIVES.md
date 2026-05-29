# SCAN-1 — Security False Positives

This document explains why naive static scans (the VibecoderHub PDF and SCAN-1's first pass) flagged certain locations as security issues that are **not** real risks. It exists so future scans can be cross-referenced against it instead of re-investigating the same patterns.

---

## SEC-004 — "Hardcoded API keys and secrets" hits

The scanner greps for patterns like `api[_-]?key\s*=\s*["']`, `secret\s*=\s*["']`, `password\s*=\s*["']`.

### Locations that match but are NOT secrets

| Path | Why the match is safe |
|---|---|
| `.env.example` | Template file. All values are obvious placeholders (`changeme`, `your-key-here`, `0000...`). The real `.env` is `.gitignore`-d (verified — `.gitignore` line 27). |
| `mai-sdk-python/tests/test_config.py` | Unit-test fixtures. Test API keys are literal strings like `"test-key-123"` that exercise the loader code path. No production code reads these. |
| `adapters/*/tests/test_*.py` | Same pattern: per-adapter unit-test fixtures. |
| `mai-api/tests/*.rs` integration tests | API key constants used to drive the `ApiKeyStore` in integration tests (`TEST_KEY_*`). Hardcoded by design — these are the tests' own input. |
| `docs/**/*.md` documentation | Example snippets showing CLI usage with `--api-key=YOUR_KEY_HERE`. |

### Verification

- `git grep -n "api_key\|api-key\|secret\|password" -- ':!*/tests/*' ':!docs/' ':!.env.example' ':!*.lock'` returns only legitimate references (struct field names, parameter names, error messages).
- `gitleaks detect --config .gitleaks.toml --no-git` (per J-10b config) returns zero findings on this tree.
- `cargo deny check sources` is clean.

### Recommendation for future scans

Treat SEC-004 as PASS when the only matches are in:
- `.env.example` (template)
- `*/tests/**`
- `docs/**`
- `*.lock` / `requirements-lock.txt` (digest fields, not credentials)

A real SEC-004 finding would be a literal credential in production source under `mai-*/src/`, `adapters/*/adapter.py`, or `adapters/*/client.py`. None exist.

---

## QUA-005 — "Excessive println! / print() per file"

### `mai-api/src/main.rs` — 20 `println!`/`eprintln!` hits

Every hit is in `run_validate_subcommand` (the SHIP-02 stop-gap `mai-api validate ...` CLI). The function deliberately uses `println!` / `eprintln!` for three reasons:

1. **`--help` text** — operator-facing usage info must go to stdout, not to the structured-log sink.
2. **`--json` output** — the SHIP readiness report is emitted as a single JSON line on stdout; consumers pipe it into `jq` and other CLI tools.
3. **`error: ...` lines** — startup-time argument errors go to stderr so the exit code + stderr can drive shell automation; tracing isn't initialized yet at this point (the function runs *before* `tracing_subscriber::fmt().init()`).

This is correct CLI practice. The function is annotated in SCAN-1:

```rust
#[allow(clippy::print_stdout, clippy::print_stderr)]
fn run_validate_subcommand(args: &[String]) -> ExitCode {
    ...
}
```

so future scans (and `cargo clippy --workspace -- -D warnings -W clippy::print_stdout -W clippy::print_stderr`) treat the file as PASS.

### Recommendation for future scans

When counting `println!`/`eprintln!`, exclude functions annotated with `#[allow(clippy::print_stdout)]` or `#[allow(clippy::print_stderr)]`, and exclude files under `*/bin/` and `tools/`.

---

## SEC-007 — "Public env var secret exposure"

N/A for this product. There is no frontend SPA shipping `NEXT_PUBLIC_*` / `VITE_*` / `REACT_APP_*` variables to a browser. The sole UI is `compliance-dashboard/`, which runs server-side and does not embed env-vars into a client bundle.

---

## Frontend bucket (FE-001..FE-008)

N/A. MAI is air-gapped inference middleware; the API surface is REST + gRPC + WebSocket. The compliance dashboard is the only UI and is out of this scan's scope (its own audit lives at `compliance-dashboard/AUDIT.md`).

---

*Cross-reference: see `docs/SCAN-1-INTERNAL-GITDOCTOR-REPORT.md` for the full SCAN-1 ledger.*
