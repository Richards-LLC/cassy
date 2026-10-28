#!/usr/bin/env python3
"""Stream local transcripts into content-free Cassy input character aggregates.

This measures newly recorded text, not cumulative billed input tokens. No source
text, commands, arguments, customer names, or source paths are emitted. Only
allowlisted category labels, hashes, numeric measures, and timestamps survive.
Use --help for the reproducible invocation. Python standard library only.
"""
import argparse
import collections
import datetime as dt
import glob
import hashlib
import json
import os
import re
import sqlite3
from pathlib import Path

MAX_LINE = 16 * 1024 * 1024
CATS = {
    'task_detail', 'sibling_notes', 'qa_impact', 'skill_load', 'skill_catalog',
    'project_instructions', 'factory_instructions', 'session_hook',
    'supervisor_reminder', 'ambient_recall', 'incoming_mail', 'director',
    'lifecycle', 'verification_qa', 'repeated_mail', 'merge_ready',
    'merge_close_gate', 'mcp_other', 'other_input',
    'factory_status', 'task_list', 'coordination_receipt', 'inbox_wrapper',
}
CAPS = {'task_detail':2000, 'skill_catalog':2500, 'skill_load':4000,
        'ambient_recall':800, 'incoming_mail':800, 'factory_status':1600,
        'task_list':1600, 'mcp_other':1000, 'coordination_receipt':200,
        'merge_close_gate':800, 'repeated_mail':0, 'lifecycle':240,
        'supervisor_reminder':80, 'verification_qa':900, 'director':250,
        'qa_impact':160, 'sibling_notes':800, 'session_hook':800,
        'project_instructions':3000, 'factory_instructions':3000,
        'inbox_wrapper':80}
SECTION = re.compile(r'\[supervisor reminder\]|\[incoming messages\]|\[ambient recall[^\n]*')
MAIL_SEP = re.compile(r'(?:^|\n)--- from [^\n]*\(message (\d+)\) ---[^\n]*\n|(?:^|\n)\*\*\[(\d+)\] From:[^\n]*\n')
ENVELOPE = re.compile(r'\[cas #(\d+) [^\]\n]*\]')
QA_IMPACT = re.compile(r'Predicted QA impact[^\n]*[\s\S]*?Record one ledger row[^\n]*(?:\n|$)')
TASK_ID = re.compile(r'\bcas-[a-z0-9]{4,8}\b')
MUTATING = {'start', 'close', 'add', 'qa_record', 'qa_waive', 'spawn_workers',
            'worktree_merge', 'request_changes', 'cancel', 'transfer', 'release'}


def stamp(value):
    try:
        return dt.datetime.fromisoformat(str(value).replace('Z', '+00:00')).timestamp()
    except (ValueError, TypeError, OverflowError):
        return None


def text_content(value):
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return '\n'.join(text_content(v) for v in value)
    if isinstance(value, dict):
        # Structured tool output is text only; never stringify image/base64 data.
        return text_content(value.get('text', value.get('content', '')))
    return ''


def bounded_records(path, errors):
    with open(path, 'rb') as handle:
        while True:
            line = handle.readline(MAX_LINE + 1)
            if not line:
                break
            if len(line) > MAX_LINE:
                size = len(line)
                while not line.endswith(b'\n'):
                    line = handle.readline(MAX_LINE + 1)
                    size += len(line)
                    if not line:
                        break
                errors['oversize_records'] += 1
                errors['oversize_bytes'] += size
                continue
            try:
                yield json.loads(line)
            except (ValueError, UnicodeError):
                errors['malformed_records'] += 1


def source_category(text, default):
    # Only call this on actual rendered hooks, mail, or initial instructions;
    # matching a tag inside a source-code read must never turn it into mail.
    if '<cas-verification-dispatch' in text or '<cas-qa-dispatch' in text:
        return 'verification_qa'
    if 'READY TO MERGE' in text[:500]:
        return 'merge_ready'
    if 'director' in text[:220].lower() or 'supervisor_stalled' in text[:500]:
        return 'director'
    if any(x in text[:1500] for x in ['<worker-attention', '<task-lifecycle', 'lifecycle-relay', 'lifecycle-wake:']):
        return 'lifecycle'
    return default


