import json,subprocess
from pathlib import Path

def git(*args):
    return subprocess.check_output(['git',*args],text=True).strip()

import argparse
parser=argparse.ArgumentParser(description='Pin authentic reviewer replay cases; task snapshots come from a read-only CAS export.')
parser.add_argument('--task-snapshots',type=Path,required=True)
parser.add_argument('--reviewer-sha',default='175f78c5')
args=parser.parse_args()
tasks={item['id']:item for item in json.loads(args.task_snapshots.read_text())}
cases=[]
def add(identifier,task,head,fix,axis,scope,defect,repair,criteria,kind='defect'):
    head=git('rev-parse',head)
    base=git('rev-parse',head+'^1')
    fixed=git('rev-parse',fix)
    for path in scope:
        subprocess.run(['git','cat-file','-e',f'{head}:{path}'],check=True)
    source_task={'c08':'cas-d1ee','c09':'cas-d1ee','c10':'cas-d1ee','c12':'cas-5569','c13':'cas-4245','c14':'cas-8e10','c15':'cas-8e10'}.get(identifier,task)
    if '/' in source_task: source_task=source_task.split('/')[0]
    source=tasks[source_task]
    cases.append(dict(task_context={key:source[key] for key in ['id','title','description','acceptance_criteria']},defect_group='self-reexec' if identifier in ['c04','c05','c06'] else identifier,id=identifier,kind=kind,source_task=task,base_sha=base,head_sha=head,
                      expected_fix_sha=fixed,expected_axis=axis,scope=scope,defect=defect,
                      expected_fix=repair,criteria=criteria))

add('c01','cas-4245','e452e2418','cc5c5350d','spec',['scripts/assembly-proof.py','scripts/release-train.d/assemble.sh','scripts/release-gate.sh','scripts/test-assembly-proof.py'],
    'Assembly receipt fingerprints workspace member version values and generated reference history; prep and ledger change those bytes and defeat intended reuse.',
    'Normalize only workspace-member manifest/lock versions and generated ledger, while dependency and code changes still invalidate proof.',
    ['Matching two-context assembly proof is reused after release preparation and ledger generation.','Dependency and code changes invalidate the reused receipt.'])
add('c02','cas-3253','d460825c6','fc942f642','spec',['.github/workflows/ci.yml','scripts/test-ci-test-tiers.sh'],
    'Scoped Validation condition removes the default-branch PR exclusion, expanding the operator-approved admission-only policy.',
    'Restore default-branch PR exclusion, matching dedupe explanation and original policy check.',
    ['Change-scoped selection applies to factory pushes and epic-targeted PRs.','Default-branch PRs retain only required Fast Validation and macOS Check lanes.'])
add('c03','cas-d0fb','4fb8854c','0a93e428c','spec',['cas-cli/Cargo.toml','cas-cli/tests/integration/contracts.rs','cas-cli/tests/hooks_test/main.rs','scripts/cas-test-targets.py'],
    'Inventory sees root tests/*.rs only; directory-main hook suite is omitted from the grouped harness, dropping 34 executed tests.',
    'Include tests/*/main.rs in inventory and wire hooks_test/main.rs with correct nested module imports.',
    ['At most ten integration harnesses preserve every previously executed integration test.','Inventory validation rejects any unwired original source suite.'])
for ident,path in [('c04','cas-cli/tests/factory_mcp_ops_test.rs'),('c05','cas-cli/tests/hub_clean_home_test.rs'),('c06','cas-cli/tests/worktree_surface_test.rs')]:
    add(ident,'cas-d0fb','4fb8854c','0a93e428c','spec',[path,'cas-cli/tests/integration/factory.rs','cas-cli/tests/integration/cli.rs'],
        'After grouping, the isolated self-reexec still uses an unqualified libtest name and may execute zero tests while returning success.',
        'Derive the qualified selector from module_path! and assert printed test execution; require running 1 test except the intentionally early-exiting atexit helper.',
        ['Grouping preserves execution of the isolated child test.','A zero-match child must not pass silently.'])
add('c07','cas-3253','d460825c6','b00a53e85','standards',['.github/workflows/ci.yml'],
    'Full-suite shard inserts a GitHub expression directly into the shell run block for --base-sha.',
    'Pass the expression through step env IMPACT_BASE_SHA and use the shell variable in run.',
    ['Record full-suite recall and timing evidence without expanding untrusted event text into shell source.'])
add('c08','cas-37ba','4c3f0dc39','9ee399dd7','spec',['cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs','crates/cas-store/src/task_store.rs'],
    'Recovery fixture supplies caller updated_at as the optimistic-lock token, although TaskStore::update owns and returns the stored timestamp.',
    'Use update return token; retain a stale-token rejection and unchanged Blocked status assertion.',
    ['Proof-scope recovery works after a blocker report.','A stale optimistic-lock token must still refuse recovery.'])
add('c09','cas-37ba','4c3f0dc39','9ee399dd7','spec',['cas-cli/src/mcp/tools/core/task/lifecycle/proof_scope.rs','cas-cli/src/mcp/tools/core/task/update.rs'],
    'Closed fixture without dispatch expects the proof-scope helper to reject terminal status changes, although terminal policy belongs to the update handler.',
    'Assert the helper boundary accurately and exercise Closed-to-Blocked rejection through the real update handler.',
    ['Proof-scope reports preserve exact scope while mixed updates remain locked.','Terminal tasks cannot change to Blocked through the public update handler.'])
