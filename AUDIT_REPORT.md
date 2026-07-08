# Security Review: mai

## Scope

Repository-wide standard security scan of the validated mai Git worktree snapshot. All 615 deterministic source-like inventory rows received full-file receipts; prior audit claims were revalidated against current code.

- Scan mode: repository
- Target kind: git_worktree
- Target ID: target_sha256_60941ffe5293cac632c572b18cdbfd9da0b94e604f744bec73688f6b738e9e15
- Revision: 6ffaaeeea0a83c7fa071e114183cfa60c5898703
- Snapshot digest: codex-security-snapshot/v1:sha256:b920522f2f117347053cfb8f0e35237868c1da3b9743ecd7549edb755bf7ddb4
- Inventory strategy: repository
- Included paths: .
- Excluded paths: none
- Runtime or test status: PASS: cargo fmt --check; PASS: cargo check --workspace; PASS: cargo test --workspace (1831 passed, 2 ignored, 137 suites); FAIL: cargo clippy --workspace -- -D warnings -A clippy::pedantic at mai-core/src/cache.rs:109 (clippy::doc_lazy_continuation).
- Artifacts reviewed: Cargo workspace and Python project, runtime Rust/Python sources, deployment, packaging, config, and tools, prior repository audit and remediation rosters
- Scan context: Authoritative repository threat model was preserved byte-for-byte from docs/scans/threat_model.md.

Limitations and exclusions:
- Three usable worker slots were below the six recommended by preflight; thread policy required parent-agent execution.
- Live production services and hardware were unavailable.
- Two runtime areas remain explicitly deferred in coverage.
- Excluded tests/, docs/, examples/, generated and build outputs from deterministic rank inventory: Default exhaustive workflow excludes non-runtime/supporting material unless deployment or privilege evidence adds it back; deployment/config and prior audit docs were reviewed separately.

### Scan Summary

| Field | Value |
| --- | --- |
| Reportable findings | 24 |
| Severity mix | high: 11, medium: 13 |
| Confidence mix | high: 20, medium: 4 |
| Coverage | partial |
| Validation mode | Static source/control/sink validation with existing tests and deployment evidence; live OpenBao/AWS/ZFS reproduction was not run. |

Canonical artifacts: `scan-manifest.json`, `findings.json`, and `coverage.json`. This report is a deterministic projection of those files.

## Threat Model

Mighty Eel MAI is a local-first inference and governance appliance whose crown jewels are regulated content, tenant identity/policy, signing and encryption authority, cloud credential brokerage, audit evidence, model/package integrity, and host/GPU control.

### Assets

- Regulated prompts, completions, embeddings, and documents
- Tenant identity, policy, budgets, and route authority
- Signing, sealing, OpenBao, TPM, ZFS, and cloud credential authority
- Audit receipts, WAL chains, readiness evidence, and release manifests
- Model weights and appliance availability

### Trust Boundaries

- Network clients to MAI, WSF, and AOG APIs
- Tenant/workload identity to the trust plane
- Rust core to Python adapters and inference backends
- Appliance to OpenBao, cloud STS, TPM, ZFS, and stores
- Package, removable media, update, and restore inputs to privileged filesystem operations
- Agent/model output to tools and egress
- Operator configuration to production runtime

### Attacker Capabilities

- Send unauthenticated or authenticated API/RPC requests
- Control package, backup, model, and selected configuration inputs at defined boundaries
- Supply malicious model/provider output or stored tenant metadata
- Influence registry tags and unavailable/stale external control state

### Security Objectives

- Authenticate and authorize every privileged operation with tenant binding
- Make attenuation restriction-only and revocation fresh and monotonic
- Bind encryption and sealing to tenant, owner, audience, policy, and operation
- Prove real storage, cryptographic, audit, and policy controls before binding
- Contain all paths below approved roots and authenticate package/update metadata
- Apply identical policy, metering, and audit controls to every protocol mode

### Assumptions

- Kernel/root/firmware compromise is outside application guarantees.
- Demo-only modes reduce severity only when production profiles cannot enable or expose them.
- Hardware-dependent claims without live evidence remain unproven.

## Findings