class Session:
    def __init__(self, key, kind, focus_marker):
        self.key, self.kind, self.focus_marker = key, kind, focus_marker
        self.source_id = ''
        self.role = 'unknown'
        self.subagent = False
        self.focus = False
        self.lo = self.hi = None
        self.stats = collections.defaultdict(collections.Counter)
        self.sizes = collections.defaultdict(list)
        self.seen_text, self.seen_mail, self.seen_records = set(), set(), set()
        self.calls, self.pending = {}, []
        self.usage = {}
        self.codex_usage = collections.Counter()
        self.mail_stats = collections.Counter()
        self.hooks = collections.Counter()
        self.tool_outputs = collections.defaultdict(collections.Counter)

    def observe(self, t):
        if t is not None:
            self.lo = min(self.lo, t) if self.lo is not None else t
            self.hi = max(self.hi, t) if self.hi is not None else t

    def identify(self, text):
        if self.focus_marker in text:
            self.focus = True
        head = text[:12000].lower()
        if 'you are the cassy factory supervisor' in head[:250] or head.startswith('[supervisor reminder]'):
            self.role = 'supervisor'
        if self.role == 'unknown':
            if head.startswith('[ambient recall v1 role=supervisor'):
                self.role = 'supervisor'
            elif any(x in head[:500] for x in ['you are a task verifier', 'you are an independent verifier']):
                self.role = 'verifier'
            elif any(x in head for x in ['qa pass (round', 'independent qa and polish pass', 'assigned qa-pass task']):
                # Generic worker boilerplate also mentions assigned QA; only
                # explicit QA task titles establish the QA role below.
                if 'qa pass (round' in head or 'independent qa and polish pass' in head:
                    self.role = 'qa'
            elif any(x in head for x in ['you are a cassy factory worker', 'you are a factory worker', 'you were spawned for task', 'cassy worker startup']):
                self.role = 'worker'

    def add(self, cat, text, mail_id=None):
        if not text:
            return
        assert cat in CATS
        digest = hashlib.sha256(text.encode()).digest()
        duplicate = (cat, digest) in self.seen_text
        self.seen_text.add((cat, digest))
        original_cat = cat
        if mail_id:
            if mail_id in self.seen_mail:
                cat = 'repeated_mail'
                self.mail_stats['same_id_repeated_occurrences'] += 1
                self.mail_stats['same_id_repeated_chars'] += len(text)
            self.seen_mail.add(mail_id)
        row = self.stats[cat]
        row['occurrences'] += 1
        row['chars'] += len(text)
        if cat in CAPS:
            row['counterfactual_cap_chars'] = CAPS[cat]
            row['counterfactual_saving_chars'] += max(0, len(text) - CAPS[cat])
        if cat == 'ambient_recall':
            for surface in ['Memory', 'Guidance', 'Knowledge', 'Task', 'Rule', 'Skill', 'Spec', 'History', 'Code']:
                lines = re.findall(r'^- \[[^\]\n]+\] ' + surface + r' [^\n]*', text, re.M)
                row['cards_' + surface] += len(lines)
                row['card_chars_' + surface] += sum(map(len, lines))
        if duplicate:
            row['exact_repeat_occurrences'] += 1
            row['exact_repeat_chars'] += len(text)
        self.sizes[cat].append(len(text))
        if cat in {'incoming_mail', 'director', 'lifecycle', 'verification_qa', 'repeated_mail', 'merge_ready', 'ambient_recall'}:
            self.pending.append([cat, set(TASK_ID.findall(text)), mail_id, 12])
            row['action_proxy_eligible'] += 1
        if original_cat != cat:
            self.mail_stats['repeat_original_' + original_cat] += 1

    def mail(self, text, default='incoming_mail'):
        matches = list(MAIL_SEP.finditer(text))
        if not matches:
            envelopes = list(ENVELOPE.finditer(text))
            if envelopes:
                matches = envelopes
        if not matches:
            self.add(source_category(text, default), text)
            return
        self.add('inbox_wrapper', text[:matches[0].start()])
        for i, match in enumerate(matches):
            part = text[match.start():matches[i+1].start() if i+1 < len(matches) else len(text)]
            ident = next((g for g in match.groups() if g), None)
            # The first envelope in the message body is the same queue row,
            # not a second delivery; split on separators OR envelopes, not both.
            self.add(source_category(part, default), part, ident)

    def hook(self, text, event):
        self.hooks[event or 'unknown'] += 1
        self.hooks[(event or 'unknown') + '_chars'] += len(text)
        matches = list(SECTION.finditer(text))
        if not matches:
            self.add('session_hook', text)
            return
        self.add('session_hook', text[:matches[0].start()])
        for i, match in enumerate(matches):
            part = text[match.start():matches[i+1].start() if i+1 < len(matches) else len(text)]
            if match.group().startswith('[incoming'):
                self.mail(part)
            else:
                if match.group().startswith('[supervisor reminder]'):
                    self.role = 'supervisor'
                self.add('ambient_recall' if match.group().startswith('[ambient') else 'supervisor_reminder', part)

    def tool(self, name, arguments, ident):
        args = arguments if isinstance(arguments, dict) else {}
        raw = json.dumps(arguments) if isinstance(arguments, dict) else str(arguments)
        action = args.get('action', '')
        if not action and name not in {'Bash', 'Read', 'Write', 'Edit', 'Grep', 'Glob'}:
            a = re.search(r'[\"\']?action[\"\']?\s*[:=]\s*[\"\']([^\"\']+)', raw)
            action = a.group(1) if a else ''
        self.calls[ident] = (name, action, raw)
        if self.role != 'supervisor':
            if action == 'qa_record': self.role = 'qa'
            elif action == 'add' and 'verification' in name: self.role = 'verifier'
        if action in {'spawn_workers', 'worktree_merge'} and ('factory' in name or '__factory' in raw):
            self.role = 'supervisor'
        for pending in self.pending[:]:
            cat, tasks, mail_id, left = pending
            matching = bool(tasks & set(TASK_ID.findall(raw))) and action in MUTATING
            ack = action == 'message_ack' and mail_id and re.search(r'\b' + mail_id + r'\b', raw)
            if matching or ack:
                self.stats[cat]['action_proxy_match'] += 1
                self.pending.remove(pending)
            else:
                pending[3] = left - 1
                if left <= 1:
                    self.pending.remove(pending)

    def result(self, text, ident):
        name, action, raw = self.calls.get(ident, ('', '', ''))
        if self.focus_marker in text:
            self.focus = True
        if re.search(r'(?:Title:|Started task:[^\n]* -)\s*QA pass \(round', text[:2500]):
            self.role = 'qa'
        elif text.startswith('Started task:') and self.role == 'unknown':
            self.role = 'worker'
        cas = bool(re.search(r'mcp__(?:cas|cs)__|cas__(?:task|factory|coordination|verification)', name + raw[:250]))
        if cas:
            family = next((f for f in ['task','coordination','factory','verification','search','memory','artifact','skill','rule','knowledge','system','spec'] if name.endswith('__' + f)), 'wrapper')
            safe_actions = {'show','get','notes','start','list','mine','ready','close','worktree_merge','worker_status','worker_activity','epic_status','add','latest','qa_record','message','message_ack','message_status','inbox','inbox_poll','whoami','search','read','create','update','publish','qa_status','heartbeat'}
            tag = family + ':' + (action if action in safe_actions else 'other')
            self.tool_outputs[tag].update({'occurrences':1,'chars':len(text)})
        if cas and action in {'inbox', 'inbox_poll'}:
            self.mail(text)
            return
        # Only a real read/cat/Skill load of a skill or instructions is a load.
        is_read = name in {'Read', 'Skill'} or bool(re.search(r'\b(cat|sed|head)\b', raw))
        if is_read and ('SKILL.md' in raw or '/skills/' in raw or name == 'Skill'):
            self.add('skill_load', text)
            return
        if is_read and any(x in raw for x in ['CLAUDE.md', 'AGENTS.md']):
            self.add('project_instructions', text)
            return
        is_task = 'task' in name or bool(re.search(r'__task\b', raw))
        is_factory = 'factory' in name or '__factory' in raw
        cat = ('task_detail' if cas and is_task and action in {'show', 'get', 'notes', 'start'}
               else 'merge_close_gate' if cas and action in {'close', 'worktree_merge'}
               else 'task_list' if cas and is_task and action in {'list', 'mine', 'ready', 'available', 'blocked'}
               else 'factory_status' if cas and is_factory and action in {'worker_status', 'worker_activity', 'epic_status', 'worktree_list', 'worktree_status', 'agent_list'}
               else 'coordination_receipt' if cas and action in {'message', 'message_ack', 'message_status', 'heartbeat', 'whoami'}
               else 'mcp_other' if cas else 'other_input')
        # Sibling context and predicted QA appendices are separate disjoint
        # spans, so their shares never double-count their parent task output.
        if cat == 'task_detail':
            spans = []
            sib = re.search(r'[^\n]*SIBLING TASK NOTES[^\n]*', text)
            if sib:
                end = re.search(r'\n[^\n]*Workflow Guidance', text[sib.end():])
                spans.append((sib.start(), sib.end() + end.start() if end else len(text), 'sibling_notes'))
            for m in QA_IMPACT.finditer(text):
                if not any(a <= m.start() < b for a,b,_ in spans):
                    spans.append((m.start(), m.end(), 'qa_impact'))
            cursor = 0
            for a,b,c in sorted(spans):
                self.add(cat, text[cursor:a]); self.add(c, text[a:b]); cursor = b
            self.add(cat, text[cursor:])
        else:
            self.add(cat, text)

    def initial(self, text):
        self.identify(text)
        if '[cas #' in text or '<cas-verification-dispatch' in text or '<worker-attention' in text:
            self.mail(text)
        elif 'AGENTS.md instructions for' in text or '<project_instructions>' in text:
            self.add('project_instructions', text)
        elif '<skills_instructions>' in text:
            a = text.find('<skills_instructions>'); b = text.find('</skills_instructions>', a)
            b = b + len('</skills_instructions>') if b >= 0 else len(text)
            self.add('factory_instructions', text[:a]); self.add('skill_catalog', text[a:b]); self.add('factory_instructions', text[b:])
        elif 'You are a Cassy Factory' in text or 'You are a factory' in text:
            self.add('factory_instructions', text)
        else:
            self.add('other_input', text)


