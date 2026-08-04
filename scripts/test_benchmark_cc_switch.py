from __future__ import annotations

import importlib.util
import sys
import tempfile
import types
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("benchmark_cc_switch.py")
SPEC = importlib.util.spec_from_file_location("benchmark_cc_switch", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
benchmark = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = benchmark
SPEC.loader.exec_module(benchmark)


class BenchmarkMarkerTests(unittest.TestCase):
    def test_sessions_loaded_requires_a_published_manifest(self) -> None:
        screen = "Sessions  Title  Time  Messages"
        self.assertFalse(
            benchmark.tui_sessions_loaded_marker(screen, "claude", False)
        )
        self.assertTrue(
            benchmark.tui_sessions_loaded_marker(screen, "claude", True)
        )

    def test_next_page_does_not_accept_a_first_page_relative_time(self) -> None:
        self.assertFalse(
            benchmark.tui_sessions_next_page_marker("Sessions  2 hr ago", 120)
        )
        self.assertTrue(
            benchmark.tui_sessions_next_page_marker("Page 2 · 101-120", 120)
        )

    def test_usage_loaded_rejects_the_loading_frame(self) -> None:
        self.assertFalse(
            benchmark.tui_usage_loaded_marker(
                "Usage Statistics  7 days · Loading...  Usage Trend"
            )
        )
        self.assertTrue(
            benchmark.tui_usage_loaded_marker(
                "Usage Statistics  7 days · 48,064 requests · 1.2M tokens"
            )
        )

    def test_usage_loaded_accepts_a_nonzero_total_ending_in_zero(self) -> None:
        self.assertTrue(
            benchmark.tui_usage_loaded_marker(
                "Usage Statistics  7 days · 1,000 requests · 2.1M tokens"
            )
        )

    def test_manifest_snapshots_are_scoped_to_the_selected_app(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            cc_dir = Path(directory)
            paths = types.SimpleNamespace(cc_dir=cc_dir)
            claude_pointer = (
                cc_dir / "session-pages-v1" / "claude" / "current.json"
            )
            claude_pointer.parent.mkdir(parents=True)
            claude_pointer.write_bytes(b"claude-v1")

            self.assertIsNone(
                benchmark.session_manifest_pointer_snapshot(paths, "codex")
            )
            self.assertEqual(
                benchmark.session_manifest_pointer_snapshot(paths, "claude"),
                b"claude-v1",
            )

            codex_pointer = (
                cc_dir / "session-pages-v1" / "codex" / "current.json"
            )
            codex_pointer.parent.mkdir(parents=True)
            codex_pointer.write_bytes(b"codex-v1")
            before = benchmark.session_manifest_pointer_snapshot(paths, "codex")
            claude_pointer.write_bytes(b"claude-v2")
            self.assertEqual(
                benchmark.session_manifest_pointer_snapshot(paths, "codex"),
                before,
                "a Claude publication must not finish a Codex refresh",
            )
            codex_pointer.write_bytes(b"codex-v2")
            self.assertNotEqual(
                benchmark.session_manifest_pointer_snapshot(paths, "codex"),
                before,
            )


if __name__ == "__main__":
    unittest.main()
