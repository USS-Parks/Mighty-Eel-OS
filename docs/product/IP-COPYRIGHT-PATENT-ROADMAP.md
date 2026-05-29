# Island Mountain AI IP, Copyright, And Patent Roadmap

**Project:** Lamprey MAI / Island Mountain Mighty Eel OS  
**Owner:** Island Mountain AI  
**Purpose:** Step-by-step process for protecting the Lamprey MAI code,
docs, demos, inventions, and confidential know-how before external
testing, acquisition review, or release.  
**Audience:** Island Mountain AI leadership, counsel, release engineers,
technical reviewers.  
**Status:** owner-side process guide.  
**Last updated:** 2026-05-23.

> **Important:** This is not legal advice. It is a practical engineering
> and release-readiness checklist to take to qualified IP counsel. Patent,
> copyright, trade secret, employment, contractor, assignment, export,
> and tribal-data questions should be reviewed by attorneys before broad
> external disclosure.

---

## 0. Plain-English Summary

Do not treat IP protection as something that happens after deployment.
For Lamprey MAI, IP protection should run **beside** the deployment
roadmap.

The simple rule is:

> Protect first. Share second.

Before sending RC1 to outside testers, acquirers, investors, or public
demo audiences, Island Mountain AI should know:

1. What is copyrightable.
2. What may be patentable.
3. What should remain trade secret.
4. Who owns each contribution.
5. What can be safely disclosed.
6. What must stay under NDA.
7. Which filings, notices, and records exist.

---

## 1. What Each IP Bucket Means

### Copyright

Copyright protects the actual expression of the work: source code,
documentation, diagrams, demo scripts, dashboard UI text, READMEs,
architecture docs, and similar authored materials.

For this project, copyright is the baseline protection layer for:

- Rust source code
- Python source code
- dashboard code
- SDK code and documentation
- architecture documents
- acquisition documents
- demo scripts
- diagrams
- README and runbook text
- release packaging text

Useful official references:

- U.S. Copyright Office registration portal:
  <https://copyright.gov/registration/>
- U.S. Copyright Office computer program circular:
  <https://www.copyright.gov/circs/circ61.pdf>
- U.S. Copyright Office copyright basics:
  <https://copyright.gov/what-is-copyright/>

### Patent

Patents protect inventions, systems, methods, technical processes, and
novel mechanisms. They do not protect the mere idea of "an AI compliance
system"; they protect specific technical claims if those claims are new,
useful, and non-obvious enough.

For this project, patent review should focus on Lamprey and Trust
Manifold mechanisms, not on generic application packaging.

Useful official references:

- USPTO patent application types:
  <https://www.uspto.gov/patents/basics/types-patent-applications>
- USPTO provisional application overview:
  <https://www.uspto.gov/patents-getting-started/patent-basics/types-patent-applications/provisional-application-patent>
- USPTO utility patent guide:
  <https://www.uspto.gov/UtilityPatentGuide>

### Trade Secret

Trade secrets protect valuable information that is not generally known
and that Island Mountain AI takes reasonable steps to keep secret.

For this project, trade secret review should focus on:

- scoring heuristics
- policy precedence details not needed for public docs
- acquisition positioning
- buyer-specific integration tactics
- security hardening details
- release packaging procedures
- operational runbooks
- deployment validation details
- non-public test evidence

Useful official reference:

- USPTO IP toolkits:
  <https://www.uspto.gov/learning-and-resources/inventors-and-entrepreneurs/ip-basic-toolkits>

### Trademark

Trademark protects names, logos, slogans, and brand identifiers.

For this project, likely candidates include:

- Island Mountain AI
- Lamprey
- MAI
- Trust Manifold
- Mighty Eel OS

Trademark can run on a separate track, but it should be discussed with
counsel before public release.

---

## 2. Release Gate Rule

Use this rule for all deployment planning:

| Release stage | IP action required before sharing |
|---|---|
| Internal dev only | Keep contributor records and code history clean |
| RC1 to trusted tester | NDA, ownership check, copyright notice, patent screen |
| RC1 to acquirer | NDA, patent counsel review, invention summaries, clean source manifest |
| Public demo | File patent-sensitive material first or remove technical detail |
| Public code release | Copyright registration and open-source/license review first |
| Customer production release | Final IP notices, licenses, assignment records, and export/compliance review |

If in doubt, slow down and ask counsel before sharing the package.

