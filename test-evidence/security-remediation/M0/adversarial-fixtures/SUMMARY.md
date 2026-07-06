# 0.4 — Adversarial regression fixtures

Quarantined harness: `crates/fabric-token/tests/security_regression.rs`, gated by the
`security-regression` cargo feature (no `#[ignore]`, per §0.5). Deterministic-id
registry: `docs/scans/SECURITY-REGRESSION-REGISTRY.md`.

Verify:
- cargo fmt --check ............................................ PASS
- cargo test -p fabric-token --features security-regression .... PASS (5 fixtures)
- cargo test -p fabric-token (default) ........................ PASS, 0 reg_af run
  (quarantined out of the product suite)
- cargo clippy -p fabric-token --features security-regression .. PASS

Fixtures proven against the current code (they assert the vulnerability, so a fix
will flip them):
- REG-AF-001-unsigned-parent: attenuate signs a child of an unsigned parent.
- REG-AF-001-wrong-key-parent: attenuate accepts an attacker-key parent.
- REG-AF-001-role-widening: child gains a role the parent never held.
- REG-AF-001-tenant-swap: child claims a different tenant.
- REG-AF-006-revoked-parent: a revoked parent still mints fresh children.

AF-002/003/004/007 identifiers are reserved in the registry; their fixtures land
with their phase (A/E/B/L), where each flips to assert the repaired behavior.
