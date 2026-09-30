#!/usr/bin/env python3
"""Export measured evidence and render the report from its Markdown source.

Requires markdown-it-py only for HTML generation. Does not invoke any models,
Rust toolchain, or CAS authority. Never generates accuracy labels.
"""
import argparse
import datetime
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('reviewer_eval', ROOT/'scripts/reviewer-eval.py')
evaluation = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evaluation)


def save(path, value):
    path.write_text(json.dumps(value, indent=2)+'\n')


def export(out, dest):
    corpus = json.loads((dest/'corpus.json').read_text())
    labels = json.loads((dest/'adjudication.json').read_text())
    metrics = evaluation.score(corpus, out, labels)
    records, commits, external, starts, ends = [], [], [], [], []
    for case in corpus['cases']:
        for axis in evaluation.AXES:
            directory = out/case['id']/axis
            result = json.loads((directory/'result.json').read_text())
            cross_file = directory/'cross-check/result.json'
            cross = json.loads(cross_file.read_text()) if cross_file.exists() else None
            record = {k:result[k] for k in ['case_id','axis','validation_error','report','commits']}
            record['telemetry'] = {k:result['telemetry'].get(k) for k in
                ['exit_code','error','elapsed_seconds','usage','models_observed','configured_model',
                 'configured_reasoning_effort','prompt_sha256','authority','commit_execution']}
            if cross:
                # Keep measured evidence while removing machine-specific runner argv.
                cross = dict(cross)
                if cross.get('telemetry'):
                    cross['telemetry'] = dict(cross['telemetry'])
                    cross['telemetry'].pop('argv', None)
            record['cross_check'] = cross
            records.append(record)
            starts.append((directory/'prompt.txt').stat().st_mtime)
            ends.append((directory/'telemetry.json').stat().st_mtime)
            for sha in result['commits']:
                receipts = [json.loads(p.read_text()) for p in (directory/'commit-bridge').glob('*.json')]
                commits.append({'case_id':case['id'],'axis':axis,'commit':sha,
                    'subject':evaluation.git('show','-s','--format=%s',sha,cwd=result['checkout']),
                    'files':evaluation.git('diff-tree','--no-commit-id','--name-only','-r',sha,cwd=result['checkout']).splitlines(),
                    'bridge_receipts':[r for r in receipts if sha in json.dumps(r)],
                    'targeted_test_status':labels.get('commits',{}).get(sha,{}).get('targeted_test_status','unknown')})
    command_count = 0
    for path in out.glob('*/**/events.jsonl'):
        for line in path.read_text().splitlines():
            event = json.loads(line);item = event.get('item',{})
            if event.get('type') == 'item.completed' and item.get('type') == 'command_execution':
                command_count += 1
                if '/home/pippenz/.codex/skills/' in item['command']:
                    external.append({'context':str(path.relative_to(out)),
                                     'command':item['command'],'exit_code':item.get('exit_code')})
    contexts = sorted({r['context'].replace('/events.jsonl','') for r in external})
    for record in records:
        record['external_source_read'] = f'{record["case_id"]}/{record["axis"]}' in contexts
    metrics.update(measurement_window_utc=[datetime.datetime.fromtimestamp(t,datetime.timezone.utc).isoformat() for t in [min(starts),max(ends)]],
        concurrent_batch_envelope_seconds=max(ends)-min(starts),
        batch_envelope_method='Filesystem prompt creation through last independent telemetry write; includes staged execution, approval/analysis gaps and supplemental runs. Per-axis times are summed actor wall time, not comparable concurrent latency.',
        external_source_read_contexts=contexts,
        unresolved_finding_occurrences=sum(r['ungraded_findings'] for r in metrics['rows']),
        adjudication_scope='All finding occurrences have explicit labels; null labels remain independently adjudicated unresolved questions.',
        cross_check_commits_accepted=sum(d['decision']=='accept' for r in records for d in (r['cross_check'] or {}).get('decisions',[])),
        cross_check_commits_reverted=sum(d['decision']=='revert' for r in records for d in (r['cross_check'] or {}).get('decisions',[])),
        cross_check_commits_pending=sum(len(r['commits']) for r in records)-sum(len((r['cross_check'] or {}).get('decisions',[])) for r in records),
        safety_interpretation='All fix tests unknown; zero cross reverts is only a lower bound. Gold-policy reversing patches are separately described, not invented test failures.')
    for row in metrics['rows']:
        row['cross_check_cached_input_tokens'] = sum(u.get('cached_input_tokens',0) for r in records if r['axis']==row['axis'] for u in ((r['cross_check'] or {}).get('telemetry') or {}).get('usage',[]))
    for filename,value in [('results.json',records),('metrics.json',metrics),('bridge-commits.json',commits)]:save(dest/filename,value)
    save(dest/'source-boundary-audit.json',{'commands_inspected':command_count,'external_reads':external,'contexts':contexts,
        'interpretation':'Successful live skill reads violate the requested checkout/source boundary. Quarantine these contexts for source-isolated accuracy claims. This specific audit is not a general sandbox proof; raw commands remain in the task artifacts.'})
    return corpus, labels, metrics, records, commits