---

## 3. IP Session Roadmap

## IP-01: Ownership And Contributor Inventory

**Goal:** Confirm Island Mountain AI owns or has rights to everything in
the release package.

**Work:**

- List every human and organization that contributed to the project.
- Identify employee, contractor, advisor, agent, or AI-assisted work.
- Confirm written assignment agreements exist for human contributors.
- Confirm contractor agreements include work-made-for-hire and IP
  assignment language where needed.
- Identify third-party libraries and generated assets.
- List any code copied from external sources.
- List any docs, diagrams, or demos derived from external materials.

**Outputs:**

- `IP-OWNERSHIP-INVENTORY.md`
- contributor list
- third-party dependency list
- missing assignment checklist

**Acceptance:**

- Island Mountain AI can explain who created each major asset.
- Any missing assignment paperwork is identified before external
  release.

---

## IP-02: Confidentiality And Disclosure Control

**Goal:** Prevent accidental public disclosure before patent and trade
secret decisions are made.

**Work:**

- Mark the repo and release packages confidential.
- Add confidentiality notices to tester docs.
- Decide who can receive RC1.
- Prepare NDA process for testers, acquirers, advisors, and contractors.
- Create a disclosure log.
- Record every external share: date, recipient, package version, NDA
  status, and purpose.

**Outputs:**

- `IP-DISCLOSURE-LOG.md`
- tester NDA packet checklist
- acquirer disclosure checklist

**Acceptance:**

- No external party receives RC1 without a recorded disclosure decision.
- Patent-sensitive technical details are not publicly posted.

---

## IP-03: Copyright Asset Inventory

**Goal:** Identify the copyrightable works Island Mountain AI may want to
register.

**Work:**

- Inventory source code by module.
- Inventory docs by category.
- Inventory demos and scripts.
- Inventory UI/dashboard assets.
- Inventory generated images, diagrams, and presentation materials.
- Identify unpublished vs published status.
- Decide whether to register code, docs, or both.

**Candidate copyright groups:**

- Lamprey MAI source code package
- Lamprey compliance documentation package
- acquisition and demo documentation package
- dashboard UI and operator docs
- SDK code and SDK documentation

**Outputs:**

- `COPYRIGHT-ASSET-INVENTORY.md`
- registration candidate list
- excluded material list

**Acceptance:**

- Counsel can tell what should be registered and what should be left out.
- Third-party material is identified before deposit copies are prepared.

---

## IP-04: Copyright Registration Prep

**Goal:** Prepare clean copyright registration materials.

**Work:**

- Choose registration targets with counsel.
- Prepare deposit copies.
- Remove secrets, keys, credentials, private customer data, and regulated
  payloads from any deposit.
- Decide whether redacted code deposits are appropriate.
- Record version, date, commit hash, and package manifest.
- Prepare owner, author, claimant, publication, and limitation details.

**Outputs:**

- copyright registration worksheet
- clean deposit package
- source manifest with commit hash

**Acceptance:**

- Copyright materials are ready for counsel or direct filing through the
  U.S. Copyright Office registration portal.
- Deposit material does not leak secrets or regulated data.

---

## IP-05: Patent Candidate Inventory

**Goal:** Convert the engineering IP memo into attorney-reviewable
invention records.

**Work:**

- Review `mai/docs/acquisition/IP.md`.
- Extract each patent candidate into a separate invention disclosure.
- Identify inventors for each candidate.
- Record first conception date if known.
- Record reduction-to-practice evidence: commits, tests, docs, demos.
- List prior art already known to the team.
- List public disclosures, private disclosures, demos, or investor
  conversations.

**Initial candidate inventions:**

- multi-domain compliance routing for AI inference
- OCAP-native AI inference governance engine
- hash-chained AI compliance audit with PQC signatures
- hardware-aware compliance routing with air-gap enforcement
- Trust Manifold signed-bundle flow excluding regulated payloads
- certified compliance report generation for AI decisions

**Outputs:**

- `PATENT-CANDIDATE-INVENTORY.md`
- one invention disclosure per candidate
- known prior-art notes
- inventor list

**Acceptance:**

- Patent counsel can evaluate each candidate without reverse-engineering
  the whole repo.
- The team knows which candidates are urgent before disclosure.

---

## IP-06: Patent Counsel Review

**Goal:** Decide what to file, what to keep secret, and what to abandon.

