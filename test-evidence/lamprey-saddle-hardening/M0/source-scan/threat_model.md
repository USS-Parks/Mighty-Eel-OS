# Mighty Eel MAI Repository Threat Model

## Overview

The repository implements the local inference and governance layer for the Island Mountain sovereign-data appliance. Its deployed runtime combines the Rust MAI API and scheduler, Python inference adapters, the Lamprey compliance engine, the Woven Sovereignty Fabric (WSF) trust plane, Agentic Orchestration Governance (AOG), encrypted-vault integrations, package/update tooling, and deployment manifests. The highest-value assets are regulated prompts and completions, model weights, tenant identity and policy state, token-signing and envelope-encryption authority, cloud credential-broker authority, audit evidence, appliance configuration, and host/GPU control.

Primary runtime surfaces are `mai-api/`, `mai-core/`, `mai-scheduler/`, `mai-router/`, `mai-compliance/`, `mai-vault/`, `mai-adapters/`, the WSF and AOG crates under `crates/`, Python adapters under `adapters/`, operator tools under `tools/`, and shipped material under `deployment/`, `packaging/`, `config/`, and `configs/`. Tests, demos, and documentation are supporting evidence; they are security-relevant where they define fixtures, defaults, setup steps, or release claims, but are not assumed to be deployed merely because they exist.

The intended invariant is local-first governance: regulated payloads remain on the appliance unless a policy-authorized route permits otherwise; identity, claims, signatures, and correlation metadata may cross trust boundaries; every privileged decision is authenticated, authorized, tenant-bound, freshness-checked, and auditable; and production must fail closed when required controls are absent.

## Threat Model, Trust Boundaries, and Assumptions

### Assets and privileges

- Regulated content: PHI, export-controlled material, OCAP-governed data, prompts, completions, embeddings, and documents.
- Tenant and subject isolation: tenant IDs, workload identities, roles, policy bundles, classification, allowed models/routes/tools, and budget authority.
- Cryptographic authority: OpenBao tokens, signing keys, ML-DSA trust anchors, TPM-sealed material, ZFS keys, Transit keys, envelope ciphertext/AAD, and revocation state.
- Execution authority: model installation/loading, adapter process execution, tool invocation, cloud-role exchange, package/import operations, power-state control, and administrative APIs.
- Evidence integrity: append-only audit/WAL records, receipt chains, signed compliance reports, readiness results, release manifests, and update/package signatures.
- Availability: inference capacity, GPU/host memory, queues, caches, policy and revocation refresh, audit persistence, and recovery after restart or partition.

### Trust boundaries

1. **Network client to MAI/WSF/AOG APIs.** REST, gRPC, SSE, WebSocket, OpenAI-compatible, Anthropic-compatible, health, metrics, and administrative routes cross from untrusted clients into trusted Rust services. Authentication and authorization must be uniform across protocol variants and long-lived streams.
2. **Tenant/workload identity to trust plane.** Token issuance, attenuation, verification, seal/unseal, credential exchange, approval, and receipt query transform caller intent into authority. Caller-supplied tenant, subject, role, audience, resource, destination, or cloud identity must never become authority without server-side derivation and policy checks.
3. **Trusted Rust core to Python adapters and inference backends.** Adapter configuration, NDJSON/IPC, HTTP endpoints, model identifiers, and backend responses are untrusted. Process isolation, resource limits, framing limits, timeouts, executable resolution, and output validation are required; architectural claims alone are not enforcement.
4. **Appliance to OpenBao, TPM, ZFS, object stores, vector stores, and cloud STS providers.** Remote responses, stored metadata, key identifiers, mount state, and policy state may be stale, malicious, unavailable, or misconfigured. Production readiness must measure live facts and must not certify configured intent.
5. **Package/removable media/update inputs to privileged filesystem and service operations.** Archive names, manifests, model IDs, USB paths, symlinks/reparse points, signatures, and update URLs are attacker-controlled until verification and containment complete.
6. **Agent/model output to tool execution and egress.** Model text and retrieved content are untrusted even when generated locally. They must not self-assert trust, bypass approval, select credentials, or trigger mutation without typed authorization and guardrails.
7. **Operator/developer configuration to production runtime.** Ship profiles, environment variables, Compose/systemd settings, certificate paths, bind addresses, policy bundles, and allowlists are privileged inputs. Production guards must reject demo defaults, missing secrets, development modes, wildcard exposure, and incomplete initialization.
8. **Audit producer to evidence consumer.** Concurrent writers, crash recovery, rotation, signing, tenant filtering, and report export cross an integrity boundary. Hash chaining without authenticated access, durable ordering, or tenant authorization is insufficient.

