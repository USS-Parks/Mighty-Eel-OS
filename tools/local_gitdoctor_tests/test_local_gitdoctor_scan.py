from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

MODULE_PATH = Path(__file__).resolve().parents[1] / "local_gitdoctor_scan.py"
SPEC = importlib.util.spec_from_file_location("local_gitdoctor_scan", MODULE_PATH)
assert SPEC is not None
scanner = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = scanner
SPEC.loader.exec_module(scanner)


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def test_detects_gitdoctor_style_security_quality_testing_and_project_findings(tmp_path: Path) -> None:
    write(
        tmp_path / "src" / "server.js",
        """
function makeId() {
  return Math.random().toString(36);
}
for (const item of items) {
  JSON.stringify(item);
}
if (a) {
    if (b) {
        if (c) {
            if (d) {
                console.log("nested");
            }
        }
    }
}
console.log("a");
console.log("b");
console.log("c");
console.log("d");
console.log("e");
""",
    )
    write(tmp_path / "package.json", '{"dependencies": {"left-pad": "^1.0.0"}}')
    write(tmp_path / ".gitignore", "target/\n")
    write(tmp_path / "README.md", "# Demo\n")
    write(tmp_path / "tests" / "test_demo.py", "def test_demo():\n    value = 1\n")

    report = scanner.run_scan(tmp_path)
    ids = {finding.check_id for finding in report.findings}

    assert "SEC-009" in ids
    assert "PERF-004" in ids
    assert "QUA-005" in ids
    assert "QUA-009" in ids
    assert "TST-004" in ids
    assert "PRJ-002" in ids
    assert "PRJ-004" in ids
    assert report.total_checks == 58
    assert report.failed >= 6


def test_detects_review_integrity_signals(tmp_path: Path) -> None:
    write(
        tmp_path / "src" / "adapters" / "ollama_adapter.py",
        '''
"""Production-ready robust Ollama adapter.

TODO: replace placeholder transport.
"""

class OllamaAdapter:
    """Complete adapter surface."""

    def generate(self, prompt):
        """Generate text from a local backend."""
        pass

    def stream(self, prompt):
        raise NotImplementedError("stub")
''',
    )
    write(tmp_path / "tests" / "test_adapter.py", "def test_adapter_smoke():\n    assert True\n")

    report = scanner.run_scan(tmp_path)
    ids = {finding.check_id for finding in report.findings}

    assert "REV-001" in ids
    assert "REV-002" in ids
    assert "REV-003" in ids
    assert "REV-006" in ids


def test_clean_minimal_project_passes_project_hygiene_checks(tmp_path: Path) -> None:
    write(tmp_path / "src" / "app.py", "def health():\n    return {'status': 'ok'}\n")
    write(tmp_path / "tests" / "integration" / "test_app.py", "def test_health():\n    assert True\n")
    write(tmp_path / "README.md", "# Demo\n")
    write(tmp_path / ".env.example", "TOKEN=REPLACE-ME\n")
    write(tmp_path / ".gitignore", "node_modules/\n.env\ndist/\nbuild/\n")
    write(tmp_path / "pyproject.toml", "[project]\nname='demo'\n")
    write(tmp_path / "requirements-lock.txt", "pytest==8.0.0\n")
    write(tmp_path / ".github" / "workflows" / "ci.yml", "name: ci\n")
    write(tmp_path / "Dockerfile", "FROM python:3.12-slim@sha256:abc\n")

    report = scanner.run_scan(tmp_path)
    ids = {finding.check_id for finding in report.findings}

    assert "PRJ-002" not in ids
    assert "PRJ-003" not in ids
    assert "PRJ-004" not in ids
    assert "CFG-004" not in ids
    assert "CFG-006" not in ids
    assert "CFG-007" not in ids


def test_cli_json_output(tmp_path: Path) -> None:
    write(tmp_path / "README.md", "# Demo\n")
    write(tmp_path / "src" / "app.py", "def health():\n    return 'ok'\n")

    output = tmp_path / "scan.json"
    exit_code = scanner.main(["--root", str(tmp_path), "--format", "json", "--output", str(output)])
    payload = json.loads(output.read_text(encoding="utf-8"))

    assert exit_code == 0
    assert payload["total_checks"] == 58
    assert "category_scores" in payload
    assert isinstance(payload["findings"], list)
