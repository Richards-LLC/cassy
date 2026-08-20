#!/usr/bin/env python3
"""Regression coverage for the cas-c505 report summary arithmetic."""

from decimal import Decimal, ROUND_HALF_UP
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ANALYSIS_DIR = Path(__file__).parents[1]
SOURCE = ANALYSIS_DIR / "2026-08-17-historical-operational-vector-index.md"
CHECKED_HTML = ANALYSIS_DIR / "2026-08-17-historical-operational-vector-index.html"
RENDERER = Path(__file__).with_name("render_historical_vector_report.py")


class HistoricalVectorReportTests(unittest.TestCase):
    def test_embedding_summary_uses_one_reproducible_baseline(self):
        eligible_bytes = Decimal("3032566716")
        current_tokens = Decimal("13530757")
        price_per_million = Decimal("0.13")
        baseline_tokens = eligible_bytes / 4
        baseline_cost = baseline_tokens * price_per_million / Decimal("1000000")
        token_delta = current_tokens - baseline_tokens
        cost_delta = current_tokens * price_per_million / Decimal("1000000") - baseline_cost
        reduction = token_delta / baseline_tokens * 100

        self.assertEqual(baseline_tokens, Decimal("758141679"))
        self.assertEqual(baseline_cost.quantize(Decimal("0.01"), ROUND_HALF_UP), Decimal("98.56"))
        self.assertEqual(token_delta, Decimal("-744610922"))
        self.assertEqual(cost_delta.quantize(Decimal("0.01"), ROUND_HALF_UP), Decimal("-96.80"))
        self.assertEqual(reduction.quantize(Decimal("0.01"), ROUND_HALF_UP), Decimal("-98.22"))

        source = SOURCE.read_text()
        expected_cells = (
            "758,141,679 unfiltered¹",
            "-744,610,922",
            "-98.22%",
            "$98.56 unfiltered¹",
            "-$96.80",
        )
        for cell in expected_cells:
            self.assertIn(cell, source)
        self.assertIn(
            "3,032,566,716 eligible bytes / 4 bytes per token = 758,141,679 tokens",
            source,
        )

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "report.html"
            subprocess.run(
                [sys.executable, str(RENDERER), str(SOURCE), str(output)], check=True
            )
            rendered = output.read_text()

        for cell in expected_cells:
            self.assertIn(cell, rendered)
        self.assertIn("3,032,566,716 eligible bytes / 4 bytes per token", rendered)

        checked_html = CHECKED_HTML.read_text()
        for cell in expected_cells:
            self.assertIn(cell, checked_html)
        self.assertIn("3,032,566,716 eligible bytes / 4 bytes per token", checked_html)


if __name__ == "__main__":
    unittest.main()