def read_session(path, kind, start, end, focus_marker, errors, identities, logical_sessions):
    # Account switches may copy a transcript onto another inode. Merge main
    # sessions by UUID, subagents by parent UUID + agent stem, and use record
    # UUIDs to remove the shared prefix. Inode dedupe alone is insufficient.
    stem = Path(path).stem
    parent = Path(path).parent.parent.name if '/subagents/' in path else ''
    logical_key = kind + ':' + parent + ':' + stem
    s = logical_sessions.setdefault(logical_key, Session(hashlib.sha256(logical_key.encode()).hexdigest()[:16], kind, focus_marker))
    s.source_id = Path(path).stem
    s.subagent = '/subagents/' in path
    for name, session_id, role in identities:
        if (session_id and session_id in path) or ('worktrees-' + name + '/') in path:
            s.focus = True
            if not s.subagent:
                s.role = role if role in {'supervisor', 'worker'} else 'unknown'
    relevant = kind == 'claude'
    for o in bounded_records(path, errors):
        t = stamp(o.get('timestamp'))
        p = o.get('payload') or {}
        if kind == 'codex' and o.get('type') == 'session_meta':
            relevant = any(x in p.get('cwd', '') for x in ['/Petrastella/cassy', '/Petrastella/cas-src'])
            for name, session_id, role in identities:
                if p.get('cwd', '').rstrip('/').endswith('/worktrees/' + name):
                    s.focus = True
                    s.role = role if role in {'supervisor', 'worker'} else 'unknown'
            if relevant and t is not None and start <= t < end:
                s.initial(text_content(p.get('base_instructions', '')))
        if not relevant or t is None or not start <= t < end:
            continue
        s.observe(t)
        if kind == 'claude':
            typ = o.get('type')
            ident = o.get('uuid')
            if ident and ident in s.seen_records:
                errors['duplicate_record_uuid'] += 1
                continue
            if ident:
                s.seen_records.add(ident)
            if typ == 'attachment':
                a = o.get('attachment') or {}
                rendered = text_content(o.get('rendered'))
                if not rendered:
                    errors['nonrendered_attachment_excluded'] += 1
                    continue
                s.identify(rendered)
                if a.get('type') == 'hook_additional_context':
                    s.hook(rendered, a.get('hookEvent'))
                elif a.get('type') == 'skill_listing':
                    s.add('skill_catalog', rendered)
                elif a.get('type') == 'queued_command':
                    s.initial(rendered)
                else:
                    s.add('other_input', rendered)
            elif typ in {'user', 'assistant'}:
                m = o.get('message') or {}
                if typ == 'assistant':
                    usage = m.get('usage') or {}
                    mid = m.get('id', ident)
                    best = s.usage.setdefault(mid, {})
                    for k,v in usage.items():
                        if isinstance(v, int):
                            best[k] = max(v, best.get(k, 0))
                c = m.get('content', [])
                if isinstance(c, str):
                    if typ == 'user': s.initial(c)
                    continue
                for b in c:
                    if not isinstance(b, dict): continue
                    if b.get('type') == 'tool_use':
                        s.tool(b.get('name', ''), b.get('input', {}), b.get('id'))
                    elif b.get('type') == 'tool_result':
                        s.result(text_content(b.get('content')), b.get('tool_use_id'))
                    elif typ == 'user' and b.get('type') == 'text':
                        s.initial(b.get('text', ''))
            else:
                errors['excluded_record_' + str(typ)] += 1
        else:
            typ = o.get('type')
            if typ == 'response_item':
                pt = p.get('type')
                if pt in {'function_call', 'custom_tool_call'}:
                    args = p.get('arguments', p.get('input', ''))
                    try: args = json.loads(args)
                    except (ValueError, TypeError): pass
                    s.tool(p.get('name', ''), args, p.get('call_id'))
                elif pt in {'function_call_output', 'custom_tool_call_output'}:
                    s.result(text_content(p.get('output')), p.get('call_id'))
                elif pt == 'message' and p.get('role') in {'user', 'developer', 'system'}:
                    s.initial(text_content(p.get('content')))
            elif typ == 'event_msg' and p.get('type') == 'token_count':
                total = (p.get('info') or {}).get('total_token_usage') or {}
                for k,v in total.items():
                    if isinstance(v, int): s.codex_usage[k] = max(v, s.codex_usage[k])
    return s if relevant and s.lo is not None else None


