#!/usr/bin/env python3
"""Version identity regression for the supervisor QA candidate builder."""

import runpy
import unittest
from pathlib import Path


verify_candidate_version = runpy.run_path(
    str(Path(__file__).with_name("qa-candidate-build.py"))
)["verify_candidate_version"]


class CandidateVersionTests(unittest.TestCase):
    def test_real_version_output_matches_revision_and_cargo_version(self):
        revision = "e23be66725fd152c4ed16b8365afe9aa0a3d34ba"
        output = "cas 9.99.7 (e23be66 2026-10-21)"
        verify_candidate_version(output, "9.99.7", revision)
        with self.assertRaisesRegex(ValueError, "differs from revision"):
            verify_candidate_version(output, "9.99.8", revision)
        with self.assertRaisesRegex(ValueError, "differs from revision"):
            verify_candidate_version(output, "9.99.7", "f23be66725fd152c4ed16b8365afe9aa0a3d34ba")
        with self.assertRaisesRegex(ValueError, "unexpected format"):
            verify_candidate_version("cas 9.99.7", "9.99.7", revision)


if __name__ == "__main__":
    unittest.main()
