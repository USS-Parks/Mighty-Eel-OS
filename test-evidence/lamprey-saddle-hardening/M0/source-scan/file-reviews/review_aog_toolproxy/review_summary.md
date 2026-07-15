# AOG Tool Proxy Defensive Review

All four assigned files were read in full. The existing crate suite passed with 53 tests using an isolated worker-local Cargo target directory.

## Independently reachable operations and modes

- `register`: accepts a local `ToolDefinition`; the shared registry overwrites duplicate IDs and validates only that the schema value is an object.
- `invoke` validation modes: registered/unknown tool, caller role, guardrails on/off, mission attached/absent, side-effecting/read-only, caller-marked trusted/untrusted, approval gate present/absent, credential minter present/absent.
- `invoke` execution modes: approved, blocked, mint failure, executor success/failure/timeout, post-execution redaction, credential revoke, session/receipt append.
- governance queries: receipt count/head/verification/snapshot.
- mission controls: allowed tools, optional allowed systems, call ceiling, caller-declared spend ceiling.
- guard controls: token-profile allowlists, call cap, distinct-system cap, bounded per-session usage map.
- scanner modes: strings, arrays, object values, non-string scalars, PHI/ITAR/selected-secret span redaction.

## Candidate disposition

Reportable candidates:

1. `ATPROXY-PROVENANCE-DEFAULT` — caller-supplied provenance plus an absent approval gate leaves side-effecting execution default-allow.
2. `ATPROXY-MISSION-CAP-RACE` — mission check and charge are non-atomic across asynchronous approval.
3. `ATPROXY-GUARD-CAP-RACE` — operator hard-cap check and charge are likewise non-atomic.
4. `ATPROXY-EGRESS-ERROR` — error text is neither scanned nor redacted and is copied into receipts.
5. `ATPROXY-EGRESS-OBJECT-KEY` — object keys bypass the value-only scanner.

Deferred candidates with exact proof gaps:

1. `ATPROXY-MISSION-SYSTEM-NONE` — exact local bypass exists for absent system metadata, but a deployed caller/executor is needed to prove production permits omission and independently chooses a protected target.
2. `ATPROXY-GUARD-SYSTEM-NONE` — same deployment proof gap for the distinct-system cap, preserved as an independent control instance.
3. `ATPROXY-CRED-CANCEL-REVOKE` — cancellation skips explicit revoke, but no production minter/maximum broker TTL was found to calibrate residual credential authority.
4. `ATPROXY-EGRESS-UNBOUNDED` — scan has no local size/depth ceiling, but deployed executor framing/service containment was outside the reviewed reachability set.

## Exact suppressions and non-promoted gaps

- Unknown tools are rejected before execution at `lib.rs:292-295`; the passing `unknown_tool_is_denied_fail_closed` test confirms the negative control.
- A mint error returns before `executor.execute` at `lib.rs:411-414`; it is fail-closed for privilege impact, although usage is charged and no receipt is appended.
- Normal executor hangs are bounded by `tokio::time::timeout` at `lib.rs:416-425`; the residual issue is post-execution/cancellation work, not the executor timeout branch itself.
- The exact borrowed `ToolCall` and `InvokeContext` passed to `ApprovalGate::review` remain immutable until the same invocation continues, so no local post-approval argument swap was found. Freshness, replay, actor authentication, and cross-session ID uniqueness depend on the external gate implementation and remain outside these four files.
- The mission tool axis is fail-closed once a contract is attached (`mission.rs:128-133`), but `ToolProxy::new` attaches no mission or deny-unlisted token policy. Whether production requires these builders is not shown by an in-repository runtime constructor.
- No signed/pinned manifest or description-drift verification occurs in these files, and the underlying registry permits duplicate-ID replacement; only tests register tools in the repository-wide caller search, so no lower-trust registration path was proved.
- `InvokeContext` also carries role, profile/token ID, session ID, and cost as caller-provided values. No production network/token adapter invoking this proxy was found, so cross-tenant/token impersonation was not promoted without an actual lower-trust caller.
- Argument enforcement delegates to `mai-agent::ToolRegistry::validate_tool_call`, which checks object/null shape rather than the declared JSON Schema. No concrete deployed executor was found in the reviewed caller set to establish a filesystem, command, or SSRF sink, so this remains a deferred integration gap rather than a standalone injection finding.
- Filesystem and network egress restrictions are not generic ToolProxy controls; they must be enforced by concrete executors. No deployed executor implementation was reachable from the assigned files.