| Finding | Severity | Confidence |
| --- | --- | --- |
| [AWS credential exchange accepts a caller-selected role ARN](#finding-1) | high | high |
| [Unauthenticated WSF token issuance grants caller-selected authority](#finding-2) | high | high |
| [AOG defaults to non-blocking shadow policy mode](#finding-3) | high | high |
| [Legacy OpenAI completions bypass compliance routing and accounting](#finding-4) | high | high |
| [Appliance composition publishes dev OpenBao with a known root token](#finding-5) | high | medium |
| [Model package signature authenticates weights but not manifest identity](#finding-6) | high | high |
| [Attenuation signs attacker-constructed child tokens without authenticating the parent](#finding-7) | high | high |
| [Envelope unseal is not bound to tenant subject audience or policy](#finding-8) | high | high |
| [gRPC trusts caller-authored administrator metadata](#finding-9) | high | high |
| [OpenAI streaming bypasses egress tokenization metering and receipts](#finding-10) | high | high |
| [Anthropic streaming bypasses egress tokenization metering and receipts](#finding-11) | high | high |
| [Restore accepts unsigned or unverified manifests by default](#finding-12) | medium | medium |
| [Manifest-derived model ID can escape the vault root](#finding-13) | medium | high |
| [AOG revocation check fails open when snapshot is absent](#finding-14) | medium | high |
| [ROI endpoint computes recommendations from every tenant](#finding-15) | medium | high |
| [Restore manifest component paths can escape backup and target roots](#finding-16) | medium | high |
| [Production readiness certifies a merely constructed vault](#finding-17) | medium | high |
| [Usage endpoint returns aggregates for every tenant](#finding-18) | medium | high |
| [AWS credentials can outlive remaining WSF token authority](#finding-19) | medium | medium |
| [WSF receipt queries are unauthenticated and cross-tenant](#finding-20) | medium | high |
| [Vault snapshot and rollback APIs report success without ZFS operations](#finding-21) | medium | high |
| [ZFS vault stores and loads model weights as plaintext](#finding-22) | medium | high |
| [Production-like deployment images use mutable tags](#finding-23) | medium | medium |
| [Revocation snapshots lack freshness scope and anti-rollback enforcement](#finding-24) | medium | high |

### Confidence Scale

| Label | Meaning |
| --- | --- |
| high | Direct evidence supports the finding with no material unresolved blocker. |
| medium | Evidence supports a plausible issue, but material runtime or reachability proof remains. |
| low | Evidence is incomplete and the item is retained only for explicit follow-up. |

<a id="finding-1"></a>

### [1] AWS credential exchange accepts a caller-selected role ARN

| Field | Value |
| --- | --- |
| Severity | high |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Confused deputy |
| CWE | CWE-441 |
| Affected lines | crates/wsf-api/src/lib.rs:333 |

#### Summary

ExchangeReq.role_arn reaches AwsStsBroker assumes the supplied role because No server-side tenant/workload-to-role allowlist. The result is Cloud privilege escalation to any role trusted by broker root credentials.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, No server-side tenant/workload-to-role allowlist, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `crates/wsf-api/src/lib.rs:331-335`

This source implements the control point where No server-side tenant/workload-to-role allowlist.

```rust
}

async fn exchange(
    State(s): State<AppState>,
    Json(req): Json<ExchangeReq>,
```

#### Validation

The current snapshot confirms ExchangeReq.role_arn -\> No server-side tenant/workload-to-role allowlist -\> AwsStsBroker assumes the supplied role -\> Cloud privilege escalation to any role trusted by broker root credentials.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `crates/wsf-api/src/lib.rs:331-335`

This source implements the control point where No server-side tenant/workload-to-role allowlist.

```rust
}

async fn exchange(
    State(s): State<AppState>,
    Json(req): Json<ExchangeReq>,
```

#### Dataflow

ExchangeReq.role_arn -\> No server-side tenant/workload-to-role allowlist -\> AwsStsBroker assumes the supplied role -\> Cloud privilege escalation to any role trusted by broker root credentials

- **Source:** ExchangeReq.role_arn

- **Sink:** AwsStsBroker assumes the supplied role

- **Outcome:** Cloud privilege escalation to any role trusted by broker root credentials

**Broken or missing security control** — `crates/wsf-api/src/lib.rs:331-335`

This source implements the control point where No server-side tenant/workload-to-role allowlist.

```rust
}

async fn exchange(
    State(s): State<AppState>,
    Json(req): Json<ExchangeReq>,
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** ExchangeReq.role_arn

- **Entry point:** crates/wsf-api/src/lib.rs

- **Outcome:** Cloud privilege escalation to any role trusted by broker root credentials

#### Severity

**High** — The path is in scope and crosses a protected product boundary. Cloud privilege escalation to any role trusted by broker root credentials. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: No server-side tenant/workload-to-role allowlist. Add a negative regression test that drives ExchangeReq.role_arn and proves the operation cannot reach AwsStsBroker assumes the supplied role.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-2"></a>

### [2] Unauthenticated WSF token issuance grants caller-selected authority

| Field | Value |
| --- | --- |
| Severity | high |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Missing authentication |
| CWE | CWE-306 |
| Affected lines | crates/wsf-api/src/lib.rs:230 |

#### Summary

Anonymous JSON request reaches TrustBridge::issue_token signs caller-selected tenant subject roles models and budget because No authentication middleware or server-derived principal. The result is Remote minting of trusted cross-tenant capability tokens.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, No authentication middleware or server-derived principal, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `crates/wsf-api/src/lib.rs:228-232`

This source implements the control point where No authentication middleware or server-derived principal.

```rust
) -> Result<Json<TokenResp>, ApiError> {
    let ir = IssueTokenRequest::new(req.tenant_id, req.subject_id, req.roles)
        .with_models(req.allowed_models);
    let ir = if let Some(b) = req.budget {
        ir.with_budget(b)
```

#### Validation

The current snapshot confirms Anonymous JSON request -\> No authentication middleware or server-derived principal -\> TrustBridge::issue_token signs caller-selected tenant subject roles models and budget -\> Remote minting of trusted cross-tenant capability tokens.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `crates/wsf-api/src/lib.rs:228-232`

This source implements the control point where No authentication middleware or server-derived principal.

```rust
) -> Result<Json<TokenResp>, ApiError> {
    let ir = IssueTokenRequest::new(req.tenant_id, req.subject_id, req.roles)
        .with_models(req.allowed_models);
    let ir = if let Some(b) = req.budget {
        ir.with_budget(b)
```

#### Dataflow

Anonymous JSON request -\> No authentication middleware or server-derived principal -\> TrustBridge::issue_token signs caller-selected tenant subject roles models and budget -\> Remote minting of trusted cross-tenant capability tokens

- **Source:** Anonymous JSON request

- **Sink:** TrustBridge::issue_token signs caller-selected tenant subject roles models and budget

- **Outcome:** Remote minting of trusted cross-tenant capability tokens

**Broken or missing security control** — `crates/wsf-api/src/lib.rs:228-232`

This source implements the control point where No authentication middleware or server-derived principal.

```rust
) -> Result<Json<TokenResp>, ApiError> {
    let ir = IssueTokenRequest::new(req.tenant_id, req.subject_id, req.roles)
        .with_models(req.allowed_models);
    let ir = if let Some(b) = req.budget {
        ir.with_budget(b)
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Anonymous JSON request

- **Entry point:** crates/wsf-api/src/lib.rs

- **Outcome:** Remote minting of trusted cross-tenant capability tokens

#### Severity

**High** — The path is in scope and crosses a protected product boundary. Remote minting of trusted cross-tenant capability tokens. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: No authentication middleware or server-derived principal. Add a negative regression test that drives Anonymous JSON request and proves the operation cannot reach TrustBridge::issue_token signs caller-selected tenant subject roles models and budget.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-3"></a>

### [3] AOG defaults to non-blocking shadow policy mode

| Field | Value |
| --- | --- |
| Severity | high |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Protection mechanism failure |
| CWE | CWE-693 |
| Affected lines | crates/aog-gateway/src/main.rs:62 |

#### Summary

Missing AOG_MODE in deployed environment reaches Policy violations are logged but never blocked because Default is shadow and AppState also defaults Shadow. The result is Regulated payloads can be sent to cloud despite deny decisions.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, Default is shadow and AppState also defaults Shadow, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `crates/aog-gateway/src/main.rs:60-64`

This source implements the control point where Default is shadow and AppState also defaults Shadow.

```rust
    let vk_prefix = env_or("AOG_VIRTUAL_KEY_PREFIX", "kv/data/aog/virtual-keys");
    let listen = env_or("AOG_LISTEN", "0.0.0.0:8080");
    let mode_str = env_or("AOG_MODE", "shadow");
    let mode = PolicyMode::parse(&mode_str).ok_or_else(|| format!("bad AOG_MODE '{mode_str}'"))?;

```

#### Validation

The current snapshot confirms Missing AOG_MODE in deployed environment -\> Default is shadow and AppState also defaults Shadow -\> Policy violations are logged but never blocked -\> Regulated payloads can be sent to cloud despite deny decisions.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `crates/aog-gateway/src/main.rs:60-64`

This source implements the control point where Default is shadow and AppState also defaults Shadow.

```rust
    let vk_prefix = env_or("AOG_VIRTUAL_KEY_PREFIX", "kv/data/aog/virtual-keys");
    let listen = env_or("AOG_LISTEN", "0.0.0.0:8080");
    let mode_str = env_or("AOG_MODE", "shadow");
    let mode = PolicyMode::parse(&mode_str).ok_or_else(|| format!("bad AOG_MODE '{mode_str}'"))?;

```

#### Dataflow

Missing AOG_MODE in deployed environment -\> Default is shadow and AppState also defaults Shadow -\> Policy violations are logged but never blocked -\> Regulated payloads can be sent to cloud despite deny decisions

- **Source:** Missing AOG_MODE in deployed environment

- **Sink:** Policy violations are logged but never blocked

- **Outcome:** Regulated payloads can be sent to cloud despite deny decisions

**Broken or missing security control** — `crates/aog-gateway/src/main.rs:60-64`

This source implements the control point where Default is shadow and AppState also defaults Shadow.

```rust
    let vk_prefix = env_or("AOG_VIRTUAL_KEY_PREFIX", "kv/data/aog/virtual-keys");
    let listen = env_or("AOG_LISTEN", "0.0.0.0:8080");
    let mode_str = env_or("AOG_MODE", "shadow");
    let mode = PolicyMode::parse(&mode_str).ok_or_else(|| format!("bad AOG_MODE '{mode_str}'"))?;

```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Missing AOG_MODE in deployed environment

- **Entry point:** crates/aog-gateway/src/main.rs

- **Outcome:** Regulated payloads can be sent to cloud despite deny decisions

#### Severity

**High** — The path is in scope and crosses a protected product boundary. Regulated payloads can be sent to cloud despite deny decisions. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: Default is shadow and AppState also defaults Shadow. Add a negative regression test that drives Missing AOG_MODE in deployed environment and proves the operation cannot reach Policy violations are logged but never blocked.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-4"></a>

### [4] Legacy OpenAI completions bypass compliance routing and accounting

| Field | Value |
| --- | --- |
| Severity | high |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Policy enforcement bypass |
| CWE | CWE-863 |
| Affected lines | crates/aog-gateway/src/surface_openai.rs:398 |

#### Summary

Authenticated legacy completion request reaches provider.complete at line 430 because Handler performs auth then resolves provider without classify policy tokenize or meter pipeline. The result is Policy bypass regulated egress and unmetered usage.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, Handler performs auth then resolves provider without classify policy tokenize or meter pipeline, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `crates/aog-gateway/src/surface_openai.rs:396-400`

This source implements the control point where Handler performs auth then resolves provider without classify policy tokenize or meter pipeline.

```rust
// ---- /v1/completions (legacy) --------------------------------------------

async fn completions(
    State(state): State<AppState>,
    headers: HeaderMap,
```

#### Validation

The current snapshot confirms Authenticated legacy completion request -\> Handler performs auth then resolves provider without classify policy tokenize or meter pipeline -\> provider.complete at line 430 -\> Policy bypass regulated egress and unmetered usage.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `crates/aog-gateway/src/surface_openai.rs:396-400`

This source implements the control point where Handler performs auth then resolves provider without classify policy tokenize or meter pipeline.

```rust
// ---- /v1/completions (legacy) --------------------------------------------

async fn completions(
    State(state): State<AppState>,
    headers: HeaderMap,
```

#### Dataflow

Authenticated legacy completion request -\> Handler performs auth then resolves provider without classify policy tokenize or meter pipeline -\> provider.complete at line 430 -\> Policy bypass regulated egress and unmetered usage

- **Source:** Authenticated legacy completion request

- **Sink:** provider.complete at line 430

- **Outcome:** Policy bypass regulated egress and unmetered usage

**Broken or missing security control** — `crates/aog-gateway/src/surface_openai.rs:396-400`

This source implements the control point where Handler performs auth then resolves provider without classify policy tokenize or meter pipeline.

```rust
// ---- /v1/completions (legacy) --------------------------------------------

async fn completions(
    State(state): State<AppState>,
    headers: HeaderMap,
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Authenticated legacy completion request

- **Entry point:** crates/aog-gateway/src/surface_openai.rs

- **Outcome:** Policy bypass regulated egress and unmetered usage

#### Severity

**High** — The path is in scope and crosses a protected product boundary. Policy bypass regulated egress and unmetered usage. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: Handler performs auth then resolves provider without classify policy tokenize or meter pipeline. Add a negative regression test that drives Authenticated legacy completion request and proves the operation cannot reach provider.complete at line 430.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-5"></a>

### [5] Appliance composition publishes dev OpenBao with a known root token

| Field | Value |
| --- | --- |
| Severity | high |
| Confidence | medium |
| Confidence rationale | Current source establishes the flaw, but deployment, provider, or operator-mediated preconditions limit exploitability confidence. |
| Category | Hardcoded credentials |
| CWE | CWE-798 |
| Affected lines | deployment/appliance/docker-compose.yml:11 |

#### Summary

Default appliance deployment reaches Host port 8200 is published because Dev mode root token root and 0.0.0.0 listener are configured. The result is Remote takeover of trust signing secrets and policy substrate.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, Dev mode root token root and 0.0.0.0 listener are configured, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `deployment/appliance/docker-compose.yml:9-13`

This source implements the control point where Dev mode root token root and 0.0.0.0 listener are configured.

```yaml
  # ── Trust core ──────────────────────────────────────────────────────────
  openbao:
    image: openbao/openbao:latest
    cap_add: [IPC_LOCK]
    command: server -dev -dev-root-token-id=root -dev-listen-address=0.0.0.0:8200
```

#### Validation

The current snapshot confirms Default appliance deployment -\> Dev mode root token root and 0.0.0.0 listener are configured -\> Host port 8200 is published -\> Remote takeover of trust signing secrets and policy substrate.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `deployment/appliance/docker-compose.yml:9-13`

This source implements the control point where Dev mode root token root and 0.0.0.0 listener are configured.

```yaml
  # ── Trust core ──────────────────────────────────────────────────────────
  openbao:
    image: openbao/openbao:latest
    cap_add: [IPC_LOCK]
    command: server -dev -dev-root-token-id=root -dev-listen-address=0.0.0.0:8200
```

#### Dataflow

Default appliance deployment -\> Dev mode root token root and 0.0.0.0 listener are configured -\> Host port 8200 is published -\> Remote takeover of trust signing secrets and policy substrate

- **Source:** Default appliance deployment

- **Sink:** Host port 8200 is published

- **Outcome:** Remote takeover of trust signing secrets and policy substrate

**Broken or missing security control** — `deployment/appliance/docker-compose.yml:9-13`

This source implements the control point where Dev mode root token root and 0.0.0.0 listener are configured.

```yaml
  # ── Trust core ──────────────────────────────────────────────────────────
  openbao:
    image: openbao/openbao:latest
    cap_add: [IPC_LOCK]
    command: server -dev -dev-root-token-id=root -dev-listen-address=0.0.0.0:8200
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Default appliance deployment

- **Entry point:** deployment/appliance/docker-compose.yml

- **Outcome:** Remote takeover of trust signing secrets and policy substrate

#### Severity

**High** — The path is in scope and crosses a protected product boundary. Remote takeover of trust signing secrets and policy substrate. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: Dev mode root token root and 0.0.0.0 listener are configured. Add a negative regression test that drives Default appliance deployment and proves the operation cannot reach Host port 8200 is published.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-6"></a>

### [6] Model package signature authenticates weights but not manifest identity

| Field | Value |
| --- | --- |
| Severity | high |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Signature verification bypass |
| CWE | CWE-347 |
| Affected lines | mai-core/src/models/verify.rs:120 |

#### Summary

USB package manifest and weights reaches Unsigned manifest supplies model identity compatibility and metadata because verify_signature covers weights bytes only. The result is Valid signed weights can be paired with malicious metadata.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, verify_signature covers weights bytes only, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `mai-core/src/models/verify.rs:118-122`

This source implements the control point where verify_signature covers weights bytes only.

```rust
    vault: &dyn VaultInterface,
) -> Result<bool, VerifyError> {
    let weights = pkg
        .read_weights()
        .map_err(|e| VerifyError::ReadError(e.to_string()))?;
```

#### Validation

The current snapshot confirms USB package manifest and weights -\> verify_signature covers weights bytes only -\> Unsigned manifest supplies model identity compatibility and metadata -\> Valid signed weights can be paired with malicious metadata.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `mai-core/src/models/verify.rs:118-122`

This source implements the control point where verify_signature covers weights bytes only.

```rust
    vault: &dyn VaultInterface,
) -> Result<bool, VerifyError> {
    let weights = pkg
        .read_weights()
        .map_err(|e| VerifyError::ReadError(e.to_string()))?;
```

#### Dataflow

USB package manifest and weights -\> verify_signature covers weights bytes only -\> Unsigned manifest supplies model identity compatibility and metadata -\> Valid signed weights can be paired with malicious metadata

- **Source:** USB package manifest and weights

- **Sink:** Unsigned manifest supplies model identity compatibility and metadata

- **Outcome:** Valid signed weights can be paired with malicious metadata

**Broken or missing security control** — `mai-core/src/models/verify.rs:118-122`

This source implements the control point where verify_signature covers weights bytes only.

```rust
    vault: &dyn VaultInterface,
) -> Result<bool, VerifyError> {
    let weights = pkg
        .read_weights()
        .map_err(|e| VerifyError::ReadError(e.to_string()))?;
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** USB package manifest and weights

- **Entry point:** mai-core/src/models/verify.rs

- **Outcome:** Valid signed weights can be paired with malicious metadata

#### Severity

**High** — The path is in scope and crosses a protected product boundary. Valid signed weights can be paired with malicious metadata. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: verify_signature covers weights bytes only. Add a negative regression test that drives USB package manifest and weights and proves the operation cannot reach Unsigned manifest supplies model identity compatibility and metadata.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-7"></a>

### [7] Attenuation signs attacker-constructed child tokens without authenticating the parent

| Field | Value |
| --- | --- |
| Severity | high |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Signature verification bypass |
| CWE | CWE-347 |
| Affected lines | crates/fabric-token/src/lib.rs:121 |

#### Summary

Caller-supplied parent and complete child token reaches issue(child, signer) at line 170 because Parent signature expiry issuer identity and many authority axes are not verified or constrained. The result is Signing oracle can mint arbitrary identities roles scopes and unbudgeted authority.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, Parent signature expiry issuer identity and many authority axes are not verified or constrained, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `crates/fabric-token/src/lib.rs:119-123`

This source implements the control point where Parent signature expiry issuer identity and many authority axes are not verified or constrained.

```rust
/// # Errors
/// Returns [`TokenError::AttenuationWidens`] on any widening, or a signing error.
pub fn attenuate(
    parent: &TrustToken,
    mut child: TrustToken,
```

#### Validation

The current snapshot confirms Caller-supplied parent and complete child token -\> Parent signature expiry issuer identity and many authority axes are not verified or constrained -\> issue(child, signer) at line 170 -\> Signing oracle can mint arbitrary identities roles scopes and unbudgeted authority.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `crates/fabric-token/src/lib.rs:119-123`

This source implements the control point where Parent signature expiry issuer identity and many authority axes are not verified or constrained.

```rust
/// # Errors
/// Returns [`TokenError::AttenuationWidens`] on any widening, or a signing error.
pub fn attenuate(
    parent: &TrustToken,
    mut child: TrustToken,
```

#### Dataflow

Caller-supplied parent and complete child token -\> Parent signature expiry issuer identity and many authority axes are not verified or constrained -\> issue(child, signer) at line 170 -\> Signing oracle can mint arbitrary identities roles scopes and unbudgeted authority

- **Source:** Caller-supplied parent and complete child token

- **Sink:** issue(child, signer) at line 170

- **Outcome:** Signing oracle can mint arbitrary identities roles scopes and unbudgeted authority

**Broken or missing security control** — `crates/fabric-token/src/lib.rs:119-123`

This source implements the control point where Parent signature expiry issuer identity and many authority axes are not verified or constrained.

```rust
/// # Errors
/// Returns [`TokenError::AttenuationWidens`] on any widening, or a signing error.
pub fn attenuate(
    parent: &TrustToken,
    mut child: TrustToken,
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Caller-supplied parent and complete child token

- **Entry point:** crates/fabric-token/src/lib.rs

- **Outcome:** Signing oracle can mint arbitrary identities roles scopes and unbudgeted authority

#### Severity

**High** — The path is in scope and crosses a protected product boundary. Signing oracle can mint arbitrary identities roles scopes and unbudgeted authority. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: Parent signature expiry issuer identity and many authority axes are not verified or constrained. Add a negative regression test that drives Caller-supplied parent and complete child token and proves the operation cannot reach issue(child, signer) at line 170.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-8"></a>

### [8] Envelope unseal is not bound to tenant subject audience or policy

| Field | Value |
| --- | --- |
| Severity | high |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Cross-tenant authorization bypass |
| CWE | CWE-639 |
| Affected lines | crates/wsf-seal/src/lib.rs:305 |

#### Summary

Any valid token and another envelope reaches OpenBao decrypt then fabric_envelope open because Only signature expiry classification and permitted-op checks. The result is Cross-tenant or cross-service plaintext disclosure.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, Only signature expiry classification and permitted-op checks, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `crates/wsf-seal/src/lib.rs:303-307`

This source implements the control point where Only signature expiry classification and permitted-op checks.

```rust
    /// lacks clearance, or the label forbids unseal; an OpenBao or envelope error
    /// otherwise. A denial is receipted before returning.
    pub async fn unseal(
        &self,
        req: UnsealRequest,
```

#### Validation

The current snapshot confirms Any valid token and another envelope -\> Only signature expiry classification and permitted-op checks -\> OpenBao decrypt then fabric_envelope open -\> Cross-tenant or cross-service plaintext disclosure.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `crates/wsf-seal/src/lib.rs:303-307`

This source implements the control point where Only signature expiry classification and permitted-op checks.

```rust
    /// lacks clearance, or the label forbids unseal; an OpenBao or envelope error
    /// otherwise. A denial is receipted before returning.
    pub async fn unseal(
        &self,
        req: UnsealRequest,
```

#### Dataflow

Any valid token and another envelope -\> Only signature expiry classification and permitted-op checks -\> OpenBao decrypt then fabric_envelope open -\> Cross-tenant or cross-service plaintext disclosure

- **Source:** Any valid token and another envelope

- **Sink:** OpenBao decrypt then fabric_envelope open

- **Outcome:** Cross-tenant or cross-service plaintext disclosure

**Broken or missing security control** — `crates/wsf-seal/src/lib.rs:303-307`

This source implements the control point where Only signature expiry classification and permitted-op checks.

```rust
    /// lacks clearance, or the label forbids unseal; an OpenBao or envelope error
    /// otherwise. A denial is receipted before returning.
    pub async fn unseal(
        &self,
        req: UnsealRequest,
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Any valid token and another envelope

- **Entry point:** crates/wsf-seal/src/lib.rs

- **Outcome:** Cross-tenant or cross-service plaintext disclosure

#### Severity

**High** — The path is in scope and crosses a protected product boundary. Cross-tenant or cross-service plaintext disclosure. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: Only signature expiry classification and permitted-op checks. Add a negative regression test that drives Any valid token and another envelope and proves the operation cannot reach OpenBao decrypt then fabric_envelope open.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-9"></a>

### [9] gRPC trusts caller-authored administrator metadata

| Field | Value |
| --- | --- |
| Severity | high |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Authentication bypass |
| CWE | CWE-290 |
| Affected lines | mai-api/src/grpc/mod.rs:98 |

#### Summary

x-im-profile metadata reaches role_has_permission treats caller string admin as privileged because Role is parsed directly from untrusted metadata. The result is Remote privileged model power registry and audit RPC access.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, Role is parsed directly from untrusted metadata, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `mai-api/src/grpc/mod.rs:96-100`

This source implements the control point where Role is parsed directly from untrusted metadata.

```rust
/// Returns (profile_id, role_string) or a Status error.
#[allow(clippy::result_large_err)]
pub fn extract_grpc_profile<T>(request: &Request<T>) -> Result<(String, String), Status> {
    let metadata = request.metadata();
    let header_value = metadata
```

#### Validation

The current snapshot confirms x-im-profile metadata -\> Role is parsed directly from untrusted metadata -\> role_has_permission treats caller string admin as privileged -\> Remote privileged model power registry and audit RPC access.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `mai-api/src/grpc/mod.rs:96-100`

This source implements the control point where Role is parsed directly from untrusted metadata.

```rust
/// Returns (profile_id, role_string) or a Status error.
#[allow(clippy::result_large_err)]
pub fn extract_grpc_profile<T>(request: &Request<T>) -> Result<(String, String), Status> {
    let metadata = request.metadata();
    let header_value = metadata
```

#### Dataflow

x-im-profile metadata -\> Role is parsed directly from untrusted metadata -\> role_has_permission treats caller string admin as privileged -\> Remote privileged model power registry and audit RPC access

- **Source:** x-im-profile metadata

- **Sink:** role_has_permission treats caller string admin as privileged

- **Outcome:** Remote privileged model power registry and audit RPC access

**Broken or missing security control** — `mai-api/src/grpc/mod.rs:96-100`

This source implements the control point where Role is parsed directly from untrusted metadata.

```rust
/// Returns (profile_id, role_string) or a Status error.
#[allow(clippy::result_large_err)]
pub fn extract_grpc_profile<T>(request: &Request<T>) -> Result<(String, String), Status> {
    let metadata = request.metadata();
    let header_value = metadata
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** x-im-profile metadata

- **Entry point:** mai-api/src/grpc/mod.rs

- **Outcome:** Remote privileged model power registry and audit RPC access

#### Severity

**High** — The path is in scope and crosses a protected product boundary. Remote privileged model power registry and audit RPC access. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: Role is parsed directly from untrusted metadata. Add a negative regression test that drives x-im-profile metadata and proves the operation cannot reach role_has_permission treats caller string admin as privileged.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-10"></a>

### [10] OpenAI streaming bypasses egress tokenization metering and receipts

| Field | Value |
| --- | --- |
| Severity | high |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Policy enforcement bypass |
| CWE | CWE-863 |
| Affected lines | crates/aog-gateway/src/surface_openai.rs:176 |

#### Summary

Authenticated streaming chat request reaches provider.stream receives neutral plaintext directly because Policy gate runs but shared tokenization and settlement stages are skipped. The result is Regulated content egress and unmetered spend.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, Policy gate runs but shared tokenization and settlement stages are skipped, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `crates/aog-gateway/src/surface_openai.rs:174-178`

This source implements the control point where Policy gate runs but shared tokenization and settlement stages are skipped.

```rust
    let mut tokenized_spans = 0u32;
    let resp = if body.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        match provider.stream(&neutral).await {
            Ok(chunks) => chat_sse(inbound_model, chunks),
            Err(e) => provider_http(&e).into_response(),
```

#### Validation

The current snapshot confirms Authenticated streaming chat request -\> Policy gate runs but shared tokenization and settlement stages are skipped -\> provider.stream receives neutral plaintext directly -\> Regulated content egress and unmetered spend.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `crates/aog-gateway/src/surface_openai.rs:174-178`

This source implements the control point where Policy gate runs but shared tokenization and settlement stages are skipped.

```rust
    let mut tokenized_spans = 0u32;
    let resp = if body.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        match provider.stream(&neutral).await {
            Ok(chunks) => chat_sse(inbound_model, chunks),
            Err(e) => provider_http(&e).into_response(),
```

#### Dataflow

Authenticated streaming chat request -\> Policy gate runs but shared tokenization and settlement stages are skipped -\> provider.stream receives neutral plaintext directly -\> Regulated content egress and unmetered spend

- **Source:** Authenticated streaming chat request

- **Sink:** provider.stream receives neutral plaintext directly

- **Outcome:** Regulated content egress and unmetered spend

**Broken or missing security control** — `crates/aog-gateway/src/surface_openai.rs:174-178`

This source implements the control point where Policy gate runs but shared tokenization and settlement stages are skipped.

```rust
    let mut tokenized_spans = 0u32;
    let resp = if body.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        match provider.stream(&neutral).await {
            Ok(chunks) => chat_sse(inbound_model, chunks),
            Err(e) => provider_http(&e).into_response(),
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Authenticated streaming chat request

- **Entry point:** crates/aog-gateway/src/surface_openai.rs

- **Outcome:** Regulated content egress and unmetered spend

#### Severity

**High** — The path is in scope and crosses a protected product boundary. Regulated content egress and unmetered spend. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: Policy gate runs but shared tokenization and settlement stages are skipped. Add a negative regression test that drives Authenticated streaming chat request and proves the operation cannot reach provider.stream receives neutral plaintext directly.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-11"></a>

### [11] Anthropic streaming bypasses egress tokenization metering and receipts

| Field | Value |
| --- | --- |
| Severity | high |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Policy enforcement bypass |
| CWE | CWE-863 |
| Affected lines | crates/aog-gateway/src/surface_anthropic.rs:133 |

#### Summary

Authenticated streaming messages request reaches provider.stream receives neutral plaintext directly because Tokenization metering receipt and budget settlement are skipped. The result is Regulated content egress and unmetered spend.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, Tokenization metering receipt and budget settlement are skipped, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `crates/aog-gateway/src/surface_anthropic.rs:131-135`

This source implements the control point where Tokenization metering receipt and budget settlement are skipped.

```rust
    let mut tokenized_spans = 0u32;
    let resp = if body.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        match provider.stream(&neutral).await {
            Ok(chunks) => messages_sse(inbound_model, chunks),
            Err(e) => provider_http(&e).into_response(),
```

#### Validation

The current snapshot confirms Authenticated streaming messages request -\> Tokenization metering receipt and budget settlement are skipped -\> provider.stream receives neutral plaintext directly -\> Regulated content egress and unmetered spend.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `crates/aog-gateway/src/surface_anthropic.rs:131-135`

This source implements the control point where Tokenization metering receipt and budget settlement are skipped.

```rust
    let mut tokenized_spans = 0u32;
    let resp = if body.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        match provider.stream(&neutral).await {
            Ok(chunks) => messages_sse(inbound_model, chunks),
            Err(e) => provider_http(&e).into_response(),
```

#### Dataflow

Authenticated streaming messages request -\> Tokenization metering receipt and budget settlement are skipped -\> provider.stream receives neutral plaintext directly -\> Regulated content egress and unmetered spend

- **Source:** Authenticated streaming messages request

- **Sink:** provider.stream receives neutral plaintext directly

- **Outcome:** Regulated content egress and unmetered spend

**Broken or missing security control** — `crates/aog-gateway/src/surface_anthropic.rs:131-135`

This source implements the control point where Tokenization metering receipt and budget settlement are skipped.

```rust
    let mut tokenized_spans = 0u32;
    let resp = if body.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        match provider.stream(&neutral).await {
            Ok(chunks) => messages_sse(inbound_model, chunks),
            Err(e) => provider_http(&e).into_response(),
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Authenticated streaming messages request

- **Entry point:** crates/aog-gateway/src/surface_anthropic.rs

- **Outcome:** Regulated content egress and unmetered spend

#### Severity

**High** — The path is in scope and crosses a protected product boundary. Regulated content egress and unmetered spend. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: Tokenization metering receipt and budget settlement are skipped. Add a negative regression test that drives Authenticated streaming messages request and proves the operation cannot reach provider.stream receives neutral plaintext directly.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-12"></a>

### [12] Restore accepts unsigned or unverified manifests by default

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | medium |
| Confidence rationale | Current source establishes the flaw, but deployment, provider, or operator-mediated preconditions limit exploitability confidence. |
| Category | Signature verification bypass |
| CWE | CWE-347 |
| Affected lines | tools/mai-admin/src/main.rs:123 |

#### Summary

Operator-supplied backup manifest reaches Restore plan uses untrusted manifest paths and digests because require_signed defaults false and a signed manifest may be accepted without verification key. The result is Tampered backup metadata can drive privileged restore.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, require_signed defaults false and a signed manifest may be accepted without verification key, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `tools/mai-admin/src/main.rs:121-125`

This source implements the control point where require_signed defaults false and a signed manifest may be accepted without verification key.

```rust
        verifying_key: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        require_signed: bool,
        /// Overwrite existing files / populated directory trees inside
        /// the target. Refuse to operate on a non-empty target without
```

#### Validation

The current snapshot confirms Operator-supplied backup manifest -\> require_signed defaults false and a signed manifest may be accepted without verification key -\> Restore plan uses untrusted manifest paths and digests -\> Tampered backup metadata can drive privileged restore.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `tools/mai-admin/src/main.rs:121-125`

This source implements the control point where require_signed defaults false and a signed manifest may be accepted without verification key.

```rust
        verifying_key: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        require_signed: bool,
        /// Overwrite existing files / populated directory trees inside
        /// the target. Refuse to operate on a non-empty target without
```

#### Dataflow

Operator-supplied backup manifest -\> require_signed defaults false and a signed manifest may be accepted without verification key -\> Restore plan uses untrusted manifest paths and digests -\> Tampered backup metadata can drive privileged restore

- **Source:** Operator-supplied backup manifest

- **Sink:** Restore plan uses untrusted manifest paths and digests

- **Outcome:** Tampered backup metadata can drive privileged restore

**Broken or missing security control** — `tools/mai-admin/src/main.rs:121-125`

This source implements the control point where require_signed defaults false and a signed manifest may be accepted without verification key.

```rust
        verifying_key: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        require_signed: bool,
        /// Overwrite existing files / populated directory trees inside
        /// the target. Refuse to operate on a non-empty target without
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Operator-supplied backup manifest

- **Entry point:** tools/mai-admin/src/main.rs

- **Outcome:** Tampered backup metadata can drive privileged restore

#### Severity

**Medium** — The path is in scope and crosses a protected product boundary. Tampered backup metadata can drive privileged restore. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: require_signed defaults false and a signed manifest may be accepted without verification key. Add a negative regression test that drives Operator-supplied backup manifest and proves the operation cannot reach Restore plan uses untrusted manifest paths and digests.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-13"></a>

### [13] Manifest-derived model ID can escape the vault root

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Path traversal |
| CWE | CWE-22 |
| Affected lines | mai-vault/src/zfs.rs:275 |

#### Summary

Unsigned manifest model name version and quantization reaches create_dir_all and weights write under joined path because model_id is free-form and joined without containment. The result is Arbitrary filesystem write with model bytes.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, model_id is free-form and joined without containment, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `mai-vault/src/zfs.rs:273-277`

This source implements the control point where model_id is free-form and joined without containment.

```rust
        }

        let model_dir = self.config.storage.mount_point.join(model_id);
        let weights_path = model_dir.join("weights.bin");
        let manifest_path = model_dir.join("manifest.json");
```

#### Validation

The current snapshot confirms Unsigned manifest model name version and quantization -\> model_id is free-form and joined without containment -\> create_dir_all and weights write under joined path -\> Arbitrary filesystem write with model bytes.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `mai-vault/src/zfs.rs:273-277`

This source implements the control point where model_id is free-form and joined without containment.

```rust
        }

        let model_dir = self.config.storage.mount_point.join(model_id);
        let weights_path = model_dir.join("weights.bin");
        let manifest_path = model_dir.join("manifest.json");
```

#### Dataflow

Unsigned manifest model name version and quantization -\> model_id is free-form and joined without containment -\> create_dir_all and weights write under joined path -\> Arbitrary filesystem write with model bytes

- **Source:** Unsigned manifest model name version and quantization

- **Sink:** create_dir_all and weights write under joined path

- **Outcome:** Arbitrary filesystem write with model bytes

**Broken or missing security control** — `mai-vault/src/zfs.rs:273-277`

This source implements the control point where model_id is free-form and joined without containment.

```rust
        }

        let model_dir = self.config.storage.mount_point.join(model_id);
        let weights_path = model_dir.join("weights.bin");
        let manifest_path = model_dir.join("manifest.json");
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Unsigned manifest model name version and quantization

- **Entry point:** mai-vault/src/zfs.rs

- **Outcome:** Arbitrary filesystem write with model bytes

#### Severity

**Medium** — The path is in scope and crosses a protected product boundary. Arbitrary filesystem write with model bytes. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: model_id is free-form and joined without containment. Add a negative regression test that drives Unsigned manifest model name version and quantization and proves the operation cannot reach create_dir_all and weights write under joined path.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-14"></a>

### [14] AOG revocation check fails open when snapshot is absent

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Fail-open security control |
| CWE | CWE-636 |
| Affected lines | crates/aog-gateway/src/lib.rs:188 |

#### Summary

Missing or deleted OpenBao revocation record reaches Request continues after line 219 because NotFound is treated as no revocations. The result is Privileged requests proceed during revocation-state loss.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, NotFound is treated as no revocations, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `crates/aog-gateway/src/lib.rs:186-190`

This source implements the control point where NotFound is treated as no revocations.

```rust
        }

        // Kill switch (G9): consult the signed revocation snapshot. A revoked token
        // or subject halts the session's next call. No snapshot at the path = nothing
        // revoked (fail-open on absence); a present-but-invalid snapshot fails closed.
```

#### Validation

The current snapshot confirms Missing or deleted OpenBao revocation record -\> NotFound is treated as no revocations -\> Request continues after line 219 -\> Privileged requests proceed during revocation-state loss.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `crates/aog-gateway/src/lib.rs:186-190`

This source implements the control point where NotFound is treated as no revocations.

```rust
        }

        // Kill switch (G9): consult the signed revocation snapshot. A revoked token
        // or subject halts the session's next call. No snapshot at the path = nothing
        // revoked (fail-open on absence); a present-but-invalid snapshot fails closed.
```

#### Dataflow

Missing or deleted OpenBao revocation record -\> NotFound is treated as no revocations -\> Request continues after line 219 -\> Privileged requests proceed during revocation-state loss

- **Source:** Missing or deleted OpenBao revocation record

- **Sink:** Request continues after line 219

- **Outcome:** Privileged requests proceed during revocation-state loss

**Broken or missing security control** — `crates/aog-gateway/src/lib.rs:186-190`

This source implements the control point where NotFound is treated as no revocations.

```rust
        }

        // Kill switch (G9): consult the signed revocation snapshot. A revoked token
        // or subject halts the session's next call. No snapshot at the path = nothing
        // revoked (fail-open on absence); a present-but-invalid snapshot fails closed.
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Missing or deleted OpenBao revocation record

- **Entry point:** crates/aog-gateway/src/lib.rs

- **Outcome:** Privileged requests proceed during revocation-state loss

#### Severity

**Medium** — The path is in scope and crosses a protected product boundary. Privileged requests proceed during revocation-state loss. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: NotFound is treated as no revocations. Add a negative regression test that drives Missing or deleted OpenBao revocation record and proves the operation cannot reach Request continues after line 219.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-15"></a>

### [15] ROI endpoint computes recommendations from every tenant

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Cross-tenant authorization bypass |
| CWE | CWE-639 |
| Affected lines | crates/aog-gateway/src/surface_openai.rs:259 |

#### Summary

Any authenticated virtual key reaches Global aggregates feed recommendation because Resolved tenant is discarded. The result is Cross-tenant spend inference and estate-information disclosure.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, Resolved tenant is discarded, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `crates/aog-gateway/src/surface_openai.rs:257-261`

This source implements the control point where Resolved tenant is discarded.

```rust
/// the metered spend (aog-meter aggregates). `?summit_cost_cents=&window_days=`
/// override the appliance cost + window. Authenticated (like `/v1/usage`).
async fn roi(
    State(state): State<AppState>,
    headers: HeaderMap,
```

#### Validation

The current snapshot confirms Any authenticated virtual key -\> Resolved tenant is discarded -\> Global aggregates feed recommendation -\> Cross-tenant spend inference and estate-information disclosure.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `crates/aog-gateway/src/surface_openai.rs:257-261`

This source implements the control point where Resolved tenant is discarded.

```rust
/// the metered spend (aog-meter aggregates). `?summit_cost_cents=&window_days=`
/// override the appliance cost + window. Authenticated (like `/v1/usage`).
async fn roi(
    State(state): State<AppState>,
    headers: HeaderMap,
```

#### Dataflow

Any authenticated virtual key -\> Resolved tenant is discarded -\> Global aggregates feed recommendation -\> Cross-tenant spend inference and estate-information disclosure

- **Source:** Any authenticated virtual key

- **Sink:** Global aggregates feed recommendation

- **Outcome:** Cross-tenant spend inference and estate-information disclosure

**Broken or missing security control** — `crates/aog-gateway/src/surface_openai.rs:257-261`

This source implements the control point where Resolved tenant is discarded.

```rust
/// the metered spend (aog-meter aggregates). `?summit_cost_cents=&window_days=`
/// override the appliance cost + window. Authenticated (like `/v1/usage`).
async fn roi(
    State(state): State<AppState>,
    headers: HeaderMap,
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Any authenticated virtual key

- **Entry point:** crates/aog-gateway/src/surface_openai.rs

- **Outcome:** Cross-tenant spend inference and estate-information disclosure

#### Severity

**Medium** — The path is in scope and crosses a protected product boundary. Cross-tenant spend inference and estate-information disclosure. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: Resolved tenant is discarded. Add a negative regression test that drives Any authenticated virtual key and proves the operation cannot reach Global aggregates feed recommendation.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-16"></a>

### [16] Restore manifest component paths can escape backup and target roots

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Path traversal |
| CWE | CWE-22 |
| Affected lines | tools/mai-admin/src/restore.rs:292 |

#### Summary

Manifest component.path reaches backup_dir.join and target_dir.join feed recursive copy because No relative-path validation canonical containment or no-follow enforcement. The result is Arbitrary file read and write during privileged restore.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, No relative-path validation canonical containment or no-follow enforcement, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `tools/mai-admin/src/restore.rs:290-294`

This source implements the control point where No relative-path validation canonical containment or no-follow enforcement.

```rust
    let mut actions: Vec<RestoreAction> = Vec::with_capacity(manifest.components.len());
    for component in &manifest.components {
        let source_abs = backup_dir.join(&component.path);
        if !source_abs.exists() {
            return Err(RestoreError::SourceMissing(component.name.clone()));
```

#### Validation

The current snapshot confirms Manifest component.path -\> No relative-path validation canonical containment or no-follow enforcement -\> backup_dir.join and target_dir.join feed recursive copy -\> Arbitrary file read and write during privileged restore.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `tools/mai-admin/src/restore.rs:290-294`

This source implements the control point where No relative-path validation canonical containment or no-follow enforcement.

```rust
    let mut actions: Vec<RestoreAction> = Vec::with_capacity(manifest.components.len());
    for component in &manifest.components {
        let source_abs = backup_dir.join(&component.path);
        if !source_abs.exists() {
            return Err(RestoreError::SourceMissing(component.name.clone()));
```

#### Dataflow

Manifest component.path -\> No relative-path validation canonical containment or no-follow enforcement -\> backup_dir.join and target_dir.join feed recursive copy -\> Arbitrary file read and write during privileged restore

- **Source:** Manifest component.path

- **Sink:** backup_dir.join and target_dir.join feed recursive copy

- **Outcome:** Arbitrary file read and write during privileged restore

**Broken or missing security control** — `tools/mai-admin/src/restore.rs:290-294`

This source implements the control point where No relative-path validation canonical containment or no-follow enforcement.

```rust
    let mut actions: Vec<RestoreAction> = Vec::with_capacity(manifest.components.len());
    for component in &manifest.components {
        let source_abs = backup_dir.join(&component.path);
        if !source_abs.exists() {
            return Err(RestoreError::SourceMissing(component.name.clone()));
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Manifest component.path

- **Entry point:** tools/mai-admin/src/restore.rs

- **Outcome:** Arbitrary file read and write during privileged restore

#### Severity

**Medium** — The path is in scope and crosses a protected product boundary. Arbitrary file read and write during privileged restore. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: No relative-path validation canonical containment or no-follow enforcement. Add a negative regression test that drives Manifest component.path and proves the operation cannot reach backup_dir.join and target_dir.join feed recursive copy.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-17"></a>

### [17] Production readiness certifies a merely constructed vault

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Protection mechanism failure |
| CWE | CWE-693 |
| Affected lines | mai-api/src/server.rs:689 |

#### Summary

Ship profile and filesystem configuration reaches Production readiness may allow listener bind because RuntimeOutcome passes from constructor/open path only. The result is Production can expose plaintext or uninitialized vault controls.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, RuntimeOutcome passes from constructor/open path only, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `mai-api/src/server.rs:687-691`

This source implements the control point where RuntimeOutcome passes from constructor/open path only.

```rust
    });

    let vault_outcome = RuntimeOutcome::pass(format!(
        "{:?} vault opened at {}",
        profile.vault.backend,
```

#### Validation

The current snapshot confirms Ship profile and filesystem configuration -\> RuntimeOutcome passes from constructor/open path only -\> Production readiness may allow listener bind -\> Production can expose plaintext or uninitialized vault controls.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `mai-api/src/server.rs:687-691`

This source implements the control point where RuntimeOutcome passes from constructor/open path only.

```rust
    });

    let vault_outcome = RuntimeOutcome::pass(format!(
        "{:?} vault opened at {}",
        profile.vault.backend,
```

#### Dataflow

Ship profile and filesystem configuration -\> RuntimeOutcome passes from constructor/open path only -\> Production readiness may allow listener bind -\> Production can expose plaintext or uninitialized vault controls

- **Source:** Ship profile and filesystem configuration

- **Sink:** Production readiness may allow listener bind

- **Outcome:** Production can expose plaintext or uninitialized vault controls

**Broken or missing security control** — `mai-api/src/server.rs:687-691`

This source implements the control point where RuntimeOutcome passes from constructor/open path only.

```rust
    });

    let vault_outcome = RuntimeOutcome::pass(format!(
        "{:?} vault opened at {}",
        profile.vault.backend,
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Ship profile and filesystem configuration

- **Entry point:** mai-api/src/server.rs

- **Outcome:** Production can expose plaintext or uninitialized vault controls

#### Severity

**Medium** — The path is in scope and crosses a protected product boundary. Production can expose plaintext or uninitialized vault controls. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: RuntimeOutcome passes from constructor/open path only. Add a negative regression test that drives Ship profile and filesystem configuration and proves the operation cannot reach Production readiness may allow listener bind.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-18"></a>

### [18] Usage endpoint returns aggregates for every tenant

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Cross-tenant authorization bypass |
| CWE | CWE-639 |
| Affected lines | crates/aog-gateway/src/surface_openai.rs:233 |

#### Summary

Any authenticated virtual key reaches ReceiptLedger.aggregate returns all tenants because Resolved tenant is discarded. The result is Cross-tenant provider model workflow and spend metadata disclosure.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, Resolved tenant is discarded, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `crates/aog-gateway/src/surface_openai.rs:231-235`

This source implements the control point where Resolved tenant is discarded.

```rust
/// `GET /v1/usage` — aog-meter aggregates (per tenant/provider/model/task) +
/// the receipt-chain head + a live chain-verify. Authenticated.
async fn usage(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(e) = authorize(&state, &headers).await {
        return e.into_response();
```

#### Validation

The current snapshot confirms Any authenticated virtual key -\> Resolved tenant is discarded -\> ReceiptLedger.aggregate returns all tenants -\> Cross-tenant provider model workflow and spend metadata disclosure.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `crates/aog-gateway/src/surface_openai.rs:231-235`

This source implements the control point where Resolved tenant is discarded.

```rust
/// `GET /v1/usage` — aog-meter aggregates (per tenant/provider/model/task) +
/// the receipt-chain head + a live chain-verify. Authenticated.
async fn usage(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(e) = authorize(&state, &headers).await {
        return e.into_response();
```

#### Dataflow

Any authenticated virtual key -\> Resolved tenant is discarded -\> ReceiptLedger.aggregate returns all tenants -\> Cross-tenant provider model workflow and spend metadata disclosure

- **Source:** Any authenticated virtual key

- **Sink:** ReceiptLedger.aggregate returns all tenants

- **Outcome:** Cross-tenant provider model workflow and spend metadata disclosure

**Broken or missing security control** — `crates/aog-gateway/src/surface_openai.rs:231-235`

This source implements the control point where Resolved tenant is discarded.

```rust
/// `GET /v1/usage` — aog-meter aggregates (per tenant/provider/model/task) +
/// the receipt-chain head + a live chain-verify. Authenticated.
async fn usage(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(e) = authorize(&state, &headers).await {
        return e.into_response();
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Any authenticated virtual key

- **Entry point:** crates/aog-gateway/src/surface_openai.rs

- **Outcome:** Cross-tenant provider model workflow and spend metadata disclosure

#### Severity

**Medium** — The path is in scope and crosses a protected product boundary. Cross-tenant provider model workflow and spend metadata disclosure. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: Resolved tenant is discarded. Add a negative regression test that drives Any authenticated virtual key and proves the operation cannot reach ReceiptLedger.aggregate returns all tenants.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-19"></a>

### [19] AWS credentials can outlive remaining WSF token authority

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | medium |
| Confidence rationale | Current source establishes the flaw, but deployment, provider, or operator-mediated preconditions limit exploitability confidence. |
| Category | Credential lifetime mismatch |
| CWE | CWE-613 |
| Affected lines | crates/wsf-broker/src/lib.rs:245 |

#### Summary

Near-expiry valid token reaches STS credential minted for longer than token lifetime because Remaining TTL is clamped upward to provider 900-second minimum. The result is Cloud access survives capability expiry or revocation window.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, Remaining TTL is clamped upward to provider 900-second minimum, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `crates/wsf-broker/src/lib.rs:243-247`

This source implements the control point where Remaining TTL is clamped upward to provider 900-second minimum.

```rust
    };
    use fabric_crypto::Signer;
    use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};

    fn token_with(caveats: Vec<Caveat>, expires_at: &str) -> TrustToken {
```

#### Validation

The current snapshot confirms Near-expiry valid token -\> Remaining TTL is clamped upward to provider 900-second minimum -\> STS credential minted for longer than token lifetime -\> Cloud access survives capability expiry or revocation window.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `crates/wsf-broker/src/lib.rs:243-247`

This source implements the control point where Remaining TTL is clamped upward to provider 900-second minimum.

```rust
    };
    use fabric_crypto::Signer;
    use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};

    fn token_with(caveats: Vec<Caveat>, expires_at: &str) -> TrustToken {
```

#### Dataflow

Near-expiry valid token -\> Remaining TTL is clamped upward to provider 900-second minimum -\> STS credential minted for longer than token lifetime -\> Cloud access survives capability expiry or revocation window

- **Source:** Near-expiry valid token

- **Sink:** STS credential minted for longer than token lifetime

- **Outcome:** Cloud access survives capability expiry or revocation window

**Broken or missing security control** — `crates/wsf-broker/src/lib.rs:243-247`

This source implements the control point where Remaining TTL is clamped upward to provider 900-second minimum.

```rust
    };
    use fabric_crypto::Signer;
    use fabric_crypto::providers::{MlDsa87Verifier, RustCryptoMlDsa87};

    fn token_with(caveats: Vec<Caveat>, expires_at: &str) -> TrustToken {
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Near-expiry valid token

- **Entry point:** crates/wsf-broker/src/lib.rs

- **Outcome:** Cloud access survives capability expiry or revocation window

#### Severity

**Medium** — The path is in scope and crosses a protected product boundary. Cloud access survives capability expiry or revocation window. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: Remaining TTL is clamped upward to provider 900-second minimum. Add a negative regression test that drives Near-expiry valid token and proves the operation cannot reach STS credential minted for longer than token lifetime.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-20"></a>

### [20] WSF receipt queries are unauthenticated and cross-tenant

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Missing authorization |
| CWE | CWE-862 |
| Affected lines | crates/wsf-api/src/lib.rs:361 |

#### Summary

Anonymous query field and value reaches Ledger query or full entries response because No auth tenant filter pagination or field allowlist. The result is Cross-tenant evidence and correlation metadata disclosure.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, No auth tenant filter pagination or field allowlist, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `crates/wsf-api/src/lib.rs:359-363`

This source implements the control point where No auth tenant filter pagination or field allowlist.

```rust
}

async fn receipts(State(s): State<AppState>, Query(q): Query<ReceiptsQuery>) -> Json<ReceiptsResp> {
    let ledger = s.ledger.lock().expect("ledger lock");
    let entries = match (q.field, q.value) {
```

#### Validation

The current snapshot confirms Anonymous query field and value -\> No auth tenant filter pagination or field allowlist -\> Ledger query or full entries response -\> Cross-tenant evidence and correlation metadata disclosure.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `crates/wsf-api/src/lib.rs:359-363`

This source implements the control point where No auth tenant filter pagination or field allowlist.

```rust
}

async fn receipts(State(s): State<AppState>, Query(q): Query<ReceiptsQuery>) -> Json<ReceiptsResp> {
    let ledger = s.ledger.lock().expect("ledger lock");
    let entries = match (q.field, q.value) {
```

#### Dataflow

Anonymous query field and value -\> No auth tenant filter pagination or field allowlist -\> Ledger query or full entries response -\> Cross-tenant evidence and correlation metadata disclosure

- **Source:** Anonymous query field and value

- **Sink:** Ledger query or full entries response

- **Outcome:** Cross-tenant evidence and correlation metadata disclosure

**Broken or missing security control** — `crates/wsf-api/src/lib.rs:359-363`

This source implements the control point where No auth tenant filter pagination or field allowlist.

```rust
}

async fn receipts(State(s): State<AppState>, Query(q): Query<ReceiptsQuery>) -> Json<ReceiptsResp> {
    let ledger = s.ledger.lock().expect("ledger lock");
    let entries = match (q.field, q.value) {
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Anonymous query field and value

- **Entry point:** crates/wsf-api/src/lib.rs

- **Outcome:** Cross-tenant evidence and correlation metadata disclosure

#### Severity

**Medium** — The path is in scope and crosses a protected product boundary. Cross-tenant evidence and correlation metadata disclosure. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: No auth tenant filter pagination or field allowlist. Add a negative regression test that drives Anonymous query field and value and proves the operation cannot reach Ledger query or full entries response.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-21"></a>

### [21] Vault snapshot and rollback APIs report success without ZFS operations

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Protection mechanism failure |
| CWE | CWE-693 |
| Affected lines | mai-vault/src/zfs.rs:453 |

#### Summary

Privileged backup rollback requests reaches create_snapshot and rollback_snapshot return success because Methods mutate only in-memory metadata. The result is False recovery guarantees and irreversible production data loss.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, Methods mutate only in-memory metadata, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `mai-vault/src/zfs.rs:451-455`

This source implements the control point where Methods mutate only in-memory metadata.

```rust
    }

    async fn create_snapshot(&self, reason: &str) -> Result<SnapshotInfo, VaultError> {
        let name = format!("mai-snap-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
        #[allow(clippy::cast_sign_loss)] // Timestamp is always positive after epoch
```

#### Validation

The current snapshot confirms Privileged backup rollback requests -\> Methods mutate only in-memory metadata -\> create_snapshot and rollback_snapshot return success -\> False recovery guarantees and irreversible production data loss.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `mai-vault/src/zfs.rs:451-455`

This source implements the control point where Methods mutate only in-memory metadata.

```rust
    }

    async fn create_snapshot(&self, reason: &str) -> Result<SnapshotInfo, VaultError> {
        let name = format!("mai-snap-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
        #[allow(clippy::cast_sign_loss)] // Timestamp is always positive after epoch
```

#### Dataflow

Privileged backup rollback requests -\> Methods mutate only in-memory metadata -\> create_snapshot and rollback_snapshot return success -\> False recovery guarantees and irreversible production data loss

- **Source:** Privileged backup rollback requests

- **Sink:** create_snapshot and rollback_snapshot return success

- **Outcome:** False recovery guarantees and irreversible production data loss

**Broken or missing security control** — `mai-vault/src/zfs.rs:451-455`

This source implements the control point where Methods mutate only in-memory metadata.

```rust
    }

    async fn create_snapshot(&self, reason: &str) -> Result<SnapshotInfo, VaultError> {
        let name = format!("mai-snap-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
        #[allow(clippy::cast_sign_loss)] // Timestamp is always positive after epoch
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Privileged backup rollback requests

- **Entry point:** mai-vault/src/zfs.rs

- **Outcome:** False recovery guarantees and irreversible production data loss

#### Severity

**Medium** — The path is in scope and crosses a protected product boundary. False recovery guarantees and irreversible production data loss. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: Methods mutate only in-memory metadata. Add a negative regression test that drives Privileged backup rollback requests and proves the operation cannot reach create_snapshot and rollback_snapshot return success.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-22"></a>

### [22] ZFS vault stores and loads model weights as plaintext

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Missing encryption at rest |
| CWE | CWE-311 |
| Affected lines | mai-vault/src/zfs.rs:275 |

#### Summary

Installed model package bytes reaches tokio::fs::write weights.bin at line 293 because PQC engine exists but store/load paths do not encrypt. The result is Raw storage disclosure and tampering of model weights.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, PQC engine exists but store/load paths do not encrypt, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `mai-vault/src/zfs.rs:273-277`

This source implements the control point where PQC engine exists but store/load paths do not encrypt.

```rust
        }

        let model_dir = self.config.storage.mount_point.join(model_id);
        let weights_path = model_dir.join("weights.bin");
        let manifest_path = model_dir.join("manifest.json");
```

#### Validation

The current snapshot confirms Installed model package bytes -\> PQC engine exists but store/load paths do not encrypt -\> tokio::fs::write weights.bin at line 293 -\> Raw storage disclosure and tampering of model weights.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `mai-vault/src/zfs.rs:273-277`

This source implements the control point where PQC engine exists but store/load paths do not encrypt.

```rust
        }

        let model_dir = self.config.storage.mount_point.join(model_id);
        let weights_path = model_dir.join("weights.bin");
        let manifest_path = model_dir.join("manifest.json");
```

#### Dataflow

Installed model package bytes -\> PQC engine exists but store/load paths do not encrypt -\> tokio::fs::write weights.bin at line 293 -\> Raw storage disclosure and tampering of model weights

- **Source:** Installed model package bytes

- **Sink:** tokio::fs::write weights.bin at line 293

- **Outcome:** Raw storage disclosure and tampering of model weights

**Broken or missing security control** — `mai-vault/src/zfs.rs:273-277`

This source implements the control point where PQC engine exists but store/load paths do not encrypt.

```rust
        }

        let model_dir = self.config.storage.mount_point.join(model_id);
        let weights_path = model_dir.join("weights.bin");
        let manifest_path = model_dir.join("manifest.json");
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Installed model package bytes

- **Entry point:** mai-vault/src/zfs.rs

- **Outcome:** Raw storage disclosure and tampering of model weights

#### Severity

**Medium** — The path is in scope and crosses a protected product boundary. Raw storage disclosure and tampering of model weights. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: PQC engine exists but store/load paths do not encrypt. Add a negative regression test that drives Installed model package bytes and proves the operation cannot reach tokio::fs::write weights.bin at line 293.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-23"></a>

### [23] Production-like deployment images use mutable tags

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | medium |
| Confidence rationale | Current source establishes the flaw, but deployment, provider, or operator-mediated preconditions limit exploitability confidence. |
| Category | Untrusted mutable deployment artifact |
| CWE | CWE-494 |
| Affected lines | deployment/wsf-ha/docker-compose.yml:57 |

#### Summary

Registry tag resolution reaches wsf-api latest image is pulled at deploy time because No immutable digest or provenance binding. The result is Supply-chain substitution of trust-plane runtime.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, No immutable digest or provenance binding, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `deployment/wsf-ha/docker-compose.yml:55-59`

This source implements the control point where No immutable digest or provenance binding.

```yaml
  # Stateless — scale with `deploy.replicas` / `docker compose up --scale`.
  wsf-api:
    image: islandmountain/wsf-api:latest
    environment:
      WSF_OPENBAO_ADDR: https://openbao:8200
```

#### Validation

The current snapshot confirms Registry tag resolution -\> No immutable digest or provenance binding -\> wsf-api latest image is pulled at deploy time -\> Supply-chain substitution of trust-plane runtime.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `deployment/wsf-ha/docker-compose.yml:55-59`

This source implements the control point where No immutable digest or provenance binding.

```yaml
  # Stateless — scale with `deploy.replicas` / `docker compose up --scale`.
  wsf-api:
    image: islandmountain/wsf-api:latest
    environment:
      WSF_OPENBAO_ADDR: https://openbao:8200
```

#### Dataflow

Registry tag resolution -\> No immutable digest or provenance binding -\> wsf-api latest image is pulled at deploy time -\> Supply-chain substitution of trust-plane runtime

- **Source:** Registry tag resolution

- **Sink:** wsf-api latest image is pulled at deploy time

- **Outcome:** Supply-chain substitution of trust-plane runtime

**Broken or missing security control** — `deployment/wsf-ha/docker-compose.yml:55-59`

This source implements the control point where No immutable digest or provenance binding.

```yaml
  # Stateless — scale with `deploy.replicas` / `docker compose up --scale`.
  wsf-api:
    image: islandmountain/wsf-api:latest
    environment:
      WSF_OPENBAO_ADDR: https://openbao:8200
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Registry tag resolution

- **Entry point:** deployment/wsf-ha/docker-compose.yml

- **Outcome:** Supply-chain substitution of trust-plane runtime

#### Severity

**Medium** — The path is in scope and crosses a protected product boundary. Supply-chain substitution of trust-plane runtime. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: No immutable digest or provenance binding. Add a negative regression test that drives Registry tag resolution and proves the operation cannot reach wsf-api latest image is pulled at deploy time.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

<a id="finding-24"></a>

### [24] Revocation snapshots lack freshness scope and anti-rollback enforcement

| Field | Value |
| --- | --- |
| Severity | medium |
| Confidence | high |
| Confidence rationale | Direct current-snapshot source and deployment evidence establish the source, missing control, and sink; live infrastructure reproduction was not necessary for reportability. |
| Category | Revocation replay |
| CWE | CWE-294 |
| Affected lines | crates/fabric-revocation/src/lib.rs:151 |

#### Summary

Signed stale snapshot reaches Consumers accept replayed older valid snapshots because verify checks signature only and schema has no epoch issuer tenant or scope. The result is Revoked authority can remain usable.

#### Root Cause

The violated invariant is that privilege, tenant, regulated-data, filesystem, credential, or production authority must be derived and bounded by trusted server-side state before the operation. Here, verify checks signature only and schema has no epoch issuer tenant or scope, allowing the lower-trust source to reach the sink.

**Broken or missing security control** — `crates/fabric-revocation/src/lib.rs:149-153`

This source implements the control point where verify checks signature only and schema has no epoch issuer tenant or scope.

```rust
/// # Errors
/// Returns [`RevocationError::MalformedSignature`] or [`RevocationError::InvalidSignature`].
pub fn verify(
    snapshot: &RevocationSnapshot,
    verifier: &dyn Verifier,
```

#### Validation

The current snapshot confirms Signed stale snapshot -\> verify checks signature only and schema has no epoch issuer tenant or scope -\> Consumers accept replayed older valid snapshots -\> Revoked authority can remain usable.

Validation method: Static source/control/sink trace with adjacent guard, sibling, test, and deployment review

**Broken or missing security control** — `crates/fabric-revocation/src/lib.rs:149-153`

This source implements the control point where verify checks signature only and schema has no epoch issuer tenant or scope.

```rust
/// # Errors
/// Returns [`RevocationError::MalformedSignature`] or [`RevocationError::InvalidSignature`].
pub fn verify(
    snapshot: &RevocationSnapshot,
    verifier: &dyn Verifier,
```

#### Dataflow

Signed stale snapshot -\> verify checks signature only and schema has no epoch issuer tenant or scope -\> Consumers accept replayed older valid snapshots -\> Revoked authority can remain usable

- **Source:** Signed stale snapshot

- **Sink:** Consumers accept replayed older valid snapshots

- **Outcome:** Revoked authority can remain usable

**Broken or missing security control** — `crates/fabric-revocation/src/lib.rs:149-153`

This source implements the control point where verify checks signature only and schema has no epoch issuer tenant or scope.

```rust
/// # Errors
/// Returns [`RevocationError::MalformedSignature`] or [`RevocationError::InvalidSignature`].
pub fn verify(
    snapshot: &RevocationSnapshot,
    verifier: &dyn Verifier,
```

#### Reachability

Reachability is established by the saved route, RPC, CLI/package, or deployment evidence and calibrated for public, authenticated, operator-mediated, or internal scope.

- **Attacker:** Signed stale snapshot

- **Entry point:** crates/fabric-revocation/src/lib.rs

- **Outcome:** Revoked authority can remain usable

#### Severity

**Medium** — The path is in scope and crosses a protected product boundary. Revoked authority can remain usable. Exposure and preconditions were calibrated in the saved attack-path report.

Severity would decrease with a dominating fail-closed control or proof the path is unreachable in supported deployments; live end-to-end exploitation could increase confidence.

#### Remediation

Enforce a fail-closed server-side control before the affected operation: verify checks signature only and schema has no epoch issuer tenant or scope. Add a negative regression test that drives Signed stale snapshot and proves the operation cannot reach Consumers accept replayed older valid snapshots.

Tests:
- Add a focused negative test for the exact source/control/sink tuple.
- Add a production-profile integration test proving fail-closed behavior.

Preventive controls:
- Centralize the security control in a non-bypassable shared boundary.
- Maintain route/operation inventory tests and negative deployment-policy checks.

## Reviewed Surfaces

| Surface | Risk Area | Outcome | Notes |
| --- | --- | --- | --- |
| WSF authority and identity plane | Authentication, attenuation, tenant binding, cloud credentials, receipts, revocation | Reported | AF-01, AF-02, AF-04, AF-05, AF-14, AF-15, AF-15B, and AF-16 reported. Evidence: artifacts/03_coverage/repository_coverage_ledger.md |
| MAI gRPC services | Authentication and privileged RPC authorization | Reported | AF-03 reported. Evidence: artifacts/03_coverage/repository_coverage_ledger.md |
| AOG provider and analytics surfaces | Policy enforcement, egress tokenization, metering, tenant isolation | Reported | AF-08, AF-09, AF-10, AF-13, AF-17A, and AF-17B reported. Evidence: artifacts/03_coverage/repository_coverage_ledger.md |
| Vault, ZFS, and readiness | Encryption at rest, readiness truth, snapshot recovery | Reported | AF-06, AF-07A, and AF-07B reported. Evidence: artifacts/03_coverage/repository_coverage_ledger.md |
| Restore and model-package workflows | Path containment and artifact authenticity | Reported | AF-11, AF-19, DF-01A, and DF-01B reported. Evidence: artifacts/03_coverage/repository_coverage_ledger.md |
| Deployment and supply chain | Default credentials, public binds, mutable images | Reported | AF-12 and AF-20 reported. Evidence: artifacts/03_coverage/repository_coverage_ledger.md |
| MAI HTTP, compliance, and audit | HTTP auth, deny-wins policy, WAL integrity | No issue found | No additional independent high-impact finding survived beyond readiness and AOG-mode findings. Evidence: artifacts/02_discovery/work_ledger.jsonl |
| Adapter subprocess framework | Process isolation and resource limits | Needs follow-up | Direct argv and env clearing suppress command injection; default/no-Windows resource isolation needs runtime follow-up. Evidence: artifacts/02_discovery/work_ledger.jsonl |
| Update subsystem | SSRF and signed update transport | Needs follow-up | API download is currently a progress stub; production transport and signed manifest path are not wired. Evidence: artifacts/02_discovery/work_ledger.jsonl |
| Query, archive, XML, and unsafe-deserialization families | SQL/NoSQL/LDAP/XPath, archive extraction, XXE, unsafe object construction | Not applicable | No applicable production sink found in the 615-file inventory. Evidence: artifacts/03_coverage/repository_coverage_ledger.md |
| Agent tools, console, and SDKs | Tool authority and client trust | No issue found | Tool proxy guardrails and server-authority separation provided exact counterevidence for additional candidates. Evidence: artifacts/02_discovery/work_ledger.jsonl |
| Workspace build, lint, format, and tests | Release quality gates | Needs follow-up | fmt, check, and 1831 tests passed; clippy failed on doc_lazy_continuation at mai-core/src/cache.rs:109. Evidence: artifacts/02_discovery/work_ledger.jsonl |

## Open Questions And Follow Up

- Does the production adapter deployment enforce OS-level memory CPU filesystem and network isolation on every supported platform?
- What exact signed update transport will replace the current progress stub?
- Live production proof of Linux cgroup enforcement and Windows isolation is not available.
  - Follow-up prompt: Review deferred unit adapter-resource-isolation-runtime and close its stated proof gap. Paths: mai-adapters/src/process.rs.
- Production HTTPS transport and signed update manifest path are not wired, so end-to-end SSRF/integrity validation is deferred.
  - Follow-up prompt: Review deferred unit update-transport-runtime and close its stated proof gap. Paths: mai-core/src/models/update.rs, mai-api/src/handlers/updates.rs.
- Clippy fails with -D warnings on a documentation list indentation error.
  - Follow-up prompt: Review deferred unit clippy-quality-gate and close its stated proof gap. Paths: mai-core/src/cache.rs.
