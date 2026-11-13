"""python3 .github/scripts/test_release.py"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))
import release as r  # noqa: E402

LOG = """# Changelog

Intro.

## [Unreleased]

### Fixed

- The explorer no longer lags

## [0.3.0] - 2026-11-05

### Added

- Engines
"""


class Bumps(unittest.TestCase):
    def test_versions(self):
        self.assertEqual(r.plan("0.3.0", [("fix(explorer): lag", "#13")]), "0.3.1")
        self.assertEqual(r.plan("0.3.0", [("fix: a", ""), ("feat: b", "")]), "0.4.0")
        self.assertEqual(r.plan("0.3.4", [("feat(ui)!: new settings file", "")]), "0.4.0")
        self.assertEqual(r.plan("1.2.3", [("feat!: break", "")]), "2.0.0")
        self.assertEqual(r.plan("0.3.0", [("chore(deps): bump mongodb from 3.9.1 to 3.10.0 in the compatible group", "#12")]), "0.3.1")
        self.assertEqual(r.plan("0.3.0", [("Update README.md", "abc1234")]), "0.3.1")

    def test_no_release_for_docs_and_ci(self):
        self.assertIsNone(r.plan("0.3.0", [("docs: typo", "#1"), ("ci: cache", "#2"), ("chore: tidy", "#3"), ("test: more", "")]))
        self.assertIsNone(r.plan("0.3.0", []))


class Changelog(unittest.TestCase):
    def test_moves_unreleased_entries(self):
        out = r.roll_changelog(LOG, "0.3.1", "2026-11-12", [("fix(explorer): lag", "#13")])
        self.assertIn("## [Unreleased]\n\n## [0.3.1] - 2026-11-12\n\n### Fixed\n\n- The explorer no longer lags\n\n## [0.3.0] - 2026-11-05", out)
        self.assertTrue(out.startswith("# Changelog\n\nIntro.\n\n"))

    def test_lists_changes_when_unreleased_is_empty(self):
        log = LOG.replace("### Fixed\n\n- The explorer no longer lags\n\n", "")
        out = r.roll_changelog(log, "0.3.1", "2026-11-12", [("chore(deps): bump mongodb from 3.9.1 to 3.10.0", "#12"), ("docs: typo", "#14")])
        self.assertIn("## [0.3.1] - 2026-11-12\n\n### Changed\n\n- Bump mongodb from 3.9.1 to 3.10.0 (#12)\n\n## [0.3.0]", out)
        self.assertNotIn("Typo", out)


if __name__ == "__main__":
    unittest.main()