add('c10','cas-37ba','4c3f0dc39','9ee399dd7','spec',['cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs','scripts/check-scoped-test-surface.sh'],
    'Fixture compares canonical sorted/deduplicated scoped targets to traversal order, rather than exact set and uniqueness.',
    'Compare the exact target set and uniqueness, retaining checker execution and cache assertions.',
    ['Scoped proof gate and surface checker agree on the complete required target set.','Required targets are unique and path-order-independent.'])
add('c11','cas-3efd/cas-3051','6db2e506e','9ee399dd7','standards',['cas-cli/src/hooks/handlers/handlers_tests/unscoped_test_guard.rs','cas-cli/src/hooks/handlers/handlers_events/pre_tool.rs'],
    'Allowed-check rewrite fixture redirects logs to bare /tmp and inherits an unrelated clone binding; recursive workspace enforcement correctly denies it.',
    'Bind fixture clone/cwd, write permitted check logs inside the fixture, and keep explicit bare-/tmp denials for both harnesses/targets.',
    ['Both harnesses rewrite the literal permitted package-scoped check through the capped runner.','Rewriting does not bypass workspace file restrictions.'])
add('c12','cas-37ba','65e717f3b','9ee399dd7','spec',['crates/cas-store/src/prompt_queue_store.rs'],
    'Abandoned relay fixture unwraps a transition to Delivered, although both are rank-3 terminal siblings and the transition is rejected.',
    'Assert rejection, unchanged Abandoned stage, absent delivery timestamp and continued forensic reporting.',
    ['An abandoned unknown-target relay remains reportable.','Terminal sibling states cannot be rewritten as successful delivery.'])
add('c13','cas-37ba','e452e2418','9ee399dd7','spec',['cas-cli/src/builtins.rs','cas-cli/src/builtins/skills/cas-supervisor/references/workflow.md'],
    'Builtin workflow fixture still requires the old assembly-command phrase after the canonical workflow switches to the two-context assembly-proof command.',
    'Update the output contract pin to the current single-run assembly wording and exact command without changing shipped workflow text.',
    ['Builtin supervisor workflow and its contract checks agree on the current two-context assembly proof procedure.'])
add('c14','cas-4b3a','3eff5e8b0','03f36326d','standards',['cas-cli/src/builtins/skills/violet/references/registration.md'],
    'Shipped reusable registration instructions contain a concrete operator organization/tracker issue.',
    'Resolve the tracker through cas config get issues.components.violet and preserve fail-closed/no-local-mint behavior.',
    ['Reusable shipped registration instructions resolve project tracker configuration.','Missing hub client endpoint fails closed without minting locally.'])
for ident,task,head,axis,paths,criteria in [
 ('c15','cas-4b3a','03f36326d','standards',['cas-cli/src/builtins/skills/violet/references/registration.md'],['Reusable registration resolves tracker configuration and missing hub endpoint fails closed.']),
 ('c16','cas-3253','b00a53e85','standards',['.github/workflows/ci.yml'],['The impact base SHA reaches the shard through an environment variable rather than shell-source interpolation.']),
 ('c17','cas-4245','cc5c5350d','spec',['scripts/assembly-proof.py','scripts/test-assembly-proof.py'],['Preparation member-version bumps and generated ledger changes reuse the assembly proof.','Dependency, third-party lock and code changes still invalidate proof.']),
 ('c18','cas-d0fb','0a93e428c','spec',['scripts/cas-test-targets.py','cas-cli/tests/integration/contracts.rs','cas-cli/tests/factory_mcp_ops_test.rs','cas-cli/tests/worktree_surface_test.rs','cas-cli/tests/hub_clean_home_test.rs'],['Directory-main integration suites stay wired.','Isolated child-test reexecs assert actual execution after grouping.']),
 ('c19','cas-3efd','9ee399dd7','standards',['cas-cli/src/hooks/handlers/handlers_tests/unscoped_test_guard.rs'],['The allowed-check fixture binds its own clone/cwd and permitted log paths; explicit bare-/tmp denials remain.'])
]:
    add(ident,task,head,head,axis,paths,'No seeded defect; corrected real delivery used as clean negative.','No fix is expected.',criteria,'clean')
for case in cases:
    if case['id']=='c18':
        case['negative_contaminated']=True
        case['contamination_evidence']='Independent baseline found dropped hooks in CI impact inventory; reproduced and filed cas-045f. Preserved and excluded from clean-negative denominator; supplementary c19 is retained.'
    if case['id']=='c19':
        case['addition_provenance']='Supplemental real corrected fixture after independent discovery of c18 contamination.'
manifest=dict(version=2,task_snapshot_provenance='Read-only CAS database export; current stored task text, not a historical criteria snapshot.',reviewer_source_sha=git('rev-parse',args.reviewer_sha),transport_policy='Isolated model replay measures behaviour; API authority/protocol is simulated. A real registered shadow rerun on installed 3.39.0 is required before operator approval of any merge policy.',
              source_notes='Cases c04-c06 are correlated instances of one self-reexec regression; report grouped as well as per-instance metrics. Historical assembly fixture failures are fixture defects, not production-defect claims. Holdout truth must never be given to reviewers.',cases=cases)
Path('docs/review/eval/corpus.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(f'PASS {len(cases)} pinned cases: {sum(c["kind"]=="defect" for c in cases)} defects and {sum(c["kind"]=="clean" for c in cases)} negatives; all scope paths exist at authentic heads')
