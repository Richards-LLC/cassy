#!/usr/bin/env python3
"""Run the shipped staleness scan through its CLI against small app-plus-suite fixtures."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[1]
SCRIPT = REPO / 'cas-cli/src/builtins/skills/cas-playwright-tests/scripts/staleness-scan.py'


def run(root, *args):
    result = subprocess.run(['python3', '-I', str(SCRIPT), '--root', str(root), *args], text=True, capture_output=True)
    if result.returncode:
        raise AssertionError(result.stderr)
    return result.stdout


def scan(files, *args, markdown=False):
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        for name, source in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source)
        rows = {row['spec']: row for row in json.loads(run(root, '--json', *args))['specs']}
        return (rows, run(root, *args)) if markdown else rows


class StalenessScan(unittest.TestCase):
    def test_nuxt_and_nestjs_suite_grades_current_stale_dead_and_unsure(self):
        rows = scan({
            'web/pages/index.vue': '<template><main /></template>',
            'web/pages/p/[slug]/i/[itemId].vue': '<template><div data-testid="item-card" class="reply" /></template>',
            'web/pages/[...slug].vue': '<template>404</template>',
            'api/main.ts': "app.setGlobalPrefix('api/v1');",
            'api/items.controller.ts': "@Controller('items')\nclass Items {\n  @Get(':id')\n  one() {}\n}",
            'e2e/current.spec.ts': 'await page.goto("/p/a/i/7");\nawait request.get(`${API}/api/v1/items/${id}`);\n'
                                   'await expect(page.getByTestId("item-card")).toBeVisible();',
            'e2e/stale.spec.ts': 'await page.goto("/");\nawait expect(page.locator(".reply-pending")).toHaveCount(0);\n'
                                 'await request.get("/api/v1/items");',
            'e2e/dead.spec.ts': 'await page.goto("/legacy/board");\nawait page.goto("/api/v0/board");',
            'e2e/unsure.spec.ts': 'await page.goto("/");\nawait page.locator("[data-popover-open]").click();',
        }, '--specs', 'e2e/*.spec.ts', '--routes', 'nuxt:web/pages', '--api', 'nestjs:api',
           '--api-prefix', r'^/api/', '--sources', 'web', markdown=True)
        rows, table = rows
        self.assertEqual({spec: row['grade'] for spec, row in rows.items()}, {
            'e2e/current.spec.ts': 'CURRENT', 'e2e/stale.spec.ts': 'STALE',
            'e2e/dead.spec.ts': 'DEAD', 'e2e/unsure.spec.ts': 'UNSURE'})
        receipts = {d['kind']: d['source_line'] for d in rows['e2e/current.spec.ts']['dependencies']}
        self.assertEqual(receipts, {'route': 'web/pages/p/[slug]/i/[itemId].vue:1',
                                    'api': 'api/items.controller.ts:3', 'testid': 'web/pages/p/[slug]/i/[itemId].vue:1'})
        missing = {(d['kind'], d['value'], d['spec_line']) for d in rows['e2e/stale.spec.ts']['dependencies'] if d['verdict'] == 'missing'}
        self.assertEqual(missing, {('class', 'reply-pending', 'e2e/stale.spec.ts:2'), ('api', '/api/v1/items', 'e2e/stale.spec.ts:3')})
        dead = {(d['kind'], d['value'], d['verdict']) for d in rows['e2e/dead.spec.ts']['dependencies']}
        self.assertEqual(dead, {('route', '/legacy/board', 'missing'), ('api', '/api/v0/board', 'missing')})
        self.assertIn('| api/items.controller.ts:3 |', table)  # the default table carries resolved receipts

    def test_axum_spa_suite_follows_helpers_and_flags_runtime_test_ids(self):
        rows = scan({
            'hub/server.rs': 'Router::new()\n  .route(\n    "/v1/sessions/{session}/status",\n    get(status))\n'
                             '  .route("/{*path}", options(preflight))',
            'app/index.html': '<div id="app"></div>',
            'app/src/syntax.ts': 'span.dataset.testid = `syntax-${kind}`;',
            'app/e2e/double.ts': 'export const STATUS = "/v1/sessions/*/status";\nexport const GONE = "/v1/sessions/legacy";',
            'app/e2e/status.journey.ts': 'import { STATUS } from "./double";\ntest.step("open", async () => {\n'
                                         '  await page.goto("./?fixture=status");\n  await page.getByTestId("syntax-keyword");\n});',
        }, '--specs', 'app/e2e/*.journey.ts', '--routes', 'static:app', '--api', 'axum:hub',
           '--api-prefix', r'^/v\d+/', '--sources', 'app/src')
        row = rows['app/e2e/status.journey.ts']
        self.assertEqual((row['grade'], row['kind']), ('STALE', 'journey'))
        verdicts = {(d['kind'], d['value']): (d['verdict'], d['source_line'] or d['spec_line']) for d in row['dependencies']}
        self.assertEqual(verdicts, {
            ('api', '/v1/sessions/${}/status'): ('ok', 'hub/server.rs:3'),
            ('api', '/v1/sessions/legacy'): ('missing', 'app/e2e/double.ts:2'),
            ('route', '/'): ('ok', 'app/index.html:1'),
            ('testid', 'syntax-keyword'): ('unsure', 'app/src/syntax.ts:1'),
        })

    def test_an_omitted_index_never_grades_current_and_an_empty_one_reports_missing(self):
        files = {'app/index.html': '<div></div>', 'app/empty/.keep': '',
                 'app/e2e/a.spec.ts': 'await page.goto("/");\nawait page.getByTestId("card");'}
        omitted = scan(files, '--specs', 'app/e2e/*.spec.ts', '--routes', 'static:app')['app/e2e/a.spec.ts']
        self.assertEqual((omitted['grade'], omitted['ok'], omitted['unchecked']), ('UNCHECKED', 1, 1))
        empty = scan(files, '--specs', 'app/e2e/*.spec.ts', '--routes', 'static:app', '--sources', 'app/empty')['app/e2e/a.spec.ts']
        self.assertEqual((empty['grade'], empty['missing']), ('STALE', 1))


if __name__ == '__main__':
    unittest.main()
