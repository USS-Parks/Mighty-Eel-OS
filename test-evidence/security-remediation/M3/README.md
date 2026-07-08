# M3 (Phase F) - deferred high-impact frontier closure: evidence

Phase F audited the deferred high-impact shards (F1-F9). Each prompt was reviewed by
an independent read-only auditor against its PSPR gate; every High/Critical claim was
confirmed against source before any fix. Full narrative + verify results:
`docs/sessions/SECURITY-REMEDIATION-DEVLOG.md` (Phase F section). Finding status:
`docs/sessions/SECURITY-REMEDIATION-LEDGER.md`.

## Fixed with tests + gates (fmt / cargo check / clippy -D warnings / cargo test)

| Prompt | Finding | Fix |
|--------|---------|-----|
| F1 | AF-03 (Critical) gRPC trusts caller metadata | token-authenticated gRPC identity via the shared key store; reflection opt-in |
| F1 | NEW-1 (High) REST install trusts X-IM-Profile header | use the middleware-authenticated ProfileInfo |
| F1 | NEW-3 rotate-credentials dead permission | manage_profiles |
| F5a | DF-01A (High) manifest unauthenticated | manifest.mldsa authentication + weights binding + strict mode |
| F5a | DF-01B model_id path traversal | validate_model_id at both vault backends |
| F5a | NEW-1/2 JSON injection / usb name | serde_json entries + package-name validation |
| F5b | AF-11 restore path escape | validate_component_path (relative, Normal-only) |
| F5b | AF-19 restore unsigned-by-default | verify on by default; --allow-unsigned opt-out |
| F2 | AF-15B + N1/N3 gateway revocation | complete predicate + fail-closed on absent + freshness |
| F8 | AF-20 mutable first-party image | pinned ${WSF_API_VERSION:-v0.1.0} |
| F4 | adapter stdout/stderr DoS | 8 MiB bounded frame + stderr drain |

## Dispositioned (REPORTABLE / SUPPRESSED, not force-fixed)

- **F3 tool-proxy** - PRE-INTEGRATION: `ToolProxy::invoke` has no production caller;
  findings are unreachable until the proxy is wired. Fix direction recorded.
- **F6 audit-integrity** - sign-before-mutate (N1), concurrent-append fork (N2),
  interval-signature assertion (N3), composer fail-open (N6), Null-crypto defaults (N7)
  sit on the NullSigner-default audit path; a production crypto guard + sign-after-mutate
  are a feature set. The misleading "guard pattern" comment was corrected (N7).
- **F7 host/scheduler** - mostly ALREADY-FIXED; residual is Low multi-tenant fairness /
  inert router / dev air-gap stub (fail-safe).

## Deferred runtime surfaces (OPEN, tracked)

- **DEF-1** full adapter CPU/mem/fs/proc/net cgroup isolation - needs a Linux+cgroups
  host (bounded-frame/stderr DoS hardening landed in F4).
- **DEF-2** signed, size-capped, SSRF-resistant model update transport - `update.rs`
  applies no scheme/host allowlist, size cap, or post-download hash check.

## Live proof

The trust-plane fixes are additionally exercised end-to-end by the Phase X live gates
(X2) against Dockerized OpenBao + Moto; the independent re-scan (X4) is the CLOSED gate.