// TypeScript mirrors of the WSF + AOG wire contracts. Field names and enum
// spellings are exact to the Rust serde output (`fabric-contracts`, `wsf-api`,
// `wsf-ledger`, `aog-gateway::meter`) so the console deserializes real
// responses without adapters. Keep these in lockstep with the crates.

// ── fabric-contracts::common ───────────────────────────────────────────────

/** Routing ceiling / permitted destination (serde `snake_case`). */
export type Route = 'local_only' | 'local_preferred' | 'cloud_allowed';

/** Data-classification ladder, least → most sensitive (serde `lowercase`). */
export type Classification =
  | 'public'
  | 'internal'
  | 'restricted'
  | 'controlled'
  | 'secret';

/** Compliance regimes a tenant may license (serde `snake_case`). */
export type ComplianceScope = 'hipaa' | 'itar_ear' | 'ocap';

/** Revocation state at evaluation time (serde `lowercase`). */
export type RevocationStatus = 'valid' | 'revoked' | 'stale' | 'unknown';

/** Receipt routing outcome (wire form is PascalCase, mirrors MAI `AuditEntry`). */
export type RoutingDecision = 'Allow' | 'LocalOnly' | 'Quarantine' | 'Deny';

/** A detached signature over a canonical payload. */
export interface Signature {
  alg: string;
  key_id: string;
  value: string;
}

// ── fabric-contracts::token ────────────────────────────────────────────────

/** Spend ceilings carried in the token (u64 caps, u32 tool caps). */
export interface Budget {
  token_cap: number;
  tokens_spent: number;
  usd_cap_cents: number;
  usd_spent_cents: number;
  tool_call_cap: number;
  tool_calls_spent: number;
}

export type CaveatType =
  | 'route_ceiling'
  | 'model_allowlist'
  | 'resource_prefix'
  | 'tool_allowlist'
  | 'expiry_before'
  | 'classification_ceiling';

export interface Caveat {
  type: CaveatType;
  value: string;
}

export interface Attenuation {
  parent_id?: string | null;
  caveats: Caveat[];
}

/** The trust token — the WSF primitive. A superset of the MAI `SignedClaim`. */
export interface TrustToken {
  token_id: string;
  issued_at: string;
  expires_at: string;
  issuer: string;
  trust_bundle_version: string;
  tenant_id: string;
  subject_id?: string | null;
  subject_hash: string;
  service_identity?: string | null;
  identity_id?: string | null;
  roles: string[];
  compliance_scopes: ComplianceScope[];
  allowed_routes: Route[];
  allowed_models: string[];
  max_data_classification: Classification;
  country?: string | null;
  person_type?: string | null;
  offline_mode: boolean;
  revocation_status: RevocationStatus;
  budget?: Budget | null;
  attenuation: Attenuation;
  signature: Signature;
}

// ── wsf-api DTOs ───────────────────────────────────────────────────────────

export interface IssueReq {
  tenant_id: string;
  subject_id: string;
  roles?: string[];
  budget?: Budget | null;
  allowed_models?: string[];
}

export interface TokenResp {
  token: TrustToken;
}

export interface VerifyResp {
  valid: boolean;
  reason: string;
}

export interface AttenuateReq {
  parent: TrustToken;
  child: TrustToken;
}

export interface ExchangeReq {
  token: TrustToken;
  role_arn: string;
}

export interface ExchangeResp {
  access_key_id: string;
  secret_access_key: string;
  session_token: string;
  expiration: string;
}

// ── wsf-ledger ─────────────────────────────────────────────────────────────

/** One ingested receipt with its position in the BLAKE3 chain. The `receipt`
 *  body is service-shaped JSON (a seal receipt, a bridge correlation, …). */
export interface LedgerEntry {
  seq: number;
  source: string;
  receipt: Record<string, unknown>;
  previous_hash: string;
  entry_hash: string;
}

export interface ReceiptsResp {
  entries: LedgerEntry[];
}

// ── aog-gateway::meter ─────────────────────────────────────────────────────

/** Aggregated spend for one (tenant, provider, model, task) group. */
export interface TaskUsage {
  tenant_id: string;
  provider: string;
  model: string;
  workflow_id?: string | null;
  calls: number;
  input_tokens: number;
  output_tokens: number;
  spend_cents: number;
}

/** GET /v1/usage — the meter aggregates + live receipt-chain integrity. */
export interface UsageResp {
  aggregates: TaskUsage[];
  chain_head: string;
  chain_verified: boolean;
}

/** GET /v1/status — the gateway's live posture (open; no virtual key needed). */
export interface StatusResp {
  mode: 'shadow' | 'report_only' | 'enforce';
  providers: string[];
  models: string[];
  receipts: number;
  chain_head: string;
  chain_verified: boolean;
}

// ── aog-approvals (T3) ─────────────────────────────────────────────────────

/** A pending human-approval request for a side-effecting tool call (T3). Carries
 *  a diff preview of exactly what the call will do. */
export interface ApprovalTicket {
  id: string;
  tool_id: string;
  session_id: string;
  profile_id: string;
  summary: string;
  diff_preview: string;
  requested_at: string;
  status: 'pending' | 'approved' | 'denied';
}

/** GET /v1/approvals — the pending inbox. */
export interface ApprovalsResp {
  pending: ApprovalTicket[];
}

/** The result of an approve/deny action — the recorded decision + actor. */
export interface ApprovalActionResp {
  id: string;
  status: 'approved' | 'denied';
  actor: string;
}

// ── aog policy studio (G6) ─────────────────────────────────────────────────

/** A rule's enforcement mode (serde `snake_case`). */
export type PolicyMode = 'shadow' | 'report_only' | 'enforce';

/** One policy rule, rendered in plain language alongside its code (C6). */
export interface PolicyRule {
  id: string;
  title: string;
  regime: ComplianceScope;
  plain_language: string;
  code: string;
  mode: PolicyMode;
}

/** GET /v1/policy — the rule set + the global default enforcement mode. */
export interface PolicyResp {
  rules: PolicyRule[];
  default_mode: PolicyMode;
}

/** A human-readable, rule-cited explanation of a specific denial — the killer demo:
 *  a local model explains *why* a request was denied, citing the exact policy line. */
export interface DenialExplanation {
  decision: string;
  rule_id: string;
  rule_title: string;
  cited_line: string;
  plain_language: string;
}
