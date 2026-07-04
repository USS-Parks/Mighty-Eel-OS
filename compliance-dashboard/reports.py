"""Report-management helpers.

Thin façade on top of the SDK's
:meth:`mai.client.MaiClient.compliance.generate_report` /
``list_reports`` / ``get_report`` / ``delete_report`` /
``download_report``. Handles parameter coercion for the dashboard's
form posts and groups report rows by status for the overview panel.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from mai.types import ComplianceReport

# Templates supported by the report engine, in the order the dashboard
# renders them in the dropdown.
TEMPLATE_CHOICES: list[tuple[str, str]] = [
    ("hipaa_audit_trail", "HIPAA Audit Trail"),
    ("itar_compliance_summary", "ITAR / EAR Compliance Summary"),
    ("ocap_governance", "OCAP Governance Report"),
    ("system_activity", "System Activity Summary"),
    ("monthly_digest", "Monthly Compliance Digest"),
]
TEMPLATE_KEYS = {key for key, _ in TEMPLATE_CHOICES}

# Output formats the dashboard download buttons offer.
FORMAT_CHOICES: list[tuple[str, str]] = [
    ("json", "JSON"),
    ("html", "HTML"),
    ("csv", "CSV"),
    ("text", "Text"),
]
FORMAT_KEYS = {key for key, _ in FORMAT_CHOICES}


@dataclass
class GenerateForm:
    """Form payload submitted from the Reports page."""

    report_type: str
    from_unix_nanos: int
    to_unix_nanos: int
    format: str = "json"
    tenant: str | None = None
    policy_version: str = "local-dev"

    def validate(self) -> list[str]:
        """Return a list of human-readable validation errors (empty on success)."""
        errors: list[str] = []
        if self.report_type not in TEMPLATE_KEYS:
            errors.append(f"Unknown report template: {self.report_type}")
        if self.format not in FORMAT_KEYS:
            errors.append(f"Unsupported format: {self.format}")
        if self.from_unix_nanos < 0:
            errors.append("from_unix_nanos must be >= 0")
        if self.to_unix_nanos < self.from_unix_nanos:
            errors.append("to_unix_nanos must be >= from_unix_nanos")
        return errors

    def sdk_kwargs(self) -> dict[str, Any]:
        """Project to the SDK ``generate_report`` keyword args."""
        return {
            "report_type": self.report_type,
            "from_unix_nanos": self.from_unix_nanos,
            "to_unix_nanos": self.to_unix_nanos,
            "tenant": self.tenant,
            "format": self.format,
            "policy_version": self.policy_version,
        }


@dataclass
class ReportSummary:
    """Aggregate view rendered above the report table."""

    total: int = 0
    complete: int = 0
    pending: int = 0
    failed: int = 0
    protected: int = 0


def summarise(reports: list[ComplianceReport]) -> ReportSummary:
    """Group reports by status for the dashboard overview chips."""
    summary = ReportSummary(total=len(reports))
    for r in reports:
        if r.status == "complete":
            summary.complete += 1
        elif r.status in {"pending", "generating"}:
            summary.pending += 1
        elif r.status == "failed":
            summary.failed += 1
        if r.protected:
            summary.protected += 1
    return summary


def template_label(report_type: str) -> str:
    """Return the human-readable label for a template key."""
    return dict(TEMPLATE_CHOICES).get(report_type, report_type)


__all__ = [
    "FORMAT_CHOICES",
    "FORMAT_KEYS",
    "TEMPLATE_CHOICES",
    "TEMPLATE_KEYS",
    "GenerateForm",
    "ReportSummary",
    "summarise",
    "template_label",
]
