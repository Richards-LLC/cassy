#!/usr/bin/env python3
"""Signing policy through the release/candidate shell entry point, no Apple secrets."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
HELPER = ROOT / 'scripts/sign-macos.sh'
DEV_ID = 'A' * 40
DEVELOPMENT = 'B' * 40
TEAM = 'PETRATEAM1'


class SigningPolicy(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name)
        self.binary = self.base / 'cas'
        self.binary.write_text('fixture executable')
        self.log = self.base / 'calls'
        self.env = dict(os.environ, PATH=f'{self.base}:{os.environ["PATH"]}',
                        SIGN_TEST_LOG=str(self.log), SIGN_TEST_BINARY=str(self.binary),
                        SIGN_TEST_TEAM=TEAM)
        for name in ('MACOS_SIGNING_CERT_P12_BASE64', 'MACOS_SIGNING_CERT_PASSWORD',
                     'MACOS_SIGNING_TEAM_ID', 'GITHUB_ACTIONS', 'GITHUB_STEP_SUMMARY'):
            self.env.pop(name, None)
        self.tool('security', '''#!/bin/bash
printf 'security %s\\n' "$*" >> "$SIGN_TEST_LOG"
if [[ "$1" == find-identity ]]; then
  [[ -z "${SIGN_TEST_IDENTITIES:-}" ]] || printf '%s\\n' "$SIGN_TEST_IDENTITIES"
fi
''')
        self.tool('codesign', '''#!/bin/bash
printf 'codesign %s\\n' "$*" >> "$SIGN_TEST_LOG"
if [[ "$1" == --force ]]; then
  printf '%s' "$*" > "$SIGN_TEST_BINARY.signature"
elif [[ "$1" == -dv ]]; then
  cat "$SIGN_TEST_BINARY.signature" >/dev/null || exit 1
  printf 'Identifier=dev.petrastella.cas\\nTeamIdentifier=%s\\nAuthority=%s\\n' \\
    "$SIGN_TEST_TEAM" "${SIGN_TEST_AUTHORITY:-Developer ID Application: Petra Stella (PETRATEAM1)}" >&2
fi
''')
        self.tool('spctl', '#!/bin/sh\necho "rejected (not notarized)" >&2\nexit 3\n')

    def tool(self, name, body):
        path = self.base / name
        path.write_text(body)
        path.chmod(0o755)

    def run_helper(self, mode, **env):
        return subprocess.run(['bash', str(HELPER), mode, str(self.binary)],
                              env=dict(self.env, **env), capture_output=True, text=True)

    def test_missing_secrets_keep_prebuild_green_but_block_publication(self):
        candidate = self.run_helper('prebuild')
        self.assertEqual(candidate.returncode, 0, candidate.stderr)
        self.assertIn('unsigned candidate, not publishable', candidate.stdout + candidate.stderr)
        calls = self.log.read_text()
        self.assertIn('--sign -', calls)
        release = self.run_helper('verify-release')
        self.assertNotEqual(release.returncode, 0)
        self.assertIn('MACOS_SIGNING_CERT_P12_BASE64', release.stderr)
        self.assertEqual(self.log.read_text(), calls, 'release must refuse before touching bytes')

    def test_release_identity_is_imported_verified_and_cleaned_up(self):
        secrets = dict(MACOS_SIGNING_CERT_P12_BASE64='cDEy',
                       MACOS_SIGNING_CERT_PASSWORD='fixture-password', MACOS_SIGNING_TEAM_ID=TEAM,
                       SIGN_TEST_IDENTITIES=f'  1) {DEV_ID} "Developer ID Application: Petra Stella ({TEAM})"')
        signed = self.run_helper('prebuild', **secrets)
        self.assertEqual(signed.returncode, 0, signed.stderr)
        calls = self.log.read_text()
        self.assertIn('--options runtime --timestamp --identifier dev.petrastella.cas', calls)
        self.assertIn(f'--sign {DEV_ID}', calls)
        self.assertIn('delete-keychain', calls)
        self.assertIn('--verify --strict', calls)
        checked = self.run_helper('verify-release', **secrets)
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertIn('not a publication gate', checked.stdout + checked.stderr)
        for mismatch in (dict(SIGN_TEST_TEAM='WRONGTEAM1'),
                         dict(SIGN_TEST_AUTHORITY=f'Apple Development: Someone ({TEAM})')):
            with self.subTest(mismatch=mismatch):
                refused = self.run_helper('verify-release', **(secrets | mismatch))
                self.assertNotEqual(refused.returncode, 0, refused.stdout)
        development_only = secrets | dict(SIGN_TEST_IDENTITIES=
            f'  1) {DEVELOPMENT} "Apple Development: Someone ({TEAM})"')
        refused = self.run_helper('prebuild', **development_only)
        self.assertNotEqual(refused.returncode, 0, 'configured CI must never fall back to development/ad-hoc')
        self.assertIn('delete-keychain', self.log.read_text())

    def test_candidate_prefers_developer_id_then_development_then_warns_once(self):
        identities = (f'  1) {DEVELOPMENT} "Apple Development: Someone (PERSONID99)"\n'
                      f'  2) {DEV_ID} "Developer ID Application: Petra Stella ({TEAM})"')
        for present, expected in ((identities, DEV_ID), (identities.splitlines()[0], DEVELOPMENT), ('', '-')):
            with self.subTest(identity=expected):
                self.log.write_text('')
                result = self.run_helper('candidate', SIGN_TEST_IDENTITIES=present)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(f'--sign {expected}', self.log.read_text())
                self.assertEqual((result.stdout + result.stderr).count('::warning::'), int(expected == '-'))


if __name__ == '__main__':
    unittest.main()
