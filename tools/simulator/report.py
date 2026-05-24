"""Render a trace-replay policy comparison as Markdown or JSON.

Consumes the JSON output of `replay_compare.py` and emits an
acquisition-friendly summary. The Markdown variant produces a comparison table
plus key headline metrics; the JSON variant is a passthrough useful for
machine consumption.

Usage:
    python report.py <comparison.json> [--format markdown|json] [--out path]
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

REPORT_COLUMNS: list[tuple[str, str]] = [
    ("policy", "Policy"),
    ("requests_total", "Requests"),
    ("completed", "Completed"),
    ("throughput_tokens_per_sec", "Throughput (tok/s)"),
    ("latency_ms_p50", "p50 (ms)"),
    ("latency_ms_p95", "p95 (ms)"),
    ("latency_ms_p99", "p99 (ms)"),
    ("evictions", "Evictions"),
    ("evictions_per_sec", "Evict/Req"),
    ("avg_kv_utilization_pct", "KV %"),
]


def _fmt(value) -> str:
    if isinstance(value, float):
        return f"{value:.2f}"
    return str(value)


def render_markdown(comparison: dict) -> str:
    """Render the comparison as a Markdown report."""
    policies = comparison.get("policies", {})
    lines: list[str] = []
    lines.append("# MAI Scheduler Trace Replay Comparison")
    lines.append("")
    lines.append(f"- **Trace:** `{comparison.get('trace_path', '?')}`")
    lines.append(f"- **Seed:** {comparison.get('seed', '?')}")
    lines.append(f"- **VRAM budget:** {comparison.get('vram_gb', '?')} GB")
    lines.append(f"- **Sim time:** {comparison.get('sim_time_secs', '?')} s")
    lines.append("")

    if not policies:
        lines.append("_No policy results were produced._")
        lines.append("")
        return "\n".join(lines)

    headers = [name for _, name in REPORT_COLUMNS]
    keys = [key for key, _ in REPORT_COLUMNS]
    lines.append("| " + " | ".join(headers) + " |")
    lines.append("|" + "|".join(["---"] * len(headers)) + "|")
    for report in policies.values():
        row = [_fmt(report.get(key, "")) for key in keys]
        lines.append("| " + " | ".join(row) + " |")
    lines.append("")

    best = find_best(policies)
    if best:
        lines.append("## Headline Findings")
        lines.append("")
        lines.append(f"- **Highest throughput:** `{best['throughput']}`")
        lines.append(f"- **Best p95 latency:** `{best['p95']}`")
        lines.append(f"- **Fewest evictions:** `{best['evictions']}`")
        lines.append("")

    lines.append("## Notes")
    lines.append("")
    lines.append(
        "Replay is deterministic at a given (trace, seed, policy) triple, so "
        "all comparisons in this table can be reproduced from the inputs above."
    )
    lines.append("")
    return "\n".join(lines)


def find_best(policies: dict) -> dict | None:
    """Return the winning policy for each headline metric."""
    if not policies:
        return None

    def safe(report: dict, key: str, default: float = 0.0) -> float:
        value = report.get(key, default)
        try:
            return float(value)
        except (TypeError, ValueError):
            return default

    best_throughput = max(
        policies.items(), key=lambda kv: safe(kv[1], "throughput_tokens_per_sec")
    )
    best_p95 = min(
        policies.items(),
        key=lambda kv: safe(kv[1], "latency_ms_p95", default=float("inf")),
    )
    fewest_evictions = min(
        policies.items(),
        key=lambda kv: safe(kv[1], "evictions", default=float("inf")),
    )
    return {
        "throughput": best_throughput[0],
        "p95": best_p95[0],
        "evictions": fewest_evictions[0],
    }


def render_json(comparison: dict) -> str:
    return json.dumps(comparison, indent=2)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Render a replay_compare.py result as Markdown or JSON."
    )
    parser.add_argument("input", type=Path, help="Comparison JSON file.")
    parser.add_argument(
        "--format",
        choices=("markdown", "json"),
        default="markdown",
        help="Output format.",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=None,
        help="Output path (stdout if omitted).",
    )
    args = parser.parse_args(argv)

    if not args.input.exists():
        print(f"error: input {args.input} does not exist", file=sys.stderr)
        return 2

    comparison = json.loads(args.input.read_text(encoding="utf-8"))
    text = (
        render_markdown(comparison)
        if args.format == "markdown"
        else render_json(comparison)
    )
    if args.out:
        args.out.write_text(text, encoding="utf-8")
        print(f"wrote {args.format} report to {args.out}")
    else:
        sys.stdout.write(text)
        if not text.endswith("\n"):
            sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