def percentage(value):
    return 'N/A' if value is None else f'{value*100:.1f}%'


def report(corpus, labels, metrics, records, commits, dest, tokens):
    basename = '2026-09-30-reviewer-accuracy'
    rows = metrics['rows']
    expected = [(3,11,15,10,12),(0,3,15,0,6),(4,14,27,6,4)]
    actual = [(r['seed_hits'],r['seed_total'],r['true_findings'],r['false_findings'],r['ungraded_findings']) for r in rows]
    if actual != expected or len(records)!=60 or len(commits)!=26:
        raise ValueError('Dated narrative is pinned to this measured snapshot. Changed results require a new independently adjudicated report; generic score remains rerunnable.')
    brief = dest/(basename+'.brief.md')
    if not brief.exists():
        brief.write_text('''# Reviewer accuracy concept brief

Type: comparison/benchmark. Audience: operator, with practitioner evidence below.

Single idea: keep merge decisions independent of reviewer verdicts because usable recall is far below the proposed gate and fix safety is unproven.

Hero form: verdict hero with a shared-scale dot plot. Two measured dots near20–30% sit far from the unapproved90% threshold; the distance is the argument, with denominators and contamination caveats beside it.

Emotional register: restrained, candid, evidence first.

Distinctive move: the empty span between the measured dots and the90% proposal occupies most of the sandstone figure; acceptance by the other reviewer never receives a green safety badge.

Omitted: rankings by token cost, decorative KPI cards, and an enforcement recommendation. Costs are lookup evidence; no candidate earns a safe verdict.
''')
    metric_table='| Actor | Seed recall | Class recall | Precision on resolved findings | Precision bounds | False-flagged clean cases | Invalid reports |\n| --- | --- | --- | --- | --- | --- | --- |\n'
    cost_table='| Actor | Input tokens (includes cached) | Cached input | Output tokens | Summed actor seconds | Cross-check input / cached / output | Cross-check seconds |\n| --- | ---: | ---: | ---: | ---: | --- | ---: |\n'
    for r in rows:
        metric_table+=f'| {r["axis"]} | {r["seed_hits"]}/{r["seed_total"]} ({percentage(r["recall"])}) | {r["grouped_hits"]}/{r["grouped_total"]} ({percentage(r["grouped_recall"])}) | {r["true_findings"]}/{r["true_findings"]+r["false_findings"]} ({percentage(r["precision"])}) | {percentage(r["precision_lower_bound"])}–{percentage(r["precision_upper_bound"])} | {r["clean_negatives_false_flagged"]}/4 | {r["process_errors"]}/20 |\n'
        cost_table+=f'| {r["axis"]} | {r["input_tokens"]} | {r["cached_input_tokens"]} | {r["output_tokens"]} | {r["elapsed_seconds"]:.1f} | {r["cross_check_input_tokens"]} / {r["cross_check_cached_input_tokens"]} / {r["cross_check_output_tokens"]} | {r["cross_check_elapsed_seconds"]:.1f} |\n'
    case_table='| Case | Expected axis | Named defect | Spec hit | Standards hit | Legacy hit |\n| --- | --- | --- | --- | --- | --- |\n'
    for c in corpus['cases']:
        if c['kind']!='defect':continue
        hits=[]
        for axis in evaluation.AXES:
            record=next(r for r in records if r['case_id']==c['id'] and r['axis']==axis)
            matched=any(labels['findings'][f'{c["id"]}/{axis}/{f["id"]}']['seed_match'] for f in evaluation.findings(record['report']))
            hits.append('Content only; invalid' if matched and record['validation_error'] else 'Yes' if matched else 'No')
        case_table+=f'| {c["id"]} | {c["expected_axis"]} | {c["defect"]} | '+ ' | '.join(hits)+' |\n'
    bridge_table='| Case / axis | Actual fix commit through bridge | Other-axis decision | Targeted tests |\n| --- | --- | --- | --- |\n'
    for c in commits:
        decisions=[d for r in records for d in (r['cross_check'] or {}).get('decisions',[]) if d['commit']==c['commit']]
        bridge_table+=f'| {c["case_id"]} / {c["axis"]} | `{c["commit"]}` | {decisions[0]["decision"] if decisions else "Pending; invalid pair"} | Unknown |\n'
    chart='''<figure class="hero-figure">
<svg viewBox="0 0 440 205" role="img" aria-labelledby="recall-title recall-desc">
<title id="recall-title">Usable reviewer recall is21.4%, below the proposed90% gate</title>
<desc id="recall-desc">The combined reviewers detected3 of14 seeds in valid reports. Legacy detected4 of14,28.6%, with source contamination. Proposed threshold90% remains unapproved.</desc>
<path class="axis" d="M155 38H415 M155 168H415"/>
<path class="proposal" d="M389 45V165"/>
<text x="389" y="24" text-anchor="middle">90% proposal</text>
<text x="0" y="76">Reviewers3/14</text><path class="axis" d="M155 71H211"/><circle class="decisive" cx="211" cy="71" r="6"/><text x="225" y="76">21.4%</text>
<text x="0" y="128">Legacy4/14*</text><path class="axis" d="M155 123H229"/><circle class="measured" cx="229" cy="123" r="5"/><text x="242" y="128">28.6%</text>
<text x="155" y="192" text-anchor="middle">0%</text><text x="285" y="192" text-anchor="middle">50%</text><text x="410" y="192" text-anchor="middle">100%</text>
</svg>
<figcaption>Protocol-valid recall,14 seeded instances; extraction2026-09-30 UTC, reviewer-eval.py score and metrics.json. *Legacy includes four contexts that read live skills outside the replay; these cannot establish source-isolated accuracy.</figcaption>
</figure>'''
    text=f'''# Keep reviews in shadow mode.

The combined reviewers' valid reports caught **3 of14 seeded defects (21.4%)**; legacy caught4 (28.6%). Neither clears the proposed90% recall gate. Fix safety remains unproven.

{chart}

| Shared population | Valid-report seed hits | Recall | Proposed minimum |
| --- | ---: | ---: | ---: |
| Combined reviewers | 3/14 | 21.4% | 90.0%, unapproved |
| Legacy verifier | 4/14 | 28.6% | 90.0%, unapproved |

## Accuracy and what the numbers mean

{metric_table}

Spec caught3/11 assigned seeds, Standards0/3. Standards found three Spec seed instances through its test-confidence lens; the c05 report was invalid, so it supplies content evidence but no usable coverage. Raw combined content recall is4/14 (28.6%); valid combined recall is3/14. Both collapse to2/12 independent defect classes (16.7%). The two-axis content result ties legacy recall; valid coverage trails legacy by7.1 percentage points. Standards'100% resolved precision is conditional on only15 resolved findings, with six unresolved judgments and zero assigned seeds caught; it is not a safety endorsement.

Precision counts serialized finding occurrences, including scope-creep entries and repeated IDs in invalid reports. Spec has15 true,10 false and12 unresolved occurrences; Standards15 true,0 false and6 unresolved; legacy27 true,6 false and4 unresolved. All93 distinct keys have explicit independent labels; duplicates yield95 occurrences. Bounds assign unresolved findings all false/all true. They are adjudication bounds, not statistical confidence intervals. Gold seed matches are explicitly recorded in adjudication.json; there is no keyword oracle. Unexpected real findings count toward precision, without inventing seeded recall. Test-confidence judgments are labeled in the raw reports and held unresolved where seam coverage or policy cannot be established.

Spec and legacy each falsely flag3/4 eligible clean cases. Those failures include reversing approved --lib, default-branch PR and metadata-normalization policies because current stored original task criteria omitted later operator clarifications. This measures the combined context-and-review system, not a model's intrinsic accuracy. Clarify canonical requirements before another gate run.

## Corpus and paired harness

Twenty pinned authentic historical slices cover the3.37.0/3.38.0 cycles:14 defects across12 classes,4 eligible corrected-delivery negatives (c15,c16,c17,c20), and2 nominal negatives retained as contaminated (c18,c19). c04–c06 share one self-reexec regression. Corrective commits, exact base/head SHAs and scoped files are in corpus.json/repairs.json. These are curated real fix cases, not a random holdout; six seeds are genuine fixture failures, rather than claims of corresponding production failures.

Two independent discoveries were preserved: c18 drops restored hooks from CI impact inventory (cas-045f), and c19 accepts a log redirection that creates an untracked file before clean_head refuses the check (cas-c0ec). Both are excluded from the clean denominator. Supplemental c20 reuses the authentic corrected factory child-test file at0a93e428c after c19 contamination. Its narrow scope and post-discovery selection add selection bias; it is not a new independent delivery. All three actors emitted zero findings on c20.

Each actor received a fresh shallow replay checkout with the authentic head and parent, no origin remote or future repair object. Spec received exact stored original task text/criteria; Standards received only the real pinned Standards snapshot and proven rules026/172/175; legacy received the pinned task-verifier body and the same task/standards inputs. The CAS task text is a current read-only DB export, not an immutable historical task snapshot. The canonical source pin is175f78c55d32c20a1b67b1b61901a30004175973, including root CODING_STANDARDS.md. Sources were not invented to make seeds detectable. In particular, the Standards sources do not explicitly mandate the shell-interpolation or operator-organization hygiene seeds, making0/3 assigned recall partly a source-coverage limit.

All actors used the same configured gpt-6-sol model with high reasoning, independently logged usage and prompts; per-run emitted model IDs/configuration are in results.json. No Rust build or test was run. Python/static checks inside the model traces do not substitute for Rust execution. Fixers remained workspace-write sandboxed; cross-checks and legacy remained read-only. The runner's approved commit bridge mechanically committed only reviewer-selected files within each replay's exact scope, recording every request/response/SHA. The boundary suite passed before each batch; final suite11/11 verifies ownership, metadata/outside-scope rejection, hidden-future exclusion and error denominators.

The API-authority and report-sealing protocol were **simulated**. Production shadow protocol uses side refs directly. All26 bridged commits below are part of the measured simulation, not registered production reviewer receipts.

## The source boundary was not fully isolated

Live skills were successfully read outside the replay in baseline c01,c04,c06,c20 and Spec c16. source-boundary-audit.json records the exact commands. These five contexts are quarantined for any claim of source-isolated accuracy; workspace-write restricted writes but did not prevent external reads. The observed numbers retain those runs transparently. The baseline's strictly uncontaminated contexts detected only c03/c05; excluding contaminated contexts as coverage misses gives2/14, not a clean paired estimate. No hidden curator corpus/repair/adjudication read was observed in the inspected command traces, but a lexical trace audit is not a general sandbox proof. No observed Rust invocation or nested reviewer delegation was found in the inspected commands; commands reading/searching Rust tool text were distinguished from executing tools.

## Recall by real defect

{case_table}

The captured seeded hits cluster in dropped hooks and grouped self-reexec. No actor caught the named metadata normalization, default-branch policy expansion, optimistic-lock fixture, Closed helper fixture, canonical target-order fixture, bare-/tmp fixture, terminal-sibling fixture, stale builtin phrase, interpolation or organization-hygiene seed. Unseeded findings include broken Nextest failure parsing, stale CI binary selection and Violet setup/runtime mismatch; these do not compensate for missed seeds.

## Commit safety and transport failures

Other-axis cross-checks accepted19/26 actual fix commits, requested **zero reverts**, and left7 pending behind invalid independent pairs. Independently verified targeted tests are unknown for all26. The observed bad-commit rate is therefore **0/26, a lower bound with possible range0–100%**, under the requested definition (other-axis revert or targeted-test failure). Legacy has no fix denominator: N/A. This does not demonstrate zero bad commits.

The curator found gold-policy reversals despite acceptance: c11 Spec removes authorized --lib behavior, c15 Spec reintroduces issue5 into configured tracker guidance, and c17 Spec's two commits remove approved version/ledger normalization. The c17 pair remains invalid and was not cross-checked; c11 and c15 were accepted by Standards. These are separate policy correctness concerns, not fabricated failed-test receipts. c01's removal of documented24h freshness remains a policy dispute. Cross-axis agreement alone did not establish correctness.

Three model protocol failures remain intact: Spec c09 cites nonexact finding sources (and repeats one commit across entries), Spec c17 repeats finding IDs, and Standards c05 declares only the corrective one of its two actual commits. Invalid runs remain in coverage/error denominators; their raw findings remain in precision and all actual commits remain in safety denominators.

Three earlier c08/c09/c10 Spec attempts failed before inference with provider HTTP400 invalid_json_schema because a strict enum contained a quoted criterion. Original directories are archived under pre-inference-schema-failures; no tokens or fixes were generated. Corrected schemas preserved exact criterion text and successful retries are the primary runs. A baseline c15 adapter validation mistake applied Spec-only status constraints to legacy VERIFIED; its original result is retained, corrected locally without changing its raw report or rerunning the model. Scheduling clone-ownership conflicts between two batch runners produced no duplicate model contexts or overwritten reports. Pilot and startup failures are excluded from primary accuracy/cost figures and retained as transport evidence.

{bridge_table}

Exact selected files, bridge responses, commit subjects and full SHAs are in bridge-commits.json. Raw isolated repositories retain each commit for supervised targeted testing. Unknown is deliberate; test receipts must not be inferred from reviewer approval.

## Tokens and time

{cost_table}

Input counts include cached tokens; do not add the cached column a second time. These are emitted cumulative multi-turn usage values, not one prompt's context length or priced cost. There are no missing primary token receipts. Independent times are summed actor wall time, not sequential batch latency. The observed concurrent batch envelope is{metrics['concurrent_batch_envelope_seconds']:.1f}s ({metrics['concurrent_batch_envelope_seconds']/60:.1f}min), from{metrics['measurement_window_utc'][0]} to{metrics['measurement_window_utc'][1]}; it includes staged scheduling, approval/analysis gaps and supplemental runs, so it does not rank actor latency. Cross-check time/tokens are additional measured model work, separately reported.

## Proposed thresholds and the operator's decision

Proposals were recorded before full grading in protocol.json; **operator_approved=false**, **merge_policy_changed=false**. Require assigned-axis, combined and class recall≥90%; adjudicated precision≥95%; zero clean-negative false findings; zero bad commits; zero protocol/process errors; zero unresolved findings; and complete cross-check plus targeted-test receipts for every actual fix. Combined reviewers must meet or exceed legacy recall and precision on identical corrected inputs. This batch fails the proposal; do not relax thresholds to fit it. Small correlated samples cannot establish a future zero bad-commit probability; expand the independently held-out set before enforcement.

Keep both merge-policy adoption and two-way-door auto-merge held. The operator approves thresholds after canonical criteria/Standards coverage is clarified. A **successful rerun through the real registered shadow protocol after3.39.0 is installed is a stated precondition before the operator enables any merge policy**. That rerun must exclude live external skill reads, preserve dispatch/round/identity/verification/cross-check receipts and obtain supervisor targeted-test/assembly proof. The committed registered adapter binds existing real actors; it does not synthesize identities or claim this simulation is registered authority. No merge policy has changed.

## Provenance and rerun

Analysis checkout checkpoint:a86ec77d6fb09fcfb92b8071938a27da1eb9a176; reviewer source:175f78c55d32c20a1b67b1b61901a30004175973. Data window:2026-09-30 UTC above. Raw data:/home/pippenz/.cas/artifacts/cas-b622/measured; pilot/failed transport logs remain alongside it. Durable copies:results.json,metrics.json,adjudication.json,bridge-commits.json,source-boundary-audit.json. No operational secrets are included.

```bash
REVIEW_EVAL_TEST_DIR=/home/pippenz/.cas/artifacts/cas-b622/python-test python3 scripts/test-reviewer-eval.py -v
python3 scripts/reviewer-eval.py run --out /home/pippenz/.cas/artifacts/cas-b622/measured --rules docs/review/eval/promoted-rules.json --reviewer-sha 175f78c5 --jobs 3 --fix-transport bridge
python3 scripts/reviewer-eval.py cross-check --out /home/pippenz/.cas/artifacts/cas-b622/measured --rules docs/review/eval/promoted-rules.json --reviewer-sha 175f78c5 --jobs 3
python3 scripts/reviewer-eval.py score --out /home/pippenz/.cas/artifacts/cas-b622/measured --labels docs/review/eval/adjudication.json
python3 scripts/report-reviewer-eval.py --out /home/pippenz/.cas/artifacts/cas-b622/measured
```

Use a fresh output directory for an intentional rerun; completed runs are reused and source-pin drift is rejected. Re-adjudicate new findings independently, then regenerate this Markdown and its HTML together. Production rerun uses --registered-adapter scripts/reviewer-eval-registered.py with supervisor-provided registered bindings (README.md); real receipts are required. Rendering reads this Markdown as its sole content source and uses inline house tokens; HTML is network-free and readable with JavaScript disabled.
'''
    import re
    text = re.sub(r'\b(caught|has|and|only|all|before|the|with|by|at|is|after|across|of|rules|cases|read|remaining|than|six|These|That|Both|Spec|Standards|legacy|baseline|Python|suite)(?=[0-9])', r'\1 ', text)
    text = text.replace('proposed90%', 'proposed 90%').replace('Reviewers3/14','Reviewers 3/14').replace('Legacy4/14','Legacy 4/14').replace('recall,14','recall, 14').replace('extraction2026','extraction 2026').replace('Spec and legacy each falsely flag3/4','Spec and legacy each falsely flag 3/4')
    return text, basename, brief


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out',type=Path,required=True)
    parser.add_argument('--tokens',type=Path,default=ROOT/'docs/review/eval/report-tokens.css',help='Pinned house tokens.css used inline for HTML')
    args=parser.parse_args();dest=ROOT/'docs/review/eval'
    corpus,labels,metrics,records,commits=export(args.out.resolve(),dest)
    text,basename,brief=report(corpus,labels,metrics,records,commits,dest,args.tokens)
    (dest/(basename+'.md')).write_text(text)
    from markdown_it import MarkdownIt
    md=MarkdownIt('commonmark',{'html':True}).enable('table')
    body=md.render(text)
    body=body.replace('<table>','<div class="table-wrap" tabindex="0" role="region" aria-label="Evidence table; scroll horizontally as needed"><table><caption>Measured evidence; see provenance and linked source data.</caption>').replace('</table>','</table></div>')
    body=body.replace('<th>','<th scope="col">')
    sections=body.split('<h2')
    body='<header>'+sections[0]+'</header>'+''.join('<section><h2'+part+'</section>' for part in sections[1:])
    css=args.tokens.read_text()+'''
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--ink);font:var(--type-body)}main{max-width:1120px;margin:auto;padding:32px 24px}h1{font:var(--type-verdict);max-width:22ch;margin:0 0 24px}h2{font:var(--type-heading);margin:48px 0 20px}p{max-width:75ch;overflow-wrap:anywhere}a{color:var(--action);overflow-wrap:anywhere}a:focus-visible,.table-wrap:focus-visible{outline:3px solid var(--focus);outline-offset:3px}.skip{position:absolute;left:24px;top:-50px}.skip:focus{top:0}header>p:first-of-type{font:var(--type-lede)}.hero-figure{margin:24px 0;padding:24px;background:var(--surface-hero);color:var(--ink);border-bottom:3px solid var(--verdict)}svg{display:block;max-width:560px;width:100%;height:auto}svg text{fill:var(--ink);font:15px var(--font-mono)}svg .axis{fill:none;stroke:var(--line-strong);stroke-width:1.5}svg .proposal{stroke:var(--ink-muted);stroke-dasharray:4 4;stroke-width:1.5}svg .decisive{fill:var(--verdict)}svg .measured{fill:var(--ink-muted)}figcaption{font:var(--type-caption);margin-top:16px;max-width:75ch}.table-wrap{max-width:100%;overflow-x:auto;margin:24px 0}table{border-collapse:collapse;width:100%;font-size:14px;line-height:1.6}caption{text-align:left;color:var(--ink-muted);padding:8px 0}th,td{text-align:left;vertical-align:top;padding:12px;border-bottom:1px solid var(--line);min-width:90px}th{font-weight:600;border-bottom:2px solid var(--line-strong)}td code{overflow-wrap:anywhere;white-space:normal}pre{white-space:pre-wrap;overflow-wrap:anywhere;padding:16px;background:var(--surface);color:var(--ink);border:1px solid var(--line)}code{font-family:var(--font-mono);font-size:.86em}footer{margin:48px 0;border-top:1px solid var(--line);padding-top:20px}strong{font-weight:650}@media(max-width:600px){main{padding:24px 16px}.hero-figure{padding:16px}h1{font-size:36px}h2{font-size:24px}}@media print{:root{--bg:white;--surface:white;--surface-hero:white;--ink:#1B1D24;--ink-muted:#5A5F6E;--line:#DAD3C7;--line-strong:#8F8371;--verdict:#2E3A9F;--action:#2E3A9F}main{padding:0;font-size:10pt;max-width:100%}h1{font-size:28pt}h2{font-size:17pt}.skip{display:none}.table-wrap{overflow:visible}table{font-size:8pt;table-layout:fixed}td,th{min-width:0;overflow-wrap:anywhere;padding:6px}thead{display:table-header-group}tr,figure{break-inside:avoid}a[href^="http"]::after{content:" (" attr(href) ")"}pre{font-size:8pt}svg{max-width:440px}}
'''
    artifact='<!DOCTYPE html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Reviewer accuracy — 2026-09-30</title><style>'+css+'</style></head><body><a class="skip" href="#report">Skip to report</a><main id="report">'+body+'<footer><a href="'+basename+'.md">Markdown source</a> · <a href="metrics.json">Exact metrics</a> · <a href="adjudication.json">Independent grading</a></footer></main></body></html>'
    (dest/(basename+'.html')).write_text(artifact)
    print(json.dumps({'reports':len(records),'commits':len(commits),'combined_valid_recall':metrics['combined_recall'],'content_recall':metrics['content_combined_recall'],'html_bytes':len(artifact.encode())}))


if __name__=='__main__':main()
