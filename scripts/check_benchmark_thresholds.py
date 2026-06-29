#!/usr/bin/env python3
"""Check cc-switch benchmark results against CI blocking thresholds."""

from __future__ import annotations

import argparse
import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Threshold:
    surface: str
    app: str
    operation: str
    median_ms: float
    p95_ms: float


THRESHOLDS = [
    Threshold("CLI", "global", "startup_version", 500.0, 1000.0),
    Threshold("CLI", "claude", "startup_provider_current", 800.0, 1500.0),
    Threshold("CLI", "claude", "provider_list", 1000.0, 1800.0),
    Threshold("CLI", "claude", "usage_query_show", 1000.0, 1800.0),
    Threshold("CLI", "claude", "sessions_list_json", 1200.0, 2200.0),
    Threshold("CLI", "claude", "sessions_show_json", 1000.0, 1800.0),
    Threshold("CLI", "claude", "sessions_messages_json", 1000.0, 1800.0),
    Threshold("CLI", "claude", "provider_switch_a_to_b", 1200.0, 2200.0),
    Threshold("CLI", "claude", "provider_duplicate_add", 1200.0, 2200.0),
    Threshold("CLI", "claude", "provider_delete_copy", 1200.0, 2200.0),
    Threshold("CLI", "codex", "startup_provider_current", 800.0, 1500.0),
    Threshold("CLI", "codex", "provider_list", 1000.0, 1800.0),
    Threshold("CLI", "codex", "usage_query_show", 1000.0, 1800.0),
    Threshold("CLI", "codex", "sessions_list_json", 1200.0, 2200.0),
    Threshold("CLI", "codex", "sessions_show_json", 1000.0, 1800.0),
    Threshold("CLI", "codex", "sessions_messages_json", 1000.0, 1800.0),
    Threshold("CLI", "codex", "provider_switch_a_to_b", 1200.0, 2200.0),
    Threshold("CLI", "codex", "provider_duplicate_add", 1200.0, 2200.0),
    Threshold("CLI", "codex", "provider_delete_copy", 1200.0, 2200.0),
    Threshold("TUI", "claude", "startup_interactive", 1800.0, 3000.0),
    Threshold("TUI", "claude", "open_usage", 900.0, 1600.0),
    Threshold("TUI", "claude", "open_sessions_route", 900.0, 1600.0),
    Threshold("TUI", "claude", "open_sessions_loaded", 1400.0, 2400.0),
    Threshold("TUI", "claude", "open_providers", 900.0, 1600.0),
    Threshold("TUI", "claude", "provider_switch_a_to_b", 1400.0, 2400.0),
    Threshold("TUI", "codex", "startup_interactive", 1800.0, 3000.0),
    Threshold("TUI", "codex", "open_usage", 900.0, 1600.0),
    Threshold("TUI", "codex", "open_sessions_route", 900.0, 1600.0),
    Threshold("TUI", "codex", "open_sessions_loaded", 1400.0, 2400.0),
    Threshold("TUI", "codex", "open_providers", 900.0, 1600.0),
    Threshold("TUI", "codex", "provider_switch_a_to_b", 1400.0, 2400.0),
]
THRESHOLD_KEYS = {(item.surface, item.app, item.operation) for item in THRESHOLDS}


def load_result(path: Path) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception as exc:
        raise SystemExit(f"failed to read benchmark JSON: {exc}") from exc


def summarize_rows(rows: list[dict], failures: list[str]) -> str:
    headers = [
        "surface",
        "app",
        "operation",
        "samples",
        "failures",
        "median_ms",
        "median_limit",
        "p95_ms",
        "p95_limit",
        "status",
    ]
    lines = ["| " + " | ".join(headers) + " |", "| " + " | ".join(["---"] * len(headers)) + " |"]
    for row in rows:
        lines.append("| " + " | ".join(str(row.get(header, "")) for header in headers) + " |")
    if failures:
        lines.extend(["", "Failures:"])
        lines.extend(f"- {failure}" for failure in failures)
    return "\n".join(lines) + "\n"


def append_step_summary(markdown: str) -> None:
    summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if not summary_path:
        return
    with open(summary_path, "a", encoding="utf-8") as handle:
        handle.write("## cc-switch benchmark\n\n")
        handle.write(markdown)


