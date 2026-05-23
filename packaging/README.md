# MAI Packaging

This directory is the source of truth for what Island Mountain ships to a
customer. The Debian package, the systemd units, the install-time
filesystem layout, and the operator's day-1 commands all live here.

Read this before changing any service unit or post-install script. Tests
in `tools/packaging_tests/` keep the layout, the units, the maintainer
scripts, and `scripts/build-package.{sh,ps1}` in sync — if a test fails,
fix the source-of-truth file rather than weakening the test.

## Layout

```
packaging/
├── README.md                 — this file
├── debian/                   — Debian package metadata (control, rules, ...)
│   ├── control
│   ├── changelog
│   ├── copyright
│   ├── rules
│   ├── install               — staging→install path map
│   ├── conffiles             — files preserved on upgrade
│   ├── compat                — debhelper compat level
│   └── source/format         — native v3
├── systemd/                  — installed to /lib/systemd/system/
│   ├── mai-api.service
│   ├── mai-dashboard.service
│   ├── mai-adapter-manager.service
│   ├── mai-healthcheck.service
│   └── mai-healthcheck.timer
└── scripts/                  — installed to /usr/lib/mai/scripts/
    ├── preinstall.sh         — copied to debian/preinst at build time
    ├── postinstall.sh        — copied to debian/postinst at build time
    ├── preremove.sh          — copied to debian/prerm at build time
    ├── postremove.sh         — copied to debian/postrm at build time
    ├── mai-ship-validate.sh  — installed as /usr/bin/mai-ship-validate
    └── mai-healthcheck.sh    — invoked by mai-healthcheck.service
```

The build orchestrator (`scripts/build-package.sh` on Linux,
`scripts/build-package.ps1` for local Windows dev validation) assembles
the staging tree under `build/package-staging/`, then either hands off
to `dpkg-buildpackage` or stops there for inspection.

## Filesystem layout on a production host

```
/usr/bin/mai-api                       — REST + gRPC inference server
/usr/bin/mai-ship-validate             — readiness validator
/usr/lib/mai/adapters/                 — LoRA/QLoRA artifacts
/usr/lib/mai/compliance-dashboard/     — FastAPI + uvicorn + .venv
/usr/lib/mai/scripts/                  — internal helpers (healthcheck etc.)
/etc/mai/profile.toml                  — production guard contract
/etc/mai/auth_keys.toml                — API key store (seeded by operator)
/etc/mai/dashboard-logging.json        — uvicorn log config
/etc/mai/policies/                     — signed Lamprey policy bundles
/etc/mai/trust-anchors/                — ML-DSA-87 trust anchors (.pub)
/var/lib/mai/vault/                    — ZFS vault root (encryption-at-rest)
/var/lib/mai/audit/                    — append-only API audit WAL
/var/lib/mai/trust/                    — bundle cache (signed boot bundle)
/var/lib/mai/models/                   — quantized model files
/var/lib/mai/reports/                  — compliance reports
/var/log/mai/                          — structured logs (JSON sink)
/run/mai/                              — runtime sockets / pid files
/var/backups/mai/                      — backup artifacts (SHIP-09)
/lib/systemd/system/mai-*.service      — unit files (and timer)
```

Ownership: everything mai writes is owned `mai:mai`. Config files in
`/etc/mai/` are `root:mai 0640` (operator owns the source of truth;
the daemon reads). State dirs are `mai:mai 0750`.

## Building a package

### On Linux (produces a .deb)

```bash
cargo --version          # 1.85+ required
python3 --version        # 3.11+ required
sudo apt install debhelper dpkg-dev fakeroot
scripts/build-package.sh --deb
ls -lh ../mai_*.deb
```

The script:

1. Cleans `build/package-staging/`.
2. Runs `cargo build --release --workspace --locked`.
3. Stages the install layout under `build/package-staging/`.
4. Vendors dashboard wheels into
   `usr/lib/mai/compliance-dashboard/wheels/` so postinstall can run
   pip in --no-index mode on an air-gapped host.
5. Copies the systemd units, config templates, docs.
6. Records `PACKAGE_BUILD_INFO` (version, git commit, build time).
7. Runs `mai-api validate --profile <staged>` against the staged
   profile. Non-zero exit aborts the build.
8. If `--deb` was passed, runs `dpkg-buildpackage -us -uc -b`.