**Work with counsel to decide:**

- Which candidates are worth filing.
- Whether to file provisional applications first.
- Whether any candidate should remain a trade secret.
- Whether any disclosure has already happened.
- Whether foreign filing rights matter.
- Whether open-source dependencies create claim or licensing issues.
- Whether tribal-data and OCAP terminology needs special handling.

**Outputs:**

- counsel review memo
- file / trade-secret / abandon decision table
- filing priority order

**Acceptance:**

- Island Mountain AI knows which inventions must be protected before
  external release.
- Public/demo materials can be edited to avoid premature disclosure.

---

## IP-07: Provisional Patent Filing Package

**Goal:** Prepare filing-ready provisional materials for selected
inventions.

**Work:**

- Draft technical description for each invention.
- Include system diagrams.
- Include method flows.
- Include examples from demos.
- Include alternative embodiments.
- Include implementation references from the codebase.
- Include benefits and problem solved.
- Include known prior art and differences.
- Confirm inventor names.
- Confirm owner/assignee.

**Outputs:**

- provisional filing packet per selected candidate
- figure list
- invention summary
- source-code appendix if counsel wants it

**Acceptance:**

- Counsel has enough detail to file.
- Filing happens before any public disclosure of the protected details.

---

## IP-08: Nonprovisional Patent Calendar

**Goal:** Prevent provisional filings from expiring unnoticed.

**Work:**

- Record provisional filing dates.
- Record 12-month nonprovisional deadlines.
- Create reminder schedule at 3, 6, 9, and 11 months.
- Decide whether international filing strategy matters.
- Track product changes that should be added to later filings.

**Outputs:**

- `PATENT-DEADLINE-CALENDAR.md`
- filing receipt archive
- next-action reminders

**Acceptance:**

- No provisional deadline depends on memory.
- The team knows when nonprovisional decisions are due.

---

## IP-09: Trade Secret Register

**Goal:** Define what Island Mountain AI is intentionally keeping secret.

**Work:**

- Identify non-public technical and business information.
- Mark each item as confidential, restricted, or shareable under NDA.
- Decide what should not be included in RC1.
- Restrict access to sensitive docs.
- Add release-package exclusions for trade-secret-only material.
- Document reasonable secrecy steps.

**Outputs:**

- `TRADE-SECRET-REGISTER.md`
- release exclusion list
- access-control notes

**Acceptance:**

- The team can explain what trade secrets exist and how they are
  protected.
- Trade-secret materials are not accidentally sent to broad tester
  groups.

---

## IP-10: License And Dependency Review

**Goal:** Make sure third-party code does not create release or
acquisition problems.

**Work:**

- Generate Rust dependency list.
- Generate Python dependency list.
- Identify licenses for all dependencies.
- Flag copyleft, network-copyleft, commercial, unknown, or incompatible
  licenses.
- Confirm obligations for notices, source availability, attribution, and
  redistribution.
- Review generated artifacts and vendored files.

**Outputs:**

- `THIRD-PARTY-LICENSE-REVIEW.md`
- license notice file
- dependency risk table

**Acceptance:**

- RC1 includes required notices.
- No unknown high-risk license is silently shipped.

---

## IP-11: Brand And Trademark Review

**Goal:** Protect the public-facing names before launch.

**Work:**

- List product and company names.
- Search for obvious conflicts.
- Ask counsel whether trademark clearance is needed.
- Decide what names are internal code names vs public brands.
- Add trademark notices where appropriate.
- Decide whether to file trademark applications.

**Candidate marks:**

- Island Mountain AI
- Lamprey
- MAI
- Trust Manifold
- Mighty Eel OS

**Outputs:**

- `BRAND-TRADEMARK-REVIEW.md`
- public naming decision
- trademark filing recommendation

**Acceptance:**

- Public materials use approved names consistently.
- Risky or confusing names are flagged before launch.

---

## IP-12: AI-Assisted Work Record

**Goal:** Preserve a clean record of how AI tools were used.

**Work:**

- Document AI-assisted development workflows.
- Identify which artifacts were human-authored, AI-assisted, or
  machine-generated.
- Record human selection, editing, arrangement, and authorship decisions
  for key docs and source modules.
- Keep prompt/session logs where appropriate.
- Ask counsel how to handle copyright claims involving AI-assisted
  works.

**Outputs:**