def check_thresholds(result: dict) -> tuple[list[dict], list[str]]:
    failures: list[str] = []
    env = result.get("environment", {})
    if env.get("mode") != "sandbox":
        failures.append(f"benchmark environment must be sandbox, got {env.get('mode')!r}")
    if result.get("operationSet") != "ci-blocking":
        failures.append(f"benchmark operationSet must be ci-blocking, got {result.get('operationSet')!r}")
    expected_samples = result.get("iterations")
    if not isinstance(expected_samples, int) or expected_samples <= 0:
        failures.append(f"benchmark iterations must be a positive integer, got {expected_samples!r}")
        expected_samples = None

    summaries = result.get("summaries")
    if not isinstance(summaries, list):
        failures.append("benchmark JSON has no summaries list")
        summaries = []

    by_key: dict[tuple[object, object, object], dict] = {}
    seen_keys: set[tuple[object, object, object]] = set()
    for row in summaries:
        if not isinstance(row, dict):
            failures.append(f"benchmark summary row must be an object, got {type(row).__name__}")
            continue
        key = (row.get("surface"), row.get("app"), row.get("operation"))
        if key in seen_keys:
            failures.append(f"duplicate benchmark row: {key[0]}/{key[1]}/{key[2]}")
        seen_keys.add(key)
        by_key[key] = row
        if key not in THRESHOLD_KEYS:
            failures.append(f"unexpected benchmark row: {key[0]}/{key[1]}/{key[2]}")

    checked_rows: list[dict] = []
    for threshold in THRESHOLDS:
        key = (threshold.surface, threshold.app, threshold.operation)
        row = by_key.get(key)
        checked = {
            "surface": threshold.surface,
            "app": threshold.app,
            "operation": threshold.operation,
            "samples": "",
            "failures": "",
            "median_ms": "",
            "median_limit": threshold.median_ms,
            "p95_ms": "",
            "p95_limit": threshold.p95_ms,
            "status": "missing",
        }
        if row is None:
            failures.append(f"missing benchmark row: {threshold.surface}/{threshold.app}/{threshold.operation}")
            checked_rows.append(checked)
            continue

        checked["samples"] = row.get("samples")
        checked["failures"] = row.get("failures")
        checked["median_ms"] = row.get("median_ms")
        checked["p95_ms"] = row.get("p95_ms")
        checked["status"] = "ok"

        sample_count = row.get("samples")
        if not isinstance(sample_count, int):
            checked["status"] = "fail"
            failures.append(f"{threshold.surface}/{threshold.app}/{threshold.operation} missing numeric samples")
        elif expected_samples is not None and sample_count != expected_samples:
            checked["status"] = "fail"
            failures.append(
                f"{threshold.surface}/{threshold.app}/{threshold.operation} samples {sample_count} does not match expected {expected_samples}"
            )

        failure_count = row.get("failures")
        if not isinstance(failure_count, int) or failure_count != 0:
            checked["status"] = "fail"
            messages = row.get("failure_messages") or []
            suffix = f": {messages[:2]}" if messages else ""
            failures.append(f"{threshold.surface}/{threshold.app}/{threshold.operation} recorded {failure_count} failures{suffix}")

        for field, limit in [("median_ms", threshold.median_ms), ("p95_ms", threshold.p95_ms)]:
            value = row.get(field)
            if not isinstance(value, (int, float)):
                checked["status"] = "fail"
                failures.append(f"{threshold.surface}/{threshold.app}/{threshold.operation} missing numeric {field}")
                continue
            if value > limit:
                checked["status"] = "fail"
                failures.append(
                    f"{threshold.surface}/{threshold.app}/{threshold.operation} {field} {value:.2f}ms exceeds {limit:.2f}ms"
                )

        checked_rows.append(checked)

    return checked_rows, failures


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Check cc-switch benchmark JSON against CI thresholds.")
    parser.add_argument("json_path", type=Path)
    parser.add_argument("--summary-output", type=Path, help="Write the threshold summary as Markdown.")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    result = load_result(args.json_path)
    rows, failures = check_thresholds(result)
    markdown = summarize_rows(rows, failures)
    print(markdown)
    append_step_summary(markdown)
    if args.summary_output:
        args.summary_output.parent.mkdir(parents=True, exist_ok=True)
        args.summary_output.write_text(markdown, encoding="utf-8")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
