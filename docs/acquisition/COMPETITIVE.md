# Competitive Analysis

**Project:** Island Mountain Model Abstraction Interface (MAI) +
Lamprey
**Audience:** Acquirer corporate-development teams, product strategy
leads, M&A analysts
**Status:** Session 45 acquisition documentation
**Last Updated:** 2026-05-23

This document compares Lamprey + MAI to the AI compliance /
guardrails / governance products currently in the market. Every
differentiation claim cites a specific module or test in this
repository — the point of this analysis is to be defensible under
diligence, not to win a marketing argument.

For positioning, see [`../ACQUISITION-PACKAGE.md`](../product/ACQUISITION-PACKAGE.md).
For IP defensibility, see [`IP.md`](IP.md).

---

## The category boundaries

The "AI governance" space in 2026 covers four overlapping product
shapes:

1. **Guardrails / output filtering.** Per-response checks
   (toxicity, PII, jailbreak resistance). Examples: Guardrails AI,
   NeMo Guardrails (NVIDIA).
2. **Policy enforcement / authorisation.** Identity-and-policy
   wrappers, typically for tool calls and resource access. Examples:
   Minder (CNCF, now Datadog), some Okta extensions.
3. **AI gateways / routers.** Multi-model routing with rate-limit /
   cache / observability. Examples: Cloudflare AI Gateway, Kong AI
   Gateway, Helicone, LiteLLM Proxy.
4. **Cloud-provider built-ins.** Provider-specific compliance
   controls. Examples: AWS Bedrock Guardrails, Azure AI Content
   Safety, Vertex AI safety filters.

Lamprey overlaps with all four but sits in a different position:
**it is the inference-time placement contract, not an output
filter or a routing wrapper.** It decides *whether* a request runs,
*where* it runs, and *what audit shape* it produces — and that
decision is the basis for the scheduler's placement, not a post-hoc
sanity check on the response.

---

## Direct competitors (named)

### Guardrails AI (`guardrailsai.com`)

**Their product:** Pydantic-backed validators that run before/after
model calls. Wide library of "rails" (PII, profanity, JSON shape,
factual consistency).

**Where they overlap with us:** PII detection. Limited overlap with
the HIPAA PHI surface in `mai-compliance/src/phi.rs`.

**Where they do not overlap:**
- No multi-domain composer. A Guardrails pipeline runs each rail in
  sequence; conflicts are handled by ordering, not by a
  rule-precedence layer.
- No tribal data sovereignty / OCAP. Not a category they address.
- No hardware-aware placement integration. Rails fire after a model
  is chosen, not before placement.
- No tamper-evident audit chain. The audit they ship is application
  logs.
- Cloud-first deployment posture; no air-gap-first stance.

**Why an acquirer would want Lamprey over Guardrails:** Guardrails is
a developer toolkit for application-side hardening. Lamprey is an
infrastructure-side governance layer. Different category.

### NeMo Guardrails (NVIDIA)

**Their product:** Colang-DSL-driven rails enforced inside the NeMo
inference path. Strong for jailbreak resistance and conversational
safety on NVIDIA-hosted models.

**Where they overlap with us:** Per-request safety enforcement,
audit hooks.

**Where they do not overlap:**
- Bound to NeMo / NVIDIA inference. Not portable to other adapters.
- No HIPAA-specific module (general PII recognition only).
- No ITAR/EAR jurisdiction logic.
- No OCAP / tribal data sovereignty.
- No hardware air-gap integration.
- The audit log is event logging, not hash-chained tamper-evident
  with PQC signatures.

**Why an acquirer would want Lamprey over NeMo Guardrails:** NeMo
Guardrails is an NVIDIA-stack moat — it ties safety to NVIDIA's
inference runtime. Lamprey is adapter-agnostic and ships HIPAA / ITAR
/ OCAP modules out of the box.

### Minder (CNCF, Datadog acquisition)

**Their product:** Policy-as-code for software supply chains and
generic resource policies; OPA-style policy evaluation.

**Where they overlap with us:** Policy evaluation engine, rule
composition.

**Where they do not overlap:**
- Not AI-specific. No PHI / ITAR / OCAP modules; you'd write all of
  that yourself in Rego.
- No inference-time placement integration.
- No trust manifold; no offline trust bundles.
- No air-gap integration.
- No compliance reporting (HIPAA / ITAR / OCAP audit reports).