def summarize(sessions):
    agg = collections.defaultdict(collections.Counter)
    sizes = collections.defaultdict(list)
    roles = collections.Counter()
    hours = 0
    for s in sessions:
        hours += max(0, s.hi - s.lo) / 3600
        roles[s.role] += 1
        for cat, row in s.stats.items():
            agg[cat].update({k:v for k,v in row.items() if k != 'counterfactual_cap_chars'})
            if cat in CAPS: agg[cat]['counterfactual_cap_chars'] = CAPS[cat]
            sizes[cat].extend(s.sizes[cat])
    total = sum(r['chars'] for r in agg.values())
    injected = total - agg['other_input']['chars']
    for cat, row in agg.items():
        vals = sorted(sizes[cat]); n = len(vals)
        row.update({'median_chars': vals[n//2] if n else 0,
                    'p95_chars': vals[min(n-1, int(n*.95))] if n else 0,
                    'max_chars': max(vals, default=0),
                    'share_all_input_chars_pct': round(100*row['chars']/max(total, 1), 4),
                    'share_classified_cassy_chars_pct': round(100*row['chars']/max(injected, 1), 4) if cat != 'other_input' else 0,
                    'occurrences_per_session_hour': round(row['occurrences']/max(hours, .001), 4)})
    return {'sessions':len(sessions), 'session_hours':round(hours,4), 'roles':dict(roles),
            'all_input_chars':total, 'classified_cassy_chars':injected,
            'categories':dict(agg)}


def queue_summary(db, start_iso, end_iso, focus):
    if not db:
        return {}
    con = sqlite3.connect('file:' + str(Path(db).resolve()) + '?mode=ro', uri=True)
    con.execute('pragma query_only=ON')
    out = {}
    for label, filt, args in [('all', '', ()), ('focus', ' and factory_session=?', (focus,))]:
        rows = con.execute('select source, highest_stage, count(*), sum(length(prompt)), '
            'max(length(prompt)), sum(delivery_attempts), sum(acked_at is not null), '
            'sum(transport_delivered_at is not null), sum(assumed_seen_at is not null) '
            'from prompt_queue where created_at>=? and created_at<?' + filt + ' group by 1,2',
            (start_iso,end_iso) + args)
        groups = collections.defaultdict(collections.Counter)
        for source, stage, n, chars, mx, attempts, ack, delivered, assumed in rows:
            label_source = source if source in {'director','daemon','supervisor','task-assignment','delivery-watchdog','relay-watchdog','terminal'} else 'other_sources'
            r = groups[label_source]; r.update({'rows':n,'chars':chars or 0,'attempts':attempts or 0,
                'acked':ack or 0,'transport_delivered':delivered or 0,'assumed_seen':assumed or 0})
            r['max_chars'] = max(mx or 0,r['max_chars'])
            r['stage_' + (stage or 'unset')] += n
        out[label] = dict(groups)
        out[label + '_repeated_exact_prompt'] = list(con.execute(
            'select count(*), coalesce(sum(n-1),0), coalesce(sum((n-1)*chars),0) from '
            '(select target,prompt,count(*) n,length(prompt) chars from prompt_queue '
            'where created_at>=? and created_at<?' + filt + ' group by target,prompt having count(*)>1)',
            (start_iso,end_iso) + args).fetchone())
    con.close()
    return out


def log_summary(root, start, end):
    if not root:
        return {}
    counts = collections.Counter()
    markers = ['ambient recall injected', 'worker_idle', 'verification_dispatch_expired',
               'supervisor_stalled', 'READY TO MERGE', 'hook_surfaced',
               'worker attention relay', 'delivery stalled', 'prompt queue']
    for path in sorted(Path(root).glob('cas-????-??-??.log')):
        day = stamp(path.name[4:14] + 'T00:00:00Z')
        if day is None or day + 86400 <= start or day >= end:
            continue
        counts['files'] += 1
        with open(path, 'rb') as handle:
            for line in handle:
                # No raw log line is ever retained or emitted.
                counts['lines'] += 1
                counts['bytes'] += len(line)
                for marker in markers:
                    if marker.encode() in line:
                        counts[marker] += 1
    return dict(counts)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--start', required=True, help='UTC ISO timestamp, inclusive')
    ap.add_argument('--end', required=True, help='UTC ISO timestamp, exclusive; freeze before running')
    ap.add_argument('--output', required=True)
    ap.add_argument('--db', help='read-only Cassy SQLite path; optional queue aggregate')
    ap.add_argument('--logs', help='optional Cassy log directory, marker counts only')
    ap.add_argument('--focus', default='cassy-tender-finch-45')
    ap.add_argument('--supervisor-session', help='optional exact requested supervisor JSONL stem')
    ap.add_argument('--root', action='append', help='override transcript root (repeatable)')
    args = ap.parse_args()
    start,end = stamp(args.start),stamp(args.end)
    if start is None or end is None or end <= start: ap.error('invalid date window')
    roots = args.root or glob.glob(os.path.expanduser('~/.claude*/projects')) + glob.glob(os.path.expanduser('~/.codex*/sessions')) + [os.path.expanduser('~/.cas/claude-base')]
    seen, sessions, logical_sessions = set(), [], {}
    errors, inventory = collections.Counter(), collections.Counter()
    identities = []
    if args.db:
        con = sqlite3.connect('file:' + str(Path(args.db).resolve()) + '?mode=ro', uri=True)
        identities = con.execute('select name,cc_session_id,role from agents where factory_session=?', (args.focus,)).fetchall()
        con.close()
    for root in roots:
        for path in sorted(glob.iglob(root + '/**/*.jsonl', recursive=True)):
            try: st = os.stat(path)
            except OSError: errors['stat_failed'] += 1; continue
            key = st.st_dev,st.st_ino
            if key in seen: inventory['alias_files_skipped'] += 1; continue
            seen.add(key)
            if st.st_mtime < start: inventory['old_files_skipped'] += 1; continue
            kind = 'codex' if '/sessions/' in path else 'claude'
            if kind == 'claude' and not any(x in path for x in ['-Petrastella-cassy', '-Petrastella-cas-src']):
                inventory['other_project_files_skipped'] += 1; continue
            inventory['candidate_files'] += 1; inventory['candidate_bytes'] += st.st_size
            s = read_session(path,kind,start,end,args.focus,errors,identities,logical_sessions)
            if s:
                if not any(prior is s for prior in sessions):
                    sessions.append(s)
                    inventory[kind + '_sessions'] += 1
                else:
                    inventory['copied_session_files_merged'] += 1
            if inventory['candidate_files'] % 100 == 0:
                print('scanned',inventory['candidate_files'],'files',flush=True)
    usage = collections.Counter()
    tool_outputs = collections.defaultdict(collections.Counter)
    for s in sessions:
        for row in s.usage.values(): usage.update(row)
        for tag, row in s.tool_outputs.items(): tool_outputs[tag].update(row)
    result = {'method_version':1, 'window':{'start':args.start,'end':args.end},
        'inventory':dict(inventory), 'excluded':dict(errors),
        'all':summarize(sessions), 'focus':summarize([s for s in sessions if s.focus]),
        'by_harness':{k:summarize([s for s in sessions if s.kind == k]) for k in ['claude','codex']},
        'by_role':{k:summarize([s for s in sessions if s.role == k]) for k in ['supervisor','worker','verifier','qa','unknown']},
        'focus_supervisor':summarize([s for s in sessions if s.focus and s.role == 'supervisor' and not s.subagent]),
        'focus_subagents':summarize([s for s in sessions if s.focus and s.subagent]),
        'requested_supervisor':summarize([s for s in sessions if s.source_id == args.supervisor_session]),
        'mcp_output_breakdown':dict(tool_outputs),
        'claude_usage_unique_message_maxima':dict(usage),
        'codex_usage_maxima_sum':dict(sum((s.codex_usage for s in sessions),collections.Counter())),
        'queue':queue_summary(args.db,args.start,args.end,args.focus),
        'log_markers':log_summary(args.logs,start,end),
        'manifest':[{'file_hash':s.key,'harness':s.kind,'role':s.role,'focus':s.focus,'subagent':s.subagent,
                     'first_epoch':s.lo,'last_epoch':s.hi,'input_chars':sum(r['chars'] for r in s.stats.values()),
                     'mail':dict(s.mail_stats),'rendered_hooks':dict(s.hooks)} for s in sessions]}
    Path(args.output).write_text(json.dumps(result,indent=2) + '\n')
    print(json.dumps({'sessions':len(sessions),'input_chars':result['all']['all_input_chars'],
                     'classified_cassy_chars':result['all']['classified_cassy_chars'],
                     'focus_sessions':result['focus']['sessions']}))


if __name__ == '__main__':
    main()
