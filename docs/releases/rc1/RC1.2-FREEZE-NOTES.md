# RC1.2 Freeze Notes

**Project:** Lamprey MAI
**Release:** RC1.2 (Post-DOUGHERTY Tester Bundle)
**Date:** 2026-05-25 (Memorial Day)
**Audience:** release engineers, RC1 testers, acquirer reviewers
**Supersedes:** docs/RC1-FREEZE-NOTES.md (RC1.1, freeze dceaabc)

---

## 1. Freeze Point

| Field | Value |
|---|---|
| Repository | mai/ (Cargo workspace + pyproject.toml monorepo) |
| Branch | main |
| HEAD commit | e55c1ff12392c1e8cad24d1f7fbb302a3bbe9c81 (short: e55c1ff) |
| Subject | docs: add Memorial Day 2026 local GitDoctor scan report |
| Author | USS-Parks <basho.parks@gmail.com> |
| Authored | 2026-05-25T22:33:34-07:00 |
| Commits since RC1.1 | 103 (from dceaabc to e55c1ff) |

## 2. What Changed vs RC1.1

| Area | RC1.1 (dceaabc) | RC1.2 (e55c1ff) |
|---|---|---|
| Adapters | 7 (ollama, llamacpp, exllamav2, vllm, tgi, sglang, tensorrt) | 11 (adds openai_compat, onnxruntime, mlx, triton) |
| Lock files | Cargo.lock only | + requirements-lock.txt + package-lock.json |
| Docker | None | Dockerfile + .dockerignore + .env.example |
| Rust SDK | 17 	odo!() stubs | Full HTTP + SSE client, 0 	odo!(), 25 wiremock tests |
| Python SDK | Present | Present (unchanged) |
| Tests | S46 baseline (~1,539) | + adapter live-backend + e2e + SDK + assertion gate |
| Docs | RC1 docs only | + ADAPTER-COMPLETION-MATRIX, ERROR-PATH-AUDIT, RC1-TESTER-RESPONSE-DOUGHERTY, MEMORIAL-DAY-SCAN-REPORT, J-15-DOUGHERTY-CLOSURE |
| DOUGHERTY lane | Not started | **Closed** — 26/26 J-sessions complete (J-23..J-26 landed under `a072634`) |
| Local GitDoctor score | ~52/100 (external scan) | **93/100** (local scan, zero HIGH findings) |

## 3. Release Binaries

Built 2026-05-25 on x86_64-pc-windows-msvc:

| Binary | Size |
|---|---|
| lamprey-mai-api.exe | 10.09 MB |
| lamprey-mai.exe (launcher) | 2.57 MB |
| lamprey-mai-admin.exe | 3.58 MB |
| lamprey-mai-ship-validate.exe | 1.67 MB |

## 4. DOUGHERTY Lane Status

The DOUGHERTY remediation lane (J-01..J-26) is **closed** as of 2026-05-25. All 26 sessions complete. J-23..J-26 (OpenAI-compat, ONNX Runtime, MLX, Triton) all landed under commit `a072634` in a parallel session. Full closure document at docs/dougherty/J-15-DOUGHERTY-CLOSURE.md.

## 5. What RC1.2 Excludes (same as RC1.1)

- mai/target/debug/ — debug build artifacts
- .pytest_cache/, .mypy_cache/, .ruff_cache/ — tool caches
- Local generated logs and stale test output
- Model weights
- Local IDE state

## 6. Acceptance Checklist

| Criterion | Status |
|---|---|
| Named freeze commit | e55c1ff on main |
| Freeze notes describe inclusions and changes | §2 |
| Working tree clean at freeze | Confirmed |
| DOUGHERTY lane closed | Yes (J-15 doc) |
| Release binaries rebuilt | Yes (§3) |
| Local GitDoctor re-scan complete | Yes (93/100) |
| RC-11 re-ship ready | Yes (docs/RC1.2-RESHIP.md) |

---

*Authored and reviewed by Basho Parks, copyright 2026*