**Why an acquirer would want Lamprey over Minder:** Minder is a
generic policy engine. Lamprey is a finished compliance product for
regulated AI. A buyer who wants HIPAA + ITAR + OCAP today would have
to build all three in Minder; Lamprey ships them.

### Cloudflare AI Gateway

**Their product:** Multi-model proxy with caching, rate-limiting,
analytics, and basic safety filtering. Strong on observability.

**Where they overlap with us:** Routing across models, observability
hooks.

**Where they do not overlap:**
- No HIPAA / ITAR / OCAP module. Safety is generic content filters.
- Cloud-only. No air-gap deployment.
- No trust manifold. Auth is API-key only.
- No tamper-evident audit chain.
- Routing decisions are configurable but not policy-driven from a
  regulatory perspective.

**Why an acquirer would want Lamprey over Cloudflare AI Gateway:**
Cloudflare AI Gateway is a routing layer for cloud AI. Lamprey is a
compliance layer for regulated AI that has to run on hardware the
customer possesses. Different deployment posture, different
regulatory surface.

### AWS Bedrock Guardrails / Azure AI Content Safety

**Their product:** Cloud-native content moderation tied to the
respective inference platform.

**Where they overlap with us:** Per-request content checks, audit
logging.

**Where they do not overlap:**
- Locked to the cloud provider; no air-gap deployment.
- HIPAA available via BAA, but the data still flows to the cloud
  provider. ITAR is awkward. OCAP is absent.
- No tribal data sovereignty.
- Cross-provider portability is not a feature.

**Why an acquirer would want Lamprey over a cloud built-in:** Cloud
built-ins lock the customer to the cloud. A regulated customer who
needs HIPAA + air-gap + tribal sovereignty cannot use a cloud
built-in for the bulk of their workload — that customer needs
Lamprey + MAI.

### Helicone, LiteLLM Proxy, Kong AI Gateway

**Their products:** Multi-provider proxies with analytics, caching,
or auth. Developer-tool category.

**Where they overlap with us:** Routing across model providers.

**Where they do not overlap:** All compliance dimensions noted above.

**Why an acquirer would want Lamprey over an AI proxy:** AI proxies
solve a "many cloud providers, one API" problem. Lamprey solves a
"regulated industry, prove the audit trail" problem.

---

## Differentiation summary

The table below uses Y / N / partial. Every Y for Lamprey cites a
specific landed module.

| Capability | Guardrails AI | NeMo Guardrails | Minder | Cloudflare AI Gateway | AWS Bedrock Guardrails | Azure AI Content Safety | **Lamprey** | Citation |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|---|
| PII / PHI detection | Y | Y | N | partial | Y | Y | Y | `mai-compliance/src/phi.rs`, `deid.rs` |
| HIPAA full module + reports | N | N | N | N | partial | partial | Y | `mai-compliance/src/hipaa/`, reports |
| ITAR/EAR module | N | N | N | N | N | N | Y | `mai-compliance/src/{itar,ear,jurisdiction,tech_data}.rs` |
| OCAP / tribal sovereignty | N | N | N | N | N | N | Y | `mai-compliance/src/ocap/` (9-stage pipeline) |
| Multi-domain conflict resolution | N | partial | partial | N | N | N | Y | `mai-compliance/src/policy/composer.rs` |
| Tamper-evident audit (hash chain + PQC sig) | N | N | N | N | N | N | Y | `mai-compliance/src/audit/{chain,store}.rs` |
| Hardware air-gap integration | N | N | N | N | N | N | Y | `mai-core/src/airgap/`, BF-4 cache |
| Offline trust bundles | N | N | N | N | N | N | Y | `mai-compliance/src/trust_cache.rs`, BF-4 |
| Routing decision *before* placement | N | partial | N | partial | N | N | Y | Router → composer → scheduler order |
| Cloud-portable / air-gap-portable | partial | N | Y | N | N | N | Y | deployment profiles |
| OpenAPI / SDK / dashboard / SIEM bridge | partial | partial | partial | Y | Y | Y | Y | mai-api, mai-sdk-python, dashboard, BF-5 |