### Input control

- **Attacker-controlled:** all unauthenticated request fields and headers; bearer tokens from clients; model/tool content; upload/package/archive contents; model IDs and query parameters; adapter/backend responses; public network traffic; tenant identifiers used for enumeration; and data returned by compromised external providers.
- **Authenticated but not automatically trusted:** subject intent, requested roles/scopes/models/routes/tools, parent tokens, approval decisions, envelope metadata, receipt queries, cloud grant requests, and long-lived stream messages.
- **Operator-controlled:** ship profiles, environment variables, key/certificate paths, OpenBao/ZFS/TPM endpoints, deployment manifests, policy bundles, update sources, and recovery commands. Operator control lowers remote exploitability only when the value cannot be influenced by lower-trust actors and misuse cannot silently weaken production guarantees.
- **Developer-controlled:** source, CI workflows, Cargo/Python dependencies, build scripts, test fixtures, Docker bases, and generated artifacts. These are supply-chain inputs and must be pinned, reviewed, and reproducible.

### Required invariants

- No unauthenticated or incorrectly scoped caller can issue, attenuate, verify for authorization, unseal, exchange, approve, invoke, install, administer, or query cross-tenant evidence.
- Token attenuation is restriction-only: authentic parent first, preserve immutable identity/issuer fields, narrow every authority axis, enforce lineage and atomic budget constraints, and apply current revocation state.
- Encryption and sealing bind tenant, owner, audience, policy version, classification, operation, destination, key ID, token lineage, and format version as authenticated context.
- Production startup proves real initialized cryptographic, storage, audit, identity, and policy controls before binding privileged sockets; dev/stub/plaintext controls cannot satisfy readiness.
- All external paths and identifiers are contained under approved roots after canonicalization, including symlink/reparse-point behavior, and before privileged reads/writes or signature trust decisions.
- Protocol variants and streams enforce the same identity lifetime, policy, quotas, cancellation, and audit rules as ordinary REST calls.
- Untrusted adapter, backend, and model output is size/time bounded and cannot acquire host, filesystem, network, credential, or tool authority outside explicit capabilities.
- Audit and receipt evidence is durable, tamper-evident, access-controlled, tenant-filtered, restart-safe, and free of secrets or regulated payloads.
- Failures of OpenBao, revocation refresh, policy loading, audit persistence, key recovery, and isolation controls fail closed on privileged operations.

### Assumptions and exclusions

Physical attackers with sustained possession of an appliance, compromised firmware/kernel/root, malicious maintainers with signing authority, or compromised hardware roots can exceed application-level guarantees; defense against them depends on measured boot, TPM policy, encrypted storage, signed releases, and operational controls. Local-only demo modes can reduce remote severity only if production profiles cannot enable or expose them. Hardware-dependent claims that lack live target evidence remain unproven rather than implicitly trusted.

## Attack Surface, Mitigations, and Attacker Stories

### Network and identity plane

Relevant stories include anonymous issuance of trusted tokens, caller-selected tenant/role escalation, verification that checks only signatures, attenuation of fabricated or expired parents, stale revocation acceptance, cross-tenant receipt enumeration, administrative route exposure, auth discrepancies between REST and gRPC, and identity loss during SSE/WebSocket lifetime. Existing mitigations include Rust API boundaries, API-key middleware in `mai-api/src/auth.rs`, ship-profile guards, policy composition, signed bundles, and typed request models. These controls matter only when every privileged router and protocol path actually applies them.

### WSF cryptographic and broker plane

Relevant stories include signing attacker-constructed child tokens, widening caveats or budgets, unsealing another tenant's envelope, replaying stale snapshots, selecting a privileged AWS/GCP/Azure identity, leaking ephemeral credentials, and querying in-memory receipts without authorization. The primary review roots are `crates/fabric-token/`, `crates/fabric-envelope/`, `crates/fabric-revocation/`, `crates/wsf-api/`, `crates/wsf-bridge/`, `crates/wsf-seal/`, `crates/wsf-broker/`, and `crates/wsf-ledger/`. Cryptographic primitives do not compensate for missing context binding, authentication, monotonicity, or consumer integration.

