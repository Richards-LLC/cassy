#!/usr/bin/env python3
"""Exercise the shipped verifier helper against real committed delivery diffs."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

HELPER = Path(__file__).resolve().parents[1] / "cas-cli/src/builtins/skills/cas-qa-craft/scripts/model-boundary.py"


class ModelBoundary(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Boundary fixture")
        self.git("config", "user.email", "fixture@example.invalid")

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], text=True).strip()

    def write(self, name, text):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def plan(self, name, boundary):
        self.write(name, "# Approved plan\n\n```model-boundary\n" + json.dumps(boundary) + "\n```\n")

    def commit(self):
        self.git("add", ".")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "fixture")
        return self.git("rev-parse", "HEAD")

    def run_check(self, base, plan):
        source = ["--plan-file", str(self.repo / plan)] if plan == "allowed.md" else ["--plan", plan]
        result = subprocess.run(["python3", str(HELPER), "--repo", str(self.repo), "--base", base,
                                 *source], text=True, capture_output=True)
        return result.returncode, json.loads(result.stdout)

    def test_added_sites_respect_approved_boundary_without_changing_unscoped_deliveries(self):
        paths = {"model_owned_paths": ["model/**"]}
        self.plan("owned.md", paths)
        self.plan("elsewhere.md", {"model_owned_paths": ["unrelated/**"]})
        self.write("absent.md", "# A plan without a model boundary\n")
        self.plan("concerns.md", {"model_owned_concerns": ["Interpret replies"]})
        self.plan("allowed.md", {**paths, "model_boundary_exceptions": [{"paths": ["model/**"],
                  "kinds": ["literal_collection", "regex", "exact_string_assertion"],
                  "reason": "This slice validates the protocol fixture, per State schema."}]})
        old = 'const OLD: &[&str] = &["old", "already"];\n'
        self.write("model/intent.rs", old)
        self.write("other/intent.rs", old)
        base = self.commit()
        added = ('const REPLIES: &[&str] = &[\n    "yes",\n    "no",\n];\n'
                 'let intent = Regex::new(r"^(approve|deny)$").unwrap();\n'
                 'assert_eq!(model.reply(), "approved");\n')
        for path in ["model/intent.rs", "other/intent.rs"]:
            self.write(path, old + added)
        # A delivery cannot remove the approved boundary to waive itself.
        self.plan("owned.md", {})
        self.commit()
        for plan, exit_code, count, allowed in [("owned.md", 1, 3, 0), ("elsewhere.md", 0, 0, 0),
                ("absent.md", 0, 0, 0), ("concerns.md", 1, 6, 0), ("allowed.md", 0, 0, 3)]:
            with self.subTest(plan=plan):
                status, report = self.run_check(base, plan)
                self.assertEqual(status, exit_code, report)
                self.assertEqual(len(report["findings"]), count, report)
                self.assertEqual(len(report["allowed"]), allowed, report)
                if plan == "owned.md":
                    self.assertEqual([(s["file"], s["line"], s["kind"]) for s in report["findings"]],
                        [("model/intent.rs", 2, "literal_collection"),
                         ("model/intent.rs", 6, "regex"), ("model/intent.rs", 7, "exact_string_assertion")])

    def test_existing_collection_additions_and_moves_into_boundary_are_visible(self):
        self.plan("plan.md", {"model_owned_paths": ["model/**"]})
        self.write("model/members.py", 'WORDS = [\n    "old",\n]\n')
        self.write("outside.py", 'WORDS = ["yes", "no"]\n')
        base = self.commit()
        self.write("model/members.py", 'WORDS = [\n    "old",\n    "new",\n]\n')
        self.git("mv", "outside.py", "model/moved.py")
        self.commit()
        status, report = self.run_check(base, "plan.md")
        self.assertEqual(status, 1, report)
        self.assertEqual([(s["file"], s["line"]) for s in report["findings"]],
                         [("model/members.py", 3), ("model/moved.py", 1)])

    def test_malformed_declared_boundary_cannot_silently_disable_review(self):
        for boundary in [{"model_owned_paths": "model/**"}, {"model_owned_paths": ["../other"]},
                         {"model_owned_concern": ["typo"]},
                         {"model_owned_paths": ["model/**"], "model_boundary_exceptions": [
                             {"paths": ["model/**"], "kinds": ["regex"], "reason": ""}]}]:
            self.plan("plan.md", boundary)
            base = self.commit()
            status, report = self.run_check(base, "plan.md")
            self.assertEqual(status, 2, report)
            self.assertIn("error", report)


if __name__ == "__main__":
    unittest.main()