Exit codes:

* `0` — staging tree built, validator passed
* `1` — cargo/python/staging failed
* `2` — production guard rejected the staged profile
* `3` — required tool missing on the build host

### On Windows (validation only — no .deb)

```powershell
scripts\build-package.ps1
```

This is for local development cross-checks. A real .deb requires the
Linux script above. The Windows path runs the same staging logic, so a
developer can verify "the systemd unit lines up with the build script"
without leaving Windows.

## Installing on a host (Debian)

```bash
sudo apt install ./mai_0.1.0-1_amd64.deb
# preinst stops any prior services, postinst creates mai user + state dirs
# Operator review:
sudo $EDITOR /etc/mai/profile.toml
sudo $EDITOR /etc/mai/auth_keys.toml
sudo cp my-trust-anchor.pub /etc/mai/trust-anchors/
sudo mai-ship-validate --profile /etc/mai/profile.toml
# exit 0 = ship-ready
sudo systemctl enable --now mai-api.service
sudo systemctl enable --now mai-dashboard.service
sudo systemctl enable --now mai-healthcheck.timer
```

`postinstall.sh` never auto-enables services. That's deliberate: the
operator must seed `/etc/mai/auth_keys.toml` and drop trust anchors
before the API can pass `mai-ship-validate`.

## Upgrading

```bash
sudo apt install ./mai_0.2.0-1_amd64.deb
```

`preinst stop` halts the running services, the new files drop in,
`postinst configure` reloads systemd and prints the next-steps banner.
State under `/var/lib/mai/` is preserved (it's not in conffiles, so
dpkg never touches it). Any edits to files in `/etc/mai/` are kept;
dpkg may prompt to merge changes since those paths are conffiles.

## Removing

```bash
sudo apt remove mai      # services stopped + binaries removed; state kept
sudo apt purge mai       # state under /var/lib/mai + /etc/mai wiped, mai user removed
```

The `prerm` stops + disables every unit. The `postrm` only deletes
customer data on `purge`. The `remove` branch prints a reminder that
`apt purge mai` is the way to wipe state.

## systemd unit policy

Every long-running unit declares:

* `User=mai`, `Group=mai`
* `NoNewPrivileges=true`, `PrivateTmp=true`, `ProtectSystem=strict`,
  `ProtectHome=true`, plus the rest of the `systemd-analyze security`
  hardening bundle
* `Restart=on-failure` with a small backoff
* `LimitNOFILE` sized for the role (`mai-api` 65536, dashboard 8192)
* `ReadWritePaths=` listing the exact dirs the daemon needs

`mai-api.service` has an `ExecStartPre=` that calls
`/usr/bin/mai-ship-validate --profile /etc/mai/profile.toml`. Non-zero
exit blocks startup; this is the same readiness gate the daemon runs
internally, surfaced at the unit level so systemd reports the failure
clearly instead of looping crash-restart cycles.

## Acceptance tests

Run from the repo root:

```bash
python -m pytest tools/packaging_tests/ -v
```

These tests are static (they don't invoke dpkg or systemctl) so they
run on any platform and any Python 3.11+ environment without
dependencies. They check:

* every systemd unit parses and declares the required hardening flags
* every Debian metadata file is consistent (install ↔ conffiles ↔ control)
* every maintainer script is idempotent and preserves customer data
* `build-package.sh` and `build-package.ps1` stage matching layouts
* every config template parses as TOML
* the filesystem layout in `SHIP-HARDENING-PLAN.md` §8 matches what
  the build script and postinstall actually produce

If you change a unit or a script, expect to update the matching test.

## Future work

The following are tracked elsewhere and intentionally out of scope here:

* `mai-admin backup`/`restore` commands — SHIP-09 / SHIP-10
* metrics endpoint + alert rule config — SHIP-11
* CI package build job + ship validator job — SHIP-12
* GPU release workflow + benchmark gates — SHIP-13
* 72-hour burn-in scripts + signed report — SHIP-14
* operator runbooks — SHIP-15

A standalone `mai-ship-validate` Cargo binary will replace the
`mai-api validate` subcommand wrapper that lives at
`packaging/scripts/mai-ship-validate.sh` today; the wrapper exists so
SHIP-08 packages have something installable at `/usr/bin/mai-ship-validate`
without bleeding scope into the SHIP-07 CLI work.
