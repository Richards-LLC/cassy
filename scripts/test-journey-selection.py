#!/usr/bin/env python3
"""Fast source-impact regressions; no browser, npm install or server required."""
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
SCRIPT = Path(os.environ.get('JOURNEYS_SELECTOR_SCRIPT', REPO / 'scripts/journeys-for-diff.py'))


class SelectionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'user.email', 'fixture@example.test')
        self.git('config', 'user.name', 'fixture')
        self.write('docs/qa/journeys.md', '# Catalog\n\n## hub-web\n\n- **Surface-wide:** `hub-web/dist/*`, `hub-web/src/main.ts`, `hub-web/src/styles.css`\n\n' + '\n'.join(self.entry(i, name) for i, name in [(1, 'reply'), (2, 'pair'), (3, 'fleet')]))
        for i, name in [(1, 'reply'), (2, 'pair'), (3, 'fleet')]:
            self.write(f'hub-web/e2e/{name}.journey.ts', f'import {{ test }} from "./fixture";\ntest("HUB-J{i} {name}", async () => {{ await journey.stage("Open", async () => {{}}); }});\n')
            self.write(f'hub-web/src/{name}.ts', f'export const {name} = `<div class="{name}-card">ready</div>`;\n')
        self.write('hub-web/e2e/fixture.ts', 'export const test = () => {};\n')
        self.write('hub-web/src/main.ts', 'import { reply } from "./reply";\nimport { fleet } from "./fleet";\nfunction renderReply() { return reply; }\nfunction renderFleet() { return fleet; }\n')
        self.write('hub-web/src/styles.css', 'body { margin: 0; }\n.reply-card { color: blue; }\n.fleet-card { color: green; }\n')
        self.write('hub-web/src/tokens.css', ':root { --ink: black; }\n')
        self.write('hub-web/package.json', json.dumps({'scripts': {'build': 'vite build'}, 'devDependencies': {'vitest': '1'}}))
        self.base = self.commit()

    def tearDown(self):
        self.temp.cleanup()

    def entry(self, i, name):
        return f'''### HUB-J{i} · {name}

- **Entry:** app
- **Goal:** {name}
- **Touches:** `hub-web/src/{name}.ts`
- **Suite:** `hub-web/e2e/{name}.journey.ts`
- **Gaps:** none

**Steps**
1. Open — see it
**Expected experience**
- visible
**Edge paths**
- denied
'''

    def write(self, path, text):
        p = self.root / path
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.root), *args], text=True).strip()

    def commit(self):
        self.git('add', '-A')
        self.git('commit', '-q', '-m', 'fixture')
        return self.git('rev-parse', 'HEAD')

    def run_selector(self, *args, head=None, base=None, check=True):
        env = dict(os.environ, CAS_JOURNEYS_ROOT=str(self.root))
        for key in ['CAS_JOURNEYS_HEAD', 'CAS_JOURNEYS_BASE']:
            env.pop(key, None)
        if head:
            env['CAS_JOURNEYS_HEAD'] = head
        if base:
            env['CAS_JOURNEYS_BASE'] = base
        r = subprocess.run(['python3', str(SCRIPT), *args], env=env, capture_output=True, text=True)
        if check:
            self.assertEqual(r.returncode, 0, r.stderr)
        return r

    def ids(self, *paths, head=None, base=None):
        result = self.run_selector('--paths', *paths, head=head, base=base)
        return {j['id'] for j in json.loads(result.stdout)['journeys']}

    def test_dist_never_selects_even_with_catalog_wide_glob(self):
        self.assertEqual(self.ids('hub-web/dist/app.css', 'hub-web/dist/app.js'), set())

    def test_transitive_source_and_cycle(self):
        self.write('hub-web/src/reply.ts', 'import { value } from "./nested"; export const reply = value;')
        self.write('hub-web/src/nested.ts', 'export { value } from "./leaf.js";')
        self.write('hub-web/src/leaf.ts', 'import "./nested"; export const value = 1;')
        self.assertEqual(self.ids('hub-web/src/leaf.ts'), {'HUB-J1'})

    def test_static_spec_fixture_import_graph(self):
        self.write('hub-web/e2e/fixture.ts', 'import "../src/transport"; export const test = () => {};')
        self.write('hub-web/src/transport.ts', 'export const transport = true;')
        self.assertEqual(self.ids('hub-web/src/transport.ts'), {'HUB-J1', 'HUB-J2', 'HUB-J3'})

    def test_supplemental_spec_titles_join_catalog_ids(self):
        self.write('hub-web/e2e/extra.journey.ts', 'import "../src/replay"; test("HUB-J1 extra", () => {});')
        self.write('hub-web/src/replay.ts', 'export const replay = true;')
        self.assertEqual(self.ids('hub-web/src/replay.ts'), {'HUB-J1'})

    def test_removed_import_still_selected_from_base(self):
        self.write('hub-web/src/reply.ts', 'import "./removed"; export const reply = true;')
        self.write('hub-web/src/removed.ts', 'export const removed = true;')
        base = self.commit()
        (self.root / 'hub-web/src/removed.ts').unlink()
        self.write('hub-web/src/reply.ts', 'export const reply = true;')
        head = self.commit()
        self.assertEqual(self.ids('hub-web/src/removed.ts', base=base, head=head), {'HUB-J1'})

    def test_component_css_and_conditional_rules_are_narrow(self):
        self.write('hub-web/src/styles.css', 'body { margin: 0; }\n.reply-card { color: red; }\n.fleet-card { color: green; }\n')
        self.assertEqual(self.ids('hub-web/src/styles.css', base=self.base), {'HUB-J1'})
        self.write('hub-web/src/styles.css', 'body { margin: 0; }\n.reply-card { color: blue; }\n.fleet-card { color: green; }\n@media(max-width:390px){.fleet-card{display:block;}}')
        self.assertEqual(self.ids('hub-web/src/styles.css', base=self.base), {'HUB-J3'})

    def test_css_base_and_tokens_are_surface_wide(self):
        self.write('hub-web/src/styles.css', 'body { margin: 1px; }\n.reply-card { color: blue; }\n.fleet-card { color: green; }\n')
        self.assertEqual(self.ids('hub-web/src/styles.css', base=self.base), {'HUB-J1', 'HUB-J2', 'HUB-J3'})
        self.assertEqual(self.ids('hub-web/src/tokens.css'), {'HUB-J1', 'HUB-J2', 'HUB-J3'})

    def test_unknown_css_fails_safe_with_reason(self):
        self.write('hub-web/src/styles.css', 'body { margin: 0; }\n.reply-card { color: blue; }\n.fleet-card { color: green; }\n.unknown{color:red}')
        result = json.loads(self.run_selector('--paths', 'hub-web/src/styles.css', base=self.base).stdout)
        self.assertEqual(len(result['journeys']), 3)
        self.assertTrue(all('unmapped-css' in j['reason'] for j in result['journeys']))

    def test_main_handler_and_import_hunks_are_narrow(self):
        self.write('hub-web/src/main.ts', 'import { reply } from "./reply";\nimport { fleet } from "./fleet";\nfunction renderReply() { return reply + "changed"; }\nfunction renderFleet() { return fleet; }\n')
        self.assertEqual(self.ids('hub-web/src/main.ts', base=self.base), {'HUB-J1'})
        self.write('hub-web/src/main.ts', 'import { reply as renderedReply } from "./reply";\nimport { fleet } from "./fleet";\nfunction renderReply() { return reply; }\nfunction renderFleet() { return fleet; }\n')
        self.assertEqual(self.ids('hub-web/src/main.ts', base=self.base), {'HUB-J1'})

    def test_main_state_type_words_do_not_select_unrelated_handlers(self):
        self.write('hub-web/src/main.ts', 'import { reply } from "./reply";\nimport { fleet } from "./fleet";\nconst kept: Map<string, string> = new Map();\nfunction renderReply() { return reply + kept.size; }\nfunction renderFleet(label: string) { return fleet + label; }\n')
        base = self.commit()
        p = self.root / 'hub-web/src/main.ts'
        p.write_text(p.read_text().replace('Map<string, string>', 'Map<string, number>'))
        self.assertEqual(self.ids('hub-web/src/main.ts', base=base), {'HUB-J1'})

    def test_main_bootstrap_or_unattributed_is_wide(self):
        self.write('hub-web/src/main.ts', (self.root / 'hub-web/src/main.ts').read_text() + '\nwindow.addEventListener("load", () => {});\n')
        self.assertEqual(self.ids('hub-web/src/main.ts', base=self.base), {'HUB-J1', 'HUB-J2', 'HUB-J3'})

    def test_dev_dependencies_only_do_not_select(self):
        self.write('hub-web/package.json', json.dumps({'scripts': {'build': 'vite build'}, 'devDependencies': {'vitest': '1', 'fake-indexeddb': '6'}}))
        self.assertEqual(self.ids('hub-web/package.json', base=self.base), set())

    def test_dev_lock_classification_and_unknown_lock(self):
        old = {'lockfileVersion': 3, 'packages': {'': {'devDependencies': {'vitest': '1'}}, 'node_modules/vitest': {'version': '1', 'dev': True}}}
        self.write('hub-web/package-lock.json', json.dumps(old))
        base = self.commit()
        self.write('hub-web/package.json', json.dumps({'scripts': {'build': 'vite build'}, 'devDependencies': {'vitest': '1', 'fake-indexeddb': '6'}}))
        new = json.loads(json.dumps(old));new['packages']['']['devDependencies']['fake-indexeddb'] = '6';new['packages']['node_modules/fake-indexeddb'] = {'version': '6', 'dev': True}
        self.write('hub-web/package-lock.json', json.dumps(new))
        self.assertEqual(self.ids('hub-web/package-lock.json', base=base), set())
        new['packages']['node_modules/runtime'] = {'version': '1'}
        self.write('hub-web/package-lock.json', json.dumps(new))
        self.assertEqual(self.ids('hub-web/package-lock.json', base=base), {'HUB-J1', 'HUB-J2', 'HUB-J3'})

    def test_runtime_build_config_and_unknown_source_are_wide(self):
        self.write('hub-web/package.json', json.dumps({'scripts': {'build': 'vite build'}, 'dependencies': {'runtime': '1'}, 'devDependencies': {'vitest': '1'}}))
        for path in ['hub-web/package.json', 'hub-web/vite.config.ts', 'hub-web/tsconfig.json', 'hub-web/src/unmapped.ts']:
            self.assertEqual(self.ids(path, base=self.base), {'HUB-J1', 'HUB-J2', 'HUB-J3'})

    def test_explicit_head_ignores_working_tree_drift(self):
        self.write('hub-web/src/reply.ts', 'import "./leaf"; export const reply = true;')
        self.write('hub-web/src/leaf.ts', 'export const leaf = true;')
        head = self.commit()
        self.write('hub-web/src/reply.ts', 'export const reply = true;')
        self.assertEqual(self.ids('hub-web/src/leaf.ts', head=head), {'HUB-J1'})
        self.write('docs/qa/journeys.md', 'broken')
        self.assertIn('catalog OK (3 journeys)', self.run_selector('--check', head=head).stdout)
        self.assertNotEqual(self.run_selector('--paths', 'hub-web/src/reply.ts', head='missing-sha', check=False).returncode, 0)

    def test_responsive_goals_real_diff_parses_regex_apostrophe(self):
        path = 'hub-web/e2e/journeys/responsive-goals.ts'
        source = (REPO / path).read_text()
        self.write(path, source)
        spec = self.root / 'hub-web/e2e/reply.journey.ts'
        spec.write_text(spec.read_text() + '\nimport { phonePairLink } from "./journeys/responsive-goals";\nphonePairLink(page, journey, token, earlier);\n')
        base = self.commit()
        self.write(path, source.replace("/Machine's hub address/", "/Machine's address/"))
        head = self.commit()
        result = json.loads(self.run_selector(base, head).stdout)
        self.assertEqual({j['id'] for j in result['journeys']}, {'HUB-J1'})

    def test_regex_quotes_and_braces_do_not_hide_fixture_ownership(self):
        path = 'hub-web/e2e/fixture.ts'
        source = "export function replyOnly() { return /[}']+\\/[{]/.test(value); }\nexport function fleetOnly() { return value / 2 / 3; }\n"
        self.write(path, source)
        spec = self.root / 'hub-web/e2e/reply.journey.ts'
        spec.write_text(spec.read_text() + 'replyOnly();')
        base = self.commit()
        self.write(path, source.replace('.test(value)', '.test(other)'))
        self.assertEqual(self.ids(path, base=base), {'HUB-J1'})

    def test_uncertain_fixture_parser_selects_all_with_reason(self):
        path = 'hub-web/e2e/fixture.ts'
        self.write(path, 'export function replyOnly() { return 1; }\n')
        base = self.commit()
        self.write(path, 'export function replyOnly() { return 2;\n')
        head = self.commit()
        for args, kwargs in [((base, head), {}), (('--paths', path), {'base': base, 'head': head})]:
            with self.subTest(args=args):
                result = json.loads(self.run_selector(*args, **kwargs).stdout)
                self.assertEqual({j['id'] for j in result['journeys']}, {'HUB-J1', 'HUB-J2', 'HUB-J3'})
                self.assertTrue(all('uncertain-source-parser' in j['reason'] for j in result['journeys']))

    def test_unterminated_regex_cannot_yield_empty_or_narrow_selection(self):
        path = 'hub-web/e2e/fixture.ts'
        self.write(path, 'export function replyOnly() { return /ready/.test(value); }\n')
        base = self.commit()
        self.write(path, 'export function replyOnly() { return /unterminated\n}\n')
        head = self.commit()
        for args, kwargs in [((base, head), {}), (('--paths', path), {'base': base, 'head': head}), (('--paths', path), {'head': head})]:
            with self.subTest(args=args, kwargs=kwargs):
                result = json.loads(self.run_selector(*args, **kwargs).stdout)
                self.assertEqual({j['id'] for j in result['journeys']}, {'HUB-J1', 'HUB-J2', 'HUB-J3'})
                self.assertTrue(all('uncertain-source-parser:Unterminated regex literal' in j['reason'] for j in result['journeys']))

    def test_shared_fixture_option_method_narrows_to_users(self):
        self.write('hub-web/e2e/fixture.ts', 'export class Double {\n private operation(route: unknown): void {\n const fleet = this.options.fleet;\n if (!fleet) return;\n fleet.count = 1;\n }\n}\n')
        spec = self.root / 'hub-web/e2e/fleet.journey.ts'
        spec.write_text(spec.read_text() + 'const settings = { fleet: true };')
        base = self.commit()
        p = self.root / 'hub-web/e2e/fixture.ts'
        p.write_text(p.read_text().replace('fleet.count = 1', 'fleet.count = 2'))
        self.assertEqual(self.ids('hub-web/e2e/fixture.ts', base=base), {'HUB-J3'})

    def test_shared_fixture_constructor_and_dispatch_stay_wide(self):
        self.write('hub-web/e2e/fixture.ts', 'export class Double {\n constructor() { this.ready = true; }\n handleSessionFrame() { this.ready = true; }\n}\n')
        base = self.commit()
        p = self.root / 'hub-web/e2e/fixture.ts'
        p.write_text(p.read_text().replace('this.ready = true', 'this.ready = false'))
        r = json.loads(self.run_selector('--paths', 'hub-web/e2e/fixture.ts', base=base).stdout)
        self.assertEqual(len(r['journeys']), 3)
        self.assertTrue(all('fixture-core' in j['reason'] for j in r['journeys']))

    def test_additive_dispatch_recorder_only_selects_observers(self):
        before = 'export class Double {\n handleSessionFrame(message: unknown) {\n this.ready = true;\n }\n}\n'
        self.write('hub-web/e2e/fixture.ts', before)
        spec = self.root / 'hub-web/e2e/reply.journey.ts'
        spec.write_text(spec.read_text() + 'expect(double.receipts).toHaveLength(1);')
        base = self.commit()
        after = before.replace(' handleSessionFrame', ' readonly receipts: unknown[] = [];\n handleSessionFrame').replace(' this.ready = true;', ' this.ready = true;\n if (message.Persisted) this.receipts.push({ id: Number(message.Persisted.id) });')
        self.write('hub-web/e2e/fixture.ts', after)
        self.assertEqual(self.ids('hub-web/e2e/fixture.ts', base=base), {'HUB-J1'})
        for unsafe in [after.replace('true;', 'false;'),
                       after.replace('Number(message.Persisted.id)', 'await observe(message)'),
                       after.replace('Number(message.Persisted.id)', 'observe(message)'),
                       after.replace('if (message.Persisted)', 'if (this.ready)'),
                       after.replace('this.receipts.push({ id: Number(message.Persisted.id) });', 'return;'),
                       after.replace('this.receipts.push', 'this.ready.push')]:
            with self.subTest(unsafe=unsafe):
                self.write('hub-web/e2e/fixture.ts', unsafe)
                self.assertEqual(self.ids('hub-web/e2e/fixture.ts', base=base), {'HUB-J1', 'HUB-J2', 'HUB-J3'})

    def test_shared_fixture_field_and_composition_provider(self):
        self.write('hub-web/e2e/fixture.ts', 'export class Double {\n readonly receipts: unknown[] = [];\n}\n')
        p = self.root / 'hub-web/e2e/reply.journey.ts'
        p.write_text(p.read_text() + 'expect(double.receipts).toHaveLength(1);')
        base = self.commit()
        self.write('hub-web/e2e/fixture.ts', 'export class Double {\n readonly receipts: unknown[] = [1];\n}\n')
        self.assertEqual(self.ids('hub-web/e2e/fixture.ts', base=base), {'HUB-J1'})
        self.write('hub-web/src/provider.ts', 'export const progress = true;')
        self.write('hub-web/src/main.ts', 'import { fleet } from "./fleet";\nimport { progress } from "./provider";\nfunction renderFleet() { return fleet + progress; }')
        self.assertEqual(self.ids('hub-web/src/provider.ts'), {'HUB-J3'})

    def test_literal_dynamic_import(self):
        self.write('hub-web/src/reply.ts', 'export const reply = () => import("./leaf");')
        self.write('hub-web/src/leaf.ts', 'export const leaf = true;')
        self.assertEqual(self.ids('hub-web/src/leaf.ts'), {'HUB-J1'})

    def test_catalog_change_is_not_generic_documentation(self):
        self.assertEqual(self.ids('docs/qa/journeys.md'), {'HUB-J1', 'HUB-J2', 'HUB-J3'})

    def test_property_names_do_not_create_function_call_edges(self):
        self.write('hub-web/src/main.ts', 'import { reply } from "./reply";\nimport { fleet } from "./fleet";\nfunction action() { return reply; }\nfunction signature() { return fleet.confirm.action; }\nfunction renderFleet() { return signature(); }\n')
        base = self.commit()
        p = self.root / 'hub-web/src/main.ts'
        p.write_text(p.read_text().replace('return signature();', 'return signature() + 1;'))
        self.assertEqual(self.ids('hub-web/src/main.ts', base=base), {'HUB-J3'})

    def test_empty_catalog_and_invalid_revision_refuse_selection(self):
        self.write('docs/qa/journeys.md', 'broken')
        self.assertNotEqual(self.run_selector('--all', check=False).returncode, 0)
        self.assertNotEqual(self.run_selector('--paths', 'README.md', check=False).returncode, 0)
        self.assertEqual(self.run_selector('--check', check=False).returncode, 1)

    def test_check_still_detects_catalog_drift(self):
        self.write('hub-web/e2e/reply.journey.ts', 'test("HUB-J1 reply", () => {});')
        result = self.run_selector('--check', check=False)
        self.assertEqual(result.returncode, 1)
        self.assertIn("step 'Open'", result.stderr)


if __name__ == '__main__':
    unittest.main()