- `AI-ASSISTED-WORK-RECORD.md`
- authorship notes for major docs and code areas

**Acceptance:**

- Copyright registration prep can answer AI-authorship questions
  honestly.
- The project has a defensible human-authorship record.

---

## IP-13: RC1 IP Gate

**Goal:** Decide whether RC1 can be shared with outside testers.

**Required before pass:**

- Ownership inventory complete.
- Disclosure control process ready.
- Copyright candidates identified.
- Patent candidates screened.
- Counsel has reviewed urgent patent risks or the team has chosen not to
  disclose sensitive details.
- NDA/tester packet ready.
- Third-party license review complete enough for private tester release.
- RC1 package has copyright and confidentiality notices.

**Outputs:**

- `RC1-IP-GATE.md`
- go/no-go decision
- allowed-recipient list
- blocked-disclosure list

**Acceptance:**

- RC1 is not sent externally until this gate passes.

---

## IP-14: Acquirer Disclosure Package

**Goal:** Prepare a cleaner, higher-control package for serious buyers
or strategic partners.

**Work:**

- Prepare invention summary packet.
- Include patent filing status without over-disclosing unfiled claims.
- Include copyright registration status.
- Include trade-secret handling statement.
- Include third-party license summary.
- Include clean source manifest.
- Include disclosure log.
- Include existing acquisition IP memo.

**Outputs:**

- `ACQUIRER-IP-PACKAGE.md`
- clean room source manifest
- IP status table

**Acceptance:**

- Acquirers can evaluate the IP story without receiving uncontrolled
  secrets.
- Island Mountain AI can show disciplined ownership and filing posture.

---

## IP-15: Public Release Gate

**Goal:** Decide what can be shown publicly.

**Required before pass:**

- Patent filings made or sensitive details removed.
- Copyright registration strategy complete.
- Trademark/name review complete.
- Public docs scrubbed for secrets.
- Export/compliance concerns reviewed.
- License notices included.
- No regulated payloads, customer data, keys, or secret configs included.

**Outputs:**

- `PUBLIC-RELEASE-IP-GATE.md`
- public-safe feature description
- public-safe demo script
- redaction checklist

**Acceptance:**

- Public release does not accidentally destroy patent options or expose
  trade secrets.

---

## 4. Documents To Create During This Track

Recommended IP folder:

```text
mai/docs/ip/
  IP-OWNERSHIP-INVENTORY.md
  IP-DISCLOSURE-LOG.md
  COPYRIGHT-ASSET-INVENTORY.md
  PATENT-CANDIDATE-INVENTORY.md
  PATENT-DEADLINE-CALENDAR.md
  TRADE-SECRET-REGISTER.md
  THIRD-PARTY-LICENSE-REVIEW.md
  BRAND-TRADEMARK-REVIEW.md
  AI-ASSISTED-WORK-RECORD.md
  RC1-IP-GATE.md
  ACQUIRER-IP-PACKAGE.md
  PUBLIC-RELEASE-IP-GATE.md
```

Do not put secrets, private keys, customer data, or regulated payloads in
these files.

---

## 5. Release Notices To Add

At minimum, RC1 should include:

```text
Copyright (c) 2026 Island Mountain AI. All rights reserved.

Lamprey MAI, Trust Manifold, Mighty Eel OS, and related documentation
are confidential and proprietary to Island Mountain AI unless otherwise
stated in a written agreement.

No rights are granted except as expressly provided in a signed agreement
with Island Mountain AI.
```

Counsel should review final notice language before external release.

---

## 6. Practical Do-Not-Do List

Do not:

- publish the repo publicly before patent review
- send RC1 without NDA/disclosure logging
- include `target/debug/` in release packages
- include secrets, keys, credentials, or real regulated data
- post detailed diagrams of unfiled patent candidates publicly
- describe unfiled inventions in public marketing copy
- treat "patent pending" as true before a filing exists
- assume copyright registration and patent filing are the same thing
- assume AI-assisted material has no ownership questions
- ignore third-party licenses

---

## 7. Immediate Next Step

Run IP-01, IP-02, and IP-05 before sending RC1 to anyone outside the
trusted build circle.

The minimum safe sequence is:

1. Confirm ownership.
2. Control disclosure.
3. Identify patent candidates.
4. Ask counsel what must be filed before sharing.
5. Prepare NDA/tester packet.
6. Then release RC1 to the first trusted tester.