### AOG/model/tool plane

Relevant stories include prompt injection reaching a privileged tool, caller-controlled trust labels, approval replay or misbinding, credential minters detached from the authorizing identity, permissive shadow/report modes in production, provider redirect or endpoint abuse, streaming responses escaping policy or metering, and unbounded budgets. Existing receipt chains, scanners, guardrails, model maps, approval types, and per-call credential seams reduce risk when production defaults and call-site wiring are complete.

### Adapter, backend, and host plane

Relevant stories include command or module-path injection, malicious backend URLs, oversized or malformed NDJSON, hangs, crash loops, cgroup/systemd setup failure followed by unsandboxed execution, model-path traversal, GPU/host exhaustion, and sensitive backend errors or output entering logs. The workspace forbids unsafe Rust globally, but Python subprocesses and external services remain separate protection domains requiring OS enforcement and strict IPC validation.

### Storage, package, and update plane

Relevant stories include archive traversal, absolute paths, symlink/reparse escape, time-of-check/time-of-use swaps, signature verification after extraction, plaintext model storage, ZFS readiness against an ordinary directory, unsafe snapshot command construction, rollback of policy/evidence, and floating or compromised container images. Signed manifests, canonical paths, direct-argv commands, authenticated encryption, immutable versioning, digest pinning, and negative-control readiness tests are expected mitigations.

### Compliance and audit plane

Relevant stories include classifier evasion, policy conflict resolving to allow, canonicalization ambiguity, concurrent chain forks, unflushed WAL entries, signature/key-rotation gaps, tenant confusion, unauthenticated report download, and sensitive payload leakage into logs or receipts. Deny-wins composition, hash chaining, periodic ML-DSA signatures, WAL replay, metadata-only receipts, and production sealer requirements are meaningful existing mitigations that need end-to-end and restart evidence.

### Supply chain and operational plane

Relevant stories include malicious Cargo/Python dependencies, floating Docker tags, default credentials, secrets in repository/history/evidence, overly privileged containers, writable host mounts, exposed OpenBao development mode, and CI gates that do not reproduce documented checks. Lockfiles, integrity scripts, proprietary release controls, lint/test gates, SBOM/provenance, secret scanning, and signed artifacts reduce this risk when enforced rather than documented only.

## Severity Calibration (Critical, High, Medium, Low)

### Critical

Use Critical for remotely reachable, low-complexity compromise of the system's core authority or regulated-data boundary: unauthenticated minting of broadly trusted tokens; arbitrary signing/encryption-key use; cross-tenant bulk decryption; production remote command execution as a privileged service; or a default deployment that exposes equivalent authority without authentication. Physical root compromise alone is not an application Critical unless the product claims to resist it and code materially defeats a hardware control.

### High

Use High for practical cross-tenant disclosure or privilege escalation requiring some valid access, arbitrary cloud-role exchange, restriction-bypassing attenuation, cross-tenant unseal, package/update traversal to privileged locations, remotely reachable tool execution outside policy, production readiness that certifies plaintext or uninitialized trust/storage controls, or protocol-specific authentication bypass. High also covers durable destruction or broad denial of a production appliance when reachable by an ordinary tenant.

### Medium

Use Medium for tenant-scoped sensitive metadata exposure, revocation gaps with bounded prerequisites, unauthorized receipt access without payload disclosure, audit-chain integrity or crash-consistency failures that undermine evidence but do not directly grant data access, remotely triggerable bounded resource exhaustion, missing rate/size limits, or production hardening weaknesses that require operator error and do not immediately expose core authority.

### Low

Use Low for development-only defaults provably rejected by production guards, limited information disclosure, local denial of service, weak diagnostics, defense-in-depth omissions, or supply-chain reproducibility gaps with substantial additional compromise required. Documentation/claim mismatches and broken quality gates are tracked as readiness or quality issues unless they directly create an exploitable security boundary failure.

Repository: codex-security-target/v1:sha256:43608ec0d10b88e92415417d3e614d0e6470fa3b2c3f520f214ad527afaefbbe
Version: codex-security-snapshot/v1:sha256:2f504c2504ea119582f8981b2fa67c948906810eeab7a61cec73e74596695e80