The four-cell block — OCAP, hardware air-gap, multi-domain composer,
PQC-signed audit chain — is unique to Lamprey today. Any one of
those four is a defensible position for an acquirer; together they
form a near-impossible-to-replicate product because each requires a
different domain of expertise (First Nations data governance, secure
hardware integration, conflict-resolution policy theory,
post-quantum cryptography).

---

## Per-acquirer rationale

### Acquirer profile A — Cloud AI gateway / proxy vendor

Examples: Cloudflare, Datadog, Kong, Snowflake.

**What they get:** A compliance product that extends their gateway
into regulated industries the gateway alone cannot serve. Lamprey
plugs above their existing routing; the dashboard becomes their
"enterprise compliance" UI.

**What they avoid:** Building HIPAA / ITAR / OCAP from scratch (12+
months and a regulated-industry hire).

### Acquirer profile B — Defence / federal integrator

Examples: Palantir, Anduril, CACI, Booz Allen, IBM Federal.

**What they get:** ITAR/EAR jurisdiction module + air-gap-first
deployment + PQC-signed audit + tamper-evident chain. Suitable for
DoD, IC, civilian-agency, and federal-prime work where cloud AI is
disallowed.

**What they avoid:** Building a clean-room compliance stack against
DoD instruction 5200.48 + 12+ commercial controls.

### Acquirer profile C — Tribal / healthcare-tribal services

Examples: tribal health consortia, IHS-partnered integrators, tribal
energy organisations.

**What they get:** The only AI compliance product with native OCAP
tribal data sovereignty + treaty consent + cultural consent. Allows
deployment on tribal land without sovereignty violation.

**What they avoid:** Trying to retrofit a non-tribal product to
OCAP, which is a known failure mode (sovereignty audits routinely
catch this).

### Acquirer profile D — Healthcare cloud / EHR vendor

Examples: Epic, Cerner-Oracle, athenahealth, Athelas.

**What they get:** A HIPAA-native compliance stack with PHI module,
BAA enforcement, healthcare report generator, and a documented
trust-boundary contract that satisfies a HIPAA Security Risk
Assessment.

**What they avoid:** Adding ITAR / OCAP later if they expand into
federal-funded or tribal health markets.

### Acquirer profile E — Cloud provider (AWS / Google / Azure)

Examples: AWS, Google Cloud, Azure, Oracle Cloud.

**What they get:** A compliance product that they can offer for
"customer-managed" regulated workloads where the customer needs the
deployment posture to be air-gap-or-cloud, not cloud-only. Bedrock
Guardrails / Azure AI Content Safety are cloud-only; Lamprey
addresses the air-gap-required half of the market.

**What they avoid:** Having to walk away from healthcare, defence,
and tribal customers who cannot deploy to public cloud.

---

## Honest assessment

Where competitors are strong and Lamprey is *not* trying to compete:

- **General content moderation** (toxicity, jailbreak, prompt injection
  resistance for general SaaS): NeMo Guardrails and the cloud
  built-ins are mature here. Lamprey ships PHI / controlled-tech /
  OCAP but does not aim to replace a general moderation layer.
- **Self-serve developer onboarding for hobby projects:** Guardrails
  AI's pip-install-and-go story is friendlier than Lamprey's
  cargo-test-+-deployment-profile story. Lamprey is an
  infrastructure-grade product, not a Saturday-afternoon library.
- **Pure routing across model providers:** LiteLLM Proxy and Kong AI
  Gateway are purpose-built for this. Lamprey routes for compliance
  reasons; it is not a multi-cloud cost-optimisation gateway.

This honesty is intentional. An acquirer's diligence team will find
these gaps within an hour; better to surface them up front.

---

## Verification

Every Y in the table above maps to a runnable test:

```powershell
cargo test -p mai-compliance --lib                    # 326+ green tests
cargo test -p mai-compliance phi::                    # PHI detection
cargo test -p mai-compliance itar::                   # ITAR module
cargo test -p mai-compliance ear::                    # EAR module
cargo test -p mai-compliance ocap::                   # OCAP pipeline
cargo test -p mai-compliance policy::composer         # composer
cargo test -p mai-compliance audit::                  # audit chain + sigs
pytest apps/openbao-trust-demo/tests/                 # Trust Manifold
pytest apps/tribal-sovereignty/tests/                 # OCAP sovereignty
pytest apps/compliance-routed/tests/                  # composer routing
```

A diligence engineer can sit with this document open and walk every
line of the table to a green test in under two hours.
