# Lamprey Saddle hardening evidence

This directory contains reproducible evidence for the canonical WSF/AOG hardening lane. Evidence is grouped by milestone (`M0` through `M5`) and prompt. Raw scan evidence is preserved without altering its candidate content.

Rules:

- record exact command, exit code, timestamp when relevant, and environment prerequisites;
- never store live credentials, private keys, bearer tokens, plaintext regulated payloads, or cloud credentials;
- distinguish focused, crate, workspace, live-service, and independent-scan gates;
- preserve failed output when it identifies a product or environment blocker; and
- do not overwrite historical evidence when a prompt is rerun—add a dated rerun artifact.

M0 source snapshot: `M0/source-scan/`.

M0 deterministic regression and reachability manifest: `M0/regression-plan.json`.
Validate it with:

`python .integrity/scripts/lamprey-regression-plan-check.py --run-reachability`

The manifest maps all 29 confirmed families to a red/green boundary and keeps
destructive state-mutation reproductions as inert request fixtures until their
owning prompt provisions disposable isolated state. It also provides one
read-only executable reachability question for every deferred family.
