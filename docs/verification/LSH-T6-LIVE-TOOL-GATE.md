# LSH-T6 Live Tool Governance Gate

Date: 2026-07-17
Status: PASS
Repository: `USS-Parks/Mighty-Eel-OS`
Prompt: `LSH-T6`

## Authority under test

- OpenBao image:
  `openbao/openbao@sha256:436eaf9778cad75507ff70ea26ace30dcbe15606e619ac3823495663d7f7c115`
- OpenBao transport: loopback HTTP, disposable development authority
- Parent authentication: AppRole carrying only token-create and
  revoke-accessor rights
- Child authority: non-renewable, no-default-policy token created through an
  exact token role with one explicit tool policy
- Maximum child TTL: 60 seconds; the exercised tool definitions requested 30
  seconds
- Child token strings: redacted from debug output and zeroized on drop
- Executor transport: loopback HTTP fixture selected only by operator route
  configuration; redirects disabled
- Response ceiling: 1,024 bytes

No fixture credential, root token, AppRole secret, child token, or authorization
header is stored in this evidence file.

## Live matrix

| Case | Expected invariant | Result |
|---|---|---|
| benign read | exact live lease; successful bounded JSON | PASS |
| injected read | output remains untrusted; follow-on mutation pauses | PASS |
| mutation | exact authenticated approval required before mint/execute | PASS |
| concurrent reads | four distinct leases; no shared standing credential | PASS |
| oversized response | streamed byte ceiling terminates fail-closed | PASS |
| secret-bearing response | scanner removes fixture before model/receipt | PASS |
| cancelled call | drop guard durably enqueues accessor revocation | PASS |

Ten unique child leases reached the live executor. After normal completion or
cancellation, OpenBao rejected lookup of all ten accessors. Nine completed calls
formed a valid tool-receipt chain. The cancelled call has no fabricated
completion receipt; its durable revocation record is the cancellation evidence.

## Commands and outcomes

```text
cargo test -p aog-tool-runtime --test live_tool_governance -- --nocapture
PASS: 1/1

cargo test -p aog-tool-runtime -p aog-toolproxy -p aog-approvals -p wsf-bridge
PASS

cargo test -p aog-conformance --test robustness_conformance
PASS: 11/11

cargo test -p aog-controller --test managed_toolproxy
PASS: 1/1

cargo check --workspace
PASS

cargo clippy --workspace --all-targets -- -D warnings -A clippy::pedantic
PASS

cargo test --workspace
PASS

cargo fmt --all -- --check
PASS

cargo audit
PASS: zero vulnerabilities; 495 dependencies scanned

cargo deny check
PASS: advisories ok, bans ok, licenses ok, sources ok
```

The workspace commands used the already-installed host `protoc` because the
restricted execution identity cannot invoke that user-scoped binary. This was
an execution-environment correction, not a code or gate waiver.

## Closure mapping

- `LSF-026`: server-derived provenance plus live injected-output mutation gate
- `LSF-027`: existing atomic reservation regressions remain green under the
  production composition
- `LSF-028`: live tool errors and bounded executor failures remain scanner-bound
- `LSF-029`: secret-bearing structured output is redacted before model context
- `LSD-009`: exact canonical `tool/<id>` binding and tenant/tool role mapping
- `LSD-010`: authority-enforced TTL, cancellation drop guard, durable accessor
  spool, bounded streaming response, and bounded scanner
