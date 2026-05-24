# Local GitDoctor-Style Scan

`tools/local_gitdoctor_scan.py` is the offline audit tool for the
DOUGHERTY lane. It mirrors the check families and check IDs from John's
GitDoctor screenshots so the team can self-run the same class of static
audit after J-13 and before the external J-14 rescan.

## What It Covers

- Security checks `SEC-001` through `SEC-016`
- Performance anti-pattern checks `PERF-001` through `PERF-006`
- Code quality checks `QUA-001` through `QUA-010`
- Configuration and DevOps checks `CFG-001` through `CFG-007`
- Testing checks `TST-001` through `TST-006`
- Review integrity checks `REV-001` through `REV-008`
- Project hygiene checks `PRJ-001` through `PRJ-005`

The scanner is heuristic. A failed check means "inspect this and either
fix it or document why it is intentional." It is not a substitute for
manual review, live backend integration tests, or the external GitDoctor
rescan in J-14.

## Review Integrity Lens

The `REV-*` checks are the answer to the "obviously vibe-coded" critique.
They do not claim to identify AI authorship. They identify reviewable
facets that make a codebase look unreviewed:

- documented APIs with placeholder bodies
- adapters or clients with multiple stub signals
- completion/security claims sitting beside TODOs or placeholders
- declared error taxonomies with weak evidence of use in critical paths
- broad silent error handling
- tests that only smoke-run with thin assertions
- duplicated boilerplate across modules
- comment-heavy files where prose may outrun implementation

## Run It

From the `mai/` directory:

```powershell
python tools/local_gitdoctor_scan.py --root . --output docs/LOCAL-GITDOCTOR-REPORT.md
```

For machine-readable output:

```powershell
python tools/local_gitdoctor_scan.py --root . --format json --output docs/LOCAL-GITDOCTOR-REPORT.json
```

To make high-severity findings fail a local gate:

```powershell
python tools/local_gitdoctor_scan.py --root . --fail-on high
```

## Repository Hook Enforcement

Local GitDoctor scans are manual evidence tools, not repository hooks. The
commit protocol only installs the integrity pre-commit guard from
`.integrity/hooks`.

Install the repo hooks from `mai/` with:

```powershell
git config core.hooksPath .integrity/hooks
```

## Three Evidence Layers

For the more defensible J-14 package, use the evidence runner:

```powershell
python tools/local_gitdoctor_evidence.py --root . --output docs/LOCAL-GITDOCTOR-EVIDENCE.md --json-output docs/LOCAL-GITDOCTOR-EVIDENCE.json
```

It separates the audit into three layers:

1. Mapped checks: the local scanner rules that intentionally mirror
   John's GitDoctor finding families.
2. Independent implementations: mature tools such as `cargo`, `ruff`,
   `bandit`, `pip-audit`, `npm audit`, `gitleaks`, `hadolint`, `tokei`,
   `scc`, or `radon` when they are installed locally.
3. Adversarial fixtures: known-bad and known-clean scanner tests that
   prove the rules detect behavior, not just the current MAI symptom list.

The fixture directory `tools/local_gitdoctor_tests/` is intentionally
excluded from normal mapped repository scoring. Those files contain
known-bad samples by design, so they belong only in Layer 3 fixture
evidence, not in the MAI remediation score.

`SKIPPED` independent probes are not passes. They mean the tool was not
installed locally or the relevant project surface was absent.

For a fast proof of the mapped and adversarial layers only:

```powershell
python tools/local_gitdoctor_evidence.py --root . --skip-independent
```

## J-13 Usage

After J-13 lands and before the external J-14 GitDoctor run:

1. Run the three-layer evidence command above.
2. Review every HIGH or CRITICAL finding.
3. Fix real findings before J-14.
4. For intentional false positives, add evidence to the Dougherty
   response draft rather than deleting the finding.

The report should be attached to the J-14 evidence directory alongside
the external screenshots.
