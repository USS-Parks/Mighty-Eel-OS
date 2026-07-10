"""Deployment profile validator for the AOG/WSF appliance trust plane.

Signature-based check of a docker-compose file for trust-plane hardening
defects. Two profiles:

- ``production``: a composition claimed to be production-grade must expose no
  dev-mode trust core, no known/literal root credential, no un-injected secret,
  and must not publish ANY trust-core port to the host (API, cluster, or
  remapped).
- ``demo``: a demo composition may run a dev-mode trust core, but it must be
  gated behind an explicit compose profile, inject its root token from the
  environment (never a baked literal), and may only bind trust-core ports to
  the loopback interface.

A trust core is recognized by its image name, service/container name,
server-only environment keys (``BAO_LOCAL_CONFIG`` etc.), or a ``bao`` /
``vault`` launch token in the entrypoint or command — an unmarked retagged
image does not evade the check.

The validator reports the exact unsafe construct (service + evidence) and exits
non-zero when any violation is found. Mapped audit finding: AF-12 (appliance
publishes dev OpenBao with a known root token). Containment owner: PSPR-01.
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import yaml

TRUST_IMAGE_MARKERS = ("openbao", "vault")
# Environment keys only a trust-core SERVER sets (clients carry *_ADDR
# instead). Lets the validator catch a core whose image name hides the
# marker (custom registry, retagged build).
TRUST_SERVER_ENV_MARKERS = (
    "BAO_LOCAL_CONFIG",
    "VAULT_LOCAL_CONFIG",
    "BAO_DEV_ROOT_TOKEN_ID",
    "VAULT_DEV_ROOT_TOKEN_ID",
)
LOOPBACK_HOSTS = ("127.0.0.1", "::1", "localhost")
# Literal root-credential values that must never appear in any composition.
BANNED_LITERAL_VALUES = frozenset(
    {"root", "dev", "test", "changeme", "admin", "insecure", "example", "password"}
)
# Environment keys that carry trust-plane credentials.
CREDENTIAL_ENV_SUFFIXES = ("_TOKEN", "_ROOT_TOKEN", "_ROOT_TOKEN_ID", "_SECRET_ID")


@dataclass(frozen=True)
class Violation:
    """A single trust-plane hardening defect found in a composition."""

    rule: str
    service: str
    detail: str

    def __str__(self) -> str:
        return f"[{self.rule}] service '{self.service}': {self.detail}"


def _as_token_list(command: Any) -> list[str]:
    if isinstance(command, str):
        return command.split()
    if isinstance(command, list):
        tokens: list[str] = []
        for part in command:
            tokens.extend(str(part).split())
        return tokens
    return []


def _as_env_pairs(environment: Any) -> list[tuple[str, str]]:
    if isinstance(environment, dict):
        return [(str(k), "" if v is None else str(v)) for k, v in environment.items()]
    if isinstance(environment, list):
        pairs: list[tuple[str, str]] = []
        for item in environment:
            key, sep, value = str(item).partition("=")
            pairs.append((key, value if sep else ""))
        return pairs
    return []


def _port_to_str(entry: Any) -> str:
    if isinstance(entry, dict):
        host_ip = entry.get("host_ip", "")
        published = entry.get("published", "")
        target = entry.get("target", "")
        prefix = f"{host_ip}:" if host_ip else ""
        return f"{prefix}{published}:{target}"
    return str(entry)


def _as_ports(ports: Any) -> list[str]:
    if isinstance(ports, list):
        return [_port_to_str(p) for p in ports]
    return []


def _is_literal(value: str) -> bool:
    """True when the value is a baked literal, not an env/secret reference."""
    stripped = value.strip()
    return bool(stripped) and "${" not in stripped


def _is_trust_core(name: str, service: dict[str, Any]) -> bool:
    image = str(service.get("image", "")).lower()
    if any(marker in image for marker in TRUST_IMAGE_MARKERS):
        return True
    names = (name, str(service.get("container_name", "")))
    if any(n.lower() in TRUST_IMAGE_MARKERS for n in names if n):
        return True
    env_keys = {key.upper() for key, _ in _as_env_pairs(service.get("environment"))}
    if env_keys.intersection(TRUST_SERVER_ENV_MARKERS):
        return True
    tokens = _launch_tokens(service)
    return any(
        tok in ("bao", "vault") or tok.endswith(("/bao", "/vault")) for tok in tokens
    )


def _launch_tokens(service: dict[str, Any]) -> list[str]:
    """Every token the container launches with: entrypoint + command."""
    return _as_token_list(service.get("entrypoint")) + _as_token_list(service.get("command"))


def _host_published(port_str: str) -> tuple[bool, str, str] | None:
    """Return (is_loopback, host_ip, target) when this mapping publishes to the host."""
    parts = port_str.split(":")
    if len(parts) < 2:
        return None  # container-internal only; not host-published
    target = parts[-1].split("/")[0]
    host_ip = parts[0] if len(parts) >= 3 else ""
    return (host_ip in LOOPBACK_HOSTS, host_ip or "all-interfaces", target)


def _check_trust_core(name: str, svc: dict[str, Any], profile: str) -> list[Violation]:
    out: list[Violation] = []
    tokens = _launch_tokens(svc)
    dev_flags = [t for t in tokens if t == "-dev" or t.startswith("-dev-")]
    if profile == "production" and dev_flags:
        out.append(Violation("dev-mode", name, f"trust core runs dev mode: {dev_flags}"))

    for tok in tokens:
        if tok.startswith("-dev-root-token-id="):
            value = tok.split("=", 1)[1]
            if not _is_literal(value):
                continue  # injected via ${...}; acceptable for a demo
            out.append(Violation("known-token", name, f"root token baked in command: {tok!r}"))
            if value.lower() in BANNED_LITERAL_VALUES:
                out.append(Violation("weak-token", name, f"known-weak root token: {value!r}"))

    # Any host-published port on a trust core is an exposure — the API port,
    # the cluster port, or a remapped one. Production: none at all. Demo:
    # loopback only.
    for port_str in _as_ports(svc.get("ports")):
        published = _host_published(port_str)
        if published is None:
            continue
        is_loopback, host_ip, target = published
        if profile == "production":
            out.append(Violation("host-published-trust", name,
                                  f"trust-core port {target} host-published {port_str!r}; "
                                  "production trust core must not be host-exposed on any port"))
        elif not is_loopback:
            out.append(Violation("trust-exposed-nonloopback", name,
                                  f"demo trust-core port {target} on non-loopback '{host_ip}' "
                                  f"{port_str!r}; bind 127.0.0.1 only"))

    if profile == "demo" and not (svc.get("profiles") or []):
        out.append(Violation("demo-not-gated", name,
                             "demo trust core lacks an explicit compose profile "
                             "(production could inherit it)"))
    return out


def _check_credentials(name: str, svc: dict[str, Any]) -> list[Violation]:
    out: list[Violation] = []
    for key, value in _as_env_pairs(svc.get("environment")):
        upper = key.upper()
        if not any(upper.endswith(sfx) for sfx in CREDENTIAL_ENV_SUFFIXES):
            continue
        if _is_literal(value):
            out.append(Violation("credential-not-injected", name,
                                 f"'{key}' baked literal {value!r}; inject via env/secret"))
            if value.lower() in BANNED_LITERAL_VALUES:
                out.append(Violation("weak-credential", name,
                                     f"'{key}' is a known-weak literal {value!r}"))
    return out


def validate(compose: dict[str, Any], profile: str) -> list[Violation]:
    """Return every trust-plane violation for the given profile."""
    if profile not in ("production", "demo"):
        raise ValueError(f"unknown profile {profile!r}; expected 'production' or 'demo'")
    services = compose.get("services") or {}
    violations: list[Violation] = []
    for name, svc in services.items():
        if not isinstance(svc, dict):
            continue
        if _is_trust_core(name, svc):
            violations.extend(_check_trust_core(name, svc, profile))
        violations.extend(_check_credentials(name, svc))
    return violations


def load_compose(path: Path) -> dict[str, Any]:
    data = yaml.safe_load(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise TypeError(f"{path}: top level is not a mapping")
    return data


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Validate an appliance compose file for trust-plane safety."
    )
    parser.add_argument("compose", type=Path, help="path to a docker-compose file")
    parser.add_argument("--profile", choices=("production", "demo"), default="production")
    args = parser.parse_args(argv)
    try:
        compose = load_compose(args.compose)
    except (OSError, TypeError, ValueError, yaml.YAMLError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    violations = validate(compose, args.profile)
    if violations:
        print(f"REJECTED ({args.profile}): {len(violations)} violation(s) in {args.compose}",
              file=sys.stderr)
        for viol in violations:
            print(f"  {viol}", file=sys.stderr)
        return 1
    print(f"OK ({args.profile}): {args.compose} passes trust-plane checks")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
