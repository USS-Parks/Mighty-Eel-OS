<!--
SCAN-1 Review Integrity: every PR fills this template. Reviewers
must check every box before approving. Anything left unchecked is
a blocker unless explicitly waived in a review comment.
-->

## Summary

<!-- One short paragraph: what changed and why. Link the session ticket / lane row if applicable (e.g. J-12b, GD75-08, SCAN-1). -->

## Lane / session id

<!-- e.g. session/J-23, session/SCAN-1, or N/A for off-lane fix -->

## Scope

- [ ] Touches the **API surface** (`mai-api/`, `proto/`)
- [ ] Touches the **adapter capsules** (`adapters/`, `mai-adapters/`)
- [ ] Touches the **compliance / governance** surface (`mai-compliance/`, `mai-vault/`, `mai-hil/`)
- [ ] Touches **build / packaging / CI** (`Dockerfile`, `Cargo.toml`, `requirements*.txt`, `.github/`)
- [ ] Touches **integrity infrastructure** (`.integrity/`, `.githooks/`, `tools/`)
- [ ] Documentation only

## Quality gates (must all be green before merge)

- [ ] `cargo fmt --check` clean
- [ ] `cargo clippy --workspace -- -D warnings -A clippy::pedantic` clean
- [ ] `cargo test --workspace` passes (or scope-narrowed equivalent named below)
- [ ] `ruff check adapters/ mai-sdk-python/` clean
- [ ] `mypy --strict mai-sdk-python/src/` and `mypy adapters/` clean
- [ ] `pytest` passes for adapters touched
- [ ] No new `todo!()` / `unimplemented!()` / `unreachable!()` in production paths
- [ ] No new `println!` / `eprintln!` outside CLI-output modules

## Evidence

<!--
For any non-trivial change, link to evidence:
  - test output, perf measurement, screenshot, or evidence doc under mai/docs/
  - if the change closes a row in a remediation lane (DOUGHERTY, GD75, SCAN-1, etc.),
    link the row and update its verdict TRUE/FALSE/MIXED.
-->

## Security / compliance impact

- [ ] No new dependencies, OR new deps are pinned + hashed and pass `cargo deny check`
- [ ] No new network egress points
- [ ] No new auth-exempt routes
- [ ] No new `unsafe` Rust blocks
- [ ] No new secrets / credentials / private keys committed
- [ ] If touches PHI / ITAR / OCAP-tagged code: relevant policy doc reviewed

## Rollback

<!-- One sentence: how do we revert this safely if it breaks in prod? Hint: most commits should be a clean `git revert <sha>` away. -->

## Footer

<!--
CANON: every commit (and the PR's last commit) ends with the canonical footer
and never credits an AI co-author. `.githooks/commit-msg` stamps it
automatically. Verify with `git log -1 --format=%B`.
-->

- [ ] Last commit ends with the exact line:
      `Authored and reviewed by Basho Parks, copyright 2026`
