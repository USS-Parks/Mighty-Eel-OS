# MAI Installation Guide

Operator-facing install procedure for a `ship`-profile appliance.
For developer builds and `local-dev` workflows see
[BUILD.md](../BUILD.md); for the package internals see
[../packaging/README.md](../packaging/README.md).

## Audience

This document is for the operator who installs the appliance.
Engineering and security-architecture reviewers should start at
[ACQUISITION-PACKAGE.md](../product/ACQUISITION-PACKAGE.md).

## Hardware prerequisites

| Item | Minimum | Notes |
|---|---|---|
| GPU | 1× NVIDIA H100/H200, A100 80GB, or L40S | The `pack-leader` tier expects 4–8× H100 |
| CPU | 16 cores | Adapter subprocesses + audit hashing |
| RAM | 64 GB | More for `pack-leader` |
| Disk: OS / binaries | 50 GB | `/usr` |
| Disk: state | 200 GB ZFS pool | `/var/lib/mai`; vault expects ZFS |
| Disk: backups | sized per retention | separate filesystem strongly recommended |
| Network | 1× management NIC | TLS terminates at a reverse proxy in front of MAI |

## Software prerequisites

- Debian 12 (or a derivative on equivalent kernel).
- NVIDIA driver installed at the host layer; MAI does not ship
  drivers.
- ZFS userland + a pre-created pool / dataset for the vault.
- `debhelper`, `dpkg-dev`, `fakeroot` only on the build host;
  not required on the appliance.
- A reverse proxy of the operator's choice (nginx, Caddy,
  HAProxy) terminating TLS; MAI binds loopback only.

## Install procedure

### 1. Drop the package on the host

The Debian package is the only supported install artifact in
ship. See [../packaging/README.md](../packaging/README.md) for
the build process; the package itself is signed and delivered
out-of-band.

```bash
sudo apt install ./mai_<version>_amd64.deb
```

`preinst` stops any prior services. `postinst` creates the
`mai:mai` user/group, the state directories under
`/var/lib/mai/`, the conffile templates under `/etc/mai/`, and
the install layout described in
[../packaging/README.md](../packaging/README.md). **No services
are auto-enabled.**

### 2. Lay down the profile

```bash
sudo $EDITOR /etc/mai/profile.toml
```

Use [`config/production.example.toml`](../config/production.example.toml)
as the template. Every section of `[profile]`, `[paths]`,
`[vault]`, `[audit]`, `[trust]`, `[auth]`, `[dashboard]`,
`[network]`, and `[observability]` has a contract documented
inline. See [SHIP-PROFILE.md](SHIP-PROFILE.md) for the per-section
enforcement table.

### 3. Drop trust anchors

The trust anchor pub keys for the signers your fleet trusts go
under `/etc/mai/trust-anchors/`. Files must be readable by
`mai`:

```bash
sudo install -o root -g mai -m 0640 \
     my-signer.pub /etc/mai/trust-anchors/
```

The daemon refuses to boot without at least one anchor whose
fingerprint matches the bundle you will install in step 5.

### 4. Validate the layout

```bash
sudo mai-ship-validate --profile /etc/mai/profile.toml
```

Exit `0` is required. Any non-zero exit prints the failing
`PROD-*` check ID; fix the underlying issue and re-run. The
validator is the only gate; never bypass it. See
[RELEASE-GATES.md](../releases/RELEASE-GATES.md) for the full check matrix.

### 5. First boot and key capture

Follow runbook [01-first-boot-and-key-capture](runbooks/01-first-boot-and-key-capture.md)
to capture the printed admin API key, persist its hash, and
enable the unit.

### 6. Install the initial policy bundle

Out-of-band, signed by an anchor installed in step 3. Follow
runbook [04-install-policy-bundle](runbooks/04-install-policy-bundle.md).

### 7. Stand up the reverse proxy

MAI binds loopback. The reverse proxy in front terminates TLS,
forwards `Host` and `X-IM-Auth-Token` headers verbatim, and
should not buffer SSE streams. Configuration templates for
nginx and Caddy live under [`deployment/`](../deployment/).

### 8. First backup

Run runbook [07-back-up-node](runbooks/07-back-up-node.md)
immediately after install completes — even before the appliance
serves any client traffic. The first backup is the floor under
every later recovery.

## What "installed" means

The appliance is installed when **all** of the following hold:

- `mai-ship-validate --profile /etc/mai/profile.toml` exits 0.
- `systemctl is-active mai-api.service mai-dashboard.service`
  returns `active` for both.
- `curl -fsS http://127.0.0.1:8420/v1/health/ready` returns 200
  with `status = "ready"`.
- `mai-admin audit verify --wal-dir /var/lib/mai/audit` exits 0.
- A backup exists under `/var/backups/mai/` and
  `mai-admin backup verify <dir>` exits 0.

Anything short of that is "partially installed", not installed.

## Removing

```bash
sudo apt remove mai      # services off, binaries gone; state kept
sudo apt purge mai       # state under /var/lib/mai + /etc/mai wiped
```

Purge wipes the audit chain and vault snapshots. Do not purge
without first preserving the backups offsite.

## See also

- [FIRST-BOOT.md](FIRST-BOOT.md) — the long-form companion to
  runbook 01.
- [SHIP-PROFILE.md](SHIP-PROFILE.md) — per-section profile
  contract.
- [SECURITY-PRODUCTION.md](../compliance/SECURITY-PRODUCTION.md) — key store,
  rotation cadence, anchor custody.
- [DEPLOYMENT.md](DEPLOYMENT.md) — local-dev quick start (not
  for production).
- [../packaging/README.md](../packaging/README.md) — package
  internals, filesystem layout, systemd policy.
