# Agentic Security Map → Sovereignty Stack (design contract)

**Canonical spec.** "The Agentic Orchestration & Security Map" (Basho Parks, July 2026 — also an
islandmountain.io blog post + the "8 ways agents get hijacked" infographic) is the **threat model
and control-plane doctrine** the AOG/WSF stack implements. The marketing map is the engineering
spec. Every AOG/WSF feature must trace to a threat on this map, and every "hop" it governs is an
enforcement point.

**Thesis (verbatim):** *"EVERY HOP CROSSES THE CONTROL PLANE — identity → policy → sandbox →
guardrails → audit, enforced outside the model."* Corollary: *"A system prompt is a request; a
sandbox and egress allowlist are enforcement."* This is also the anti-lock-in argument — the value
is in the deterministic control layer, not the model.

---

## Threat → primary control → where it lives → status

| # | Threat (OWASP-LLM-aligned) | Primary control (map) | Where in the Sovereignty Stack | Status |
|---|---|---|---|---|
| 1 | Prompt injection (direct + indirect) | Guardrails + HITL | `aog-gateway` injection classifiers (mai-compliance) + `aog-approvals` (T3/T4); untrusted-content rule | planned |
| 2 | Agentjacking (session takeover) | Session integrity | **NEW: `wsf` signed checkpoints + re-auth on resume/handoff + anomaly kill-switch**; `aog` session record/replay (T7) | **gap → add** |
| 3 | Tool poisoning (malicious/rug-pull MCP) | Supply-chain security | **NEW: `aog-toolproxy` signed+pinned tool manifests, MCP vetting, rug-pull/description-drift detection, SBOM, allow/deny/ask engine** | **gap → add (T1 expand)** |
| 4 | Data exfiltration (lethal trifecta) | Egress allowlists | `aog-gateway` egress scan + tokenization (G8); `fabric-envelope`; **NEW: outbound domain allowlists per token** | partial → sharpen |
| 5 | Excessive agency (confused deputy) | Least privilege | `fabric-token` scoped + attenuable, per-call creds via `wsf-broker` (T2), deny-by-default, blast-radius caps (T8) | planned ✓ |
| 6 | Memory / RAG poisoning | Provenance tagging | **NEW: provenance tags on memory/RAG writes + quarantine of unverified writes** (`fabric-envelope` label applied to memory) | **gap → add** |
| 7 | Identity spoofing (agent-to-agent) | AuthN on handoffs | `fabric-identity` (SPIFFE + PKI, mutual auth); **NEW: typed authenticated agent-hop payloads** | partial → sharpen |
| 8 | Supply chain (deps/models/registries) | SBOM + scanning | image SBOMs (D3); **model-weights digest in receipts** (provable model identity); tool-manifest signing (see #3) | partial → sharpen |
| 9 | Resource exhaustion (runaway/fork-bomb) | Budgets & ceilings | `fabric-token` budget strand (token/cost/tool-call caps); **NEW: depth + iteration caps + wall-clock ceilings**; kill-switch (G9) | partial → sharpen |

*"No single control is sufficient — every threat is also slowed by sandboxing, audit, and policy
(defense in depth)."* Each row's primary control is necessary, not sufficient.

---

## Enrichments the map adds to the plan (net-new features)

These were under-specified in the original PSPR. The map makes them first-class:

- **E-A Orchestration-pattern governance** — AOG governs not just single-agent tool calls but the
  orchestration patterns themselves (orchestrator-workers, sequential pipeline, parallel fan-out,
  router/handoff, evaluator-optimizer, hierarchical teams): **typed + authenticated handoff
  payloads, no implicit shared state, depth caps against runaway trees, provenance roll-up, trust
  tiers per action class, plan-approval + action-approval HITL.** Elevates AOG from
  "gateway + tool proxy + meter" to the full **control plane over agentic orchestration**.
- **E-B Memory/RAG integrity** — provenance tags on every memory/RAG write (source, agent, verified?,
  trust); **quarantine unverified writes** so poisoned content can't become persistent instruction
  injection. Reuses the `fabric-envelope` label concept for memory.
- **E-C Session integrity** — signed checkpoints + re-auth on resume/handoff + anomaly kill-switch,
  so a hijacked/stale session can't continue with inherited authority. A WSF capability (signed
  state) tied to durable execution.
- **E-D Tool supply-chain** — treat MCP servers/tools like dependencies: signed + pinned manifests,
  vetting, rug-pull / description-drift detection, SBOM, and a central **allow / deny / ask** engine.
- **E-E Sandboxed execution** — for code-interpreter / shell / browser tools: ephemeral
  containers / microVMs, workspace jail + read-only mounts, egress blocked by default. Governs how a
  tool *runs*, not just whether it's *authorized*.
- **E-F OWASP LLM Top 10 alignment** — add as an evidence-pack framework alongside HIPAA / ITAR /
  OCAP / SOC 2 / NIST AI RMF / EU AI Act. The threat→control table becomes an audit artifact.

## The lethal trifecta (design principle for egress)
Exfiltration needs all three: **private data + untrusted content + outbound channel**. The stack is
designed to **break at least one leg on every path** — restrict private data (envelopes + tokenized
egress), quarantine untrusted content (provenance + injection classifiers), or block outbound
(egress allowlists). No path is allowed to hold all three legs unbroken.

## Staging (keeps M1–M3 shippable)
- **M1 (sovereign shadow):** #4/#5/#9 controls + audit (the gateway + tokens already carry these).
- **M2 (enforce + agents):** #1/#3/#6/#7 + E-A/E-B/E-D + session-integrity start (E-C).
- **M3 (estate):** E-E sandboxed execution, full E-C, OWASP evidence pack (E-F), supply-chain (#8).
