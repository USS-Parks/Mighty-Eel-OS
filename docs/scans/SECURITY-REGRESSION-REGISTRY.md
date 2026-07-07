# Security Regression Registry

A deterministic regression identifier per audit finding. Fixtures were frozen in
prompt 0.4 asserting the CURRENT vulnerable behavior in a quarantined harness (no
`#[ignore]`, per plan §0.5). When a fix lands, the matching fixture **flips** to
assert the repaired behavior and joins the default product suite.

Fixture harness: `crates/fabric-token/tests/security_regression.rs` — now runs in
the default suite (`cargo test -p fabric-token`), no feature gate, since AF-001 /
AF-006 are repaired (Phase T).

Status: FROZEN (asserts the vuln, quarantined) → REPAIRED (flipped to assert the
fix, in the product suite) → PROVEN (a live-service gate confirms end-to-end).

| Reg ID | Finding | Fixture now asserts | Status | Location |
|---|---|---|---|---|
| REG-AF-001-unsigned-parent | AF-001 | attenuate refuses a child of an unsigned parent (`ParentUnverified`) | REPAIRED (Phase T) | fabric-token suite |
| REG-AF-001-wrong-key-parent | AF-001 | attenuate refuses an attacker-key parent (`ParentUnverified`) | REPAIRED (Phase T) | fabric-token suite |
| REG-AF-001-role-widening | AF-001 | a child role the parent never held is refused (`AttenuationWidens{roles}`) | REPAIRED (Phase T) | fabric-token suite |
| REG-AF-001-tenant-swap | AF-001 | a cross-tenant child is refused (`AttenuationWidens{tenant_id}`) | REPAIRED (Phase T) | fabric-token suite |
| REG-AF-006-revoked-parent | AF-006 | a revoked parent mints no child (`ParentRevoked`) | REPAIRED (Phase T) | fabric-token suite |
| REG-AF-002-caller-subject | AF-002 | issuance refuses an unauthenticated caller (401) and copies tenant/subject/roles from the verified principal, not the body | REPAIRED (Phase A) | wsf-api auth_gate |
| REG-AF-003-cross-tenant-unseal | AF-003 | cross-tenant / cross-owner / unbound-v1 unseal is refused before Transit; the binding is signed + AAD-bound | REPAIRED (Phase E) | wsf-seal tenant_binding + fabric-envelope |
| REG-AF-004-arbitrary-role | AF-004 | the caller names a tenant-scoped grant, not a role ARN; an unknown or cross-tenant grant is denied before AWS | REPAIRED (Phase B) | wsf-broker |
| REG-AF-007-unfiltered-receipts | AF-007 | unauthenticated receipt read → 401; a tenant sees only its own receipts; no arbitrary field query / existence oracle | REPAIRED (Phase L) | wsf-ledger + wsf-api auth_gate |

Every finding has a deterministic identifier (gate §0.4). AF-001 is REPAIRED with
focused + property proof (Phase T); AF-006's attenuate-path leg is REPAIRED here,
and its full consumer integration (revocation snapshot consumed on every
privileged path) lands in Phase R. AF-002/003/004/007 identifiers are reserved and
flip with their phase. The `caveat widening` and `stale token` cases named in §0.4
are covered by the AF-001 role/tenant-widening and AF-006 revoked-parent fixtures
respectively. Live end-to-end proof (PROVEN) rides the T7/R6 OpenBao gates.
