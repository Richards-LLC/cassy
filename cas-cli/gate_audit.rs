//! One source inventory for the generated policy audit and completeness gate.
//! Classifications live beside code; the report is never a manually curated list.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use syn::{
    spanned::Spanned,
    visit::{self, Visit},
};

const DEFINITION: &str = "A factory check is a policy refusal of a request that is otherwise well-formed and executable. A bypass acceptor is any parameter, env var or flag that lets such a refused request proceed. Scope: every registered factory check, every bypass candidate, MCP/hook/script result producers and false-positive paths. Ordinary I/O, parse and input-validation errors are excluded. Workflow C-row fact emission is deferred to step 3 (cas-d777c, decision 10:54 AM ET).";

#[derive(Default)]
pub struct Audit {
    pub rows: Vec<[String; 6]>,
    pub gaps: Vec<String>,
}
impl Audit {
    pub fn markdown(&self, total: usize) -> String {
        let mut text = format!(
            "# Mechanical factory-check audit\n\n{DEFINITION}\n\nMeasured registry total: {total}. Unclassified or uncovered rows: {}.\n\n| Kind / candidate | Site | Code | Tagged on refusal | Tagged on override | Recording boundary / exclusion reason |\n| --- | --- | --- | --- | --- | --- |\n",
            self.gaps.len()
        );
        for row in &self.rows {
            text.push('|');
            for cell in row {
                text.push_str(&format!(
                    " {} |",
                    cell.replace('|', "\\|").replace('\n', " ")
                ));
            }
            text.push('\n');
        }
        if !self.gaps.is_empty() {
            text.push_str("\n## Gaps\n\n");
            for gap in &self.gaps {
                text.push_str(&format!("- {gap}\n"));
            }
        }
        text
    }
}

#[derive(Default, Clone)]
struct Function {
    site: String,
    name: String,
    calls: BTreeSet<String>,
    refused: BTreeSet<String>,
    overridden: BTreeSet<String>,
    flags: BTreeSet<String>,
    conditional: BTreeSet<String>,
    policy: bool,
    annotations: BTreeMap<String, String>,
    producer: Option<&'static str>,
    emissions: Vec<(String, &'static str, usize)>,
}

fn candidate(name: &str) -> bool {
    matches!(
        name,
        "supervisor_override"
            | "force"
            | "override_requested"
            | "allow_trunk"
            | "stranded_branch_override"
            | "confirm_warning"
            | "proof_scope_fix"
            | "adopt_branch"
            | "no_delivery"
            | "negative_result"
            | "evidence_only"
    ) || name.starts_with("supervisor_override_")
        || name.starts_with("force_")
        || name.starts_with("override_")
        || name.ends_with("_override")
        || name.contains("bypass")
        || name.starts_with("skip_")
        || name.starts_with("allow_")
        || name.starts_with("CAS_SKIP_")
}
fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("test")
            || a.path().segments.last().is_some_and(|s| s.ident == "test")
            || a.path().is_ident("cfg")
                && match &a.meta {
                    syn::Meta::List(list) => {
                        list.tokens.to_string().contains("test")
                            && !list.tokens.to_string().starts_with("not")
                    }
                    _ => false,
                }
    })
}
fn bracket_codes(text: &str, prefix: &str) -> BTreeSet<String> {
    text.split('[')
        .skip(1)
        .filter_map(|s| s.split_once(']').map(|(s, _)| s))
        .filter_map(|s| s.strip_prefix(prefix))
        .filter(|s| {
            let Some((family, number)) = s.split_once('-') else {
                return false;
            };
            matches!(family, "G" | "H" | "W" | "N" | "C")
                && number.len() >= 2
                && number.bytes().all(|b| b.is_ascii_digit())
        })
        .map(str::to_string)
        .collect()
}
fn literal(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => Some(s.value()),
        _ => None,
    }
}
struct Body<'a> {
    function: &'a mut Function,
}
impl<'ast> Visit<'ast> for Body<'_> {
    fn visit_expr_call(&mut self, expr: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = expr.func.as_ref() {
            if let Some(segment) = path.path.segments.last() {
                let name = segment.ident.to_string();
                self.function.calls.insert(name.clone());
                if matches!(name.as_str(), "tag" | "refusal_tag" | "override_tag") {
                    if let Some(code) = expr.args.first().and_then(literal) {
                        if name != "override_tag" {
                            self.function.emissions.push((code.clone(), "refusal", expr.span().start().line));
                            self.function.refused.insert(code);
                        } else {
                            self.function.emissions.push((code.clone(), "override", expr.span().start().line));
                            self.function.overridden.insert(code);
                        }
                    }
                }
            }
        }
        visit::visit_expr_call(self, expr);
    }
    fn visit_expr_method_call(&mut self, expr: &'ast syn::ExprMethodCall) {
        self.function.calls.insert(expr.method.to_string());
        // Explicit codes carried to the final close receipt after compaction.
        if expr.method == "push"
            && let syn::Expr::Path(path) = expr.receiver.as_ref()
            && path.path.is_ident("accepted_override_codes")
            && let Some(code) = expr.args.first().and_then(literal)
        {
            self.function.emissions.push((code.clone(), "override", expr.span().start().line));
            self.function.overridden.insert(code);
        }
        visit::visit_expr_method_call(self, expr);
    }
    fn visit_expr_if(&mut self, expr: &'ast syn::ExprIf) {
        let mut flags = Flags::default();
        flags.visit_expr(&expr.cond);
        let flags = flags
            .0
            .into_iter()
            .filter(|name| candidate(name) || self.function.flags.contains(name))
            .collect::<BTreeSet<_>>();
        self.function.conditional.extend(flags.clone());
        self.function.flags.extend(flags);
        visit::visit_expr_if(self, expr);
    }
    fn visit_pat_ident(&mut self, pat: &'ast syn::PatIdent) {
        let name = pat.ident.to_string();
        if candidate(&name) {
            self.function.flags.insert(name);
        }
        visit::visit_pat_ident(self, pat);
    }
    fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
        if let Some(segment) = path.path.segments.last() {
            self.function.calls.insert(segment.ident.to_string());
        }
        visit::visit_expr_path(self, path);
    }
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        fn tokens(body: &mut Body<'_>, stream: proc_macro2::TokenStream, guard: bool) {
            let tokens: Vec<_> = stream.into_iter().collect();
            for (index, token) in tokens.iter().enumerate() {
                match token {
                    proc_macro2::TokenTree::Group(group) => {
                        tokens_in_group(body, group.stream(), guard)
                    }
                    proc_macro2::TokenTree::Literal(literal) => {
                        if let Ok(lit) = syn::parse2::<syn::LitStr>(proc_macro2::TokenStream::from(
                            proc_macro2::TokenTree::Literal(literal.clone()),
                        )) {
                            body.visit_lit_str(&lit);
                        }
                    }
                    proc_macro2::TokenTree::Ident(ident) => {
                        let name = ident.to_string();
                        if guard && candidate(&name) {
                            body.function.flags.insert(name.clone());
                            body.function.conditional.insert(name.clone());
                        }
                        if matches!(name.as_str(), "tag" | "refusal_tag" | "override_tag")
                            && let Some(proc_macro2::TokenTree::Group(args)) = tokens.get(index + 1)
                            && let Some(proc_macro2::TokenTree::Literal(lit)) =
                                args.stream().into_iter().next()
                            && let Ok(code) = syn::parse_str::<syn::LitStr>(&lit.to_string())
                        {
                            if name == "override_tag" {
                                body.function.emissions.push((code.value(), "override", ident.span().start().line));
                                body.function.overridden.insert(code.value());
                            } else {
                                body.function.emissions.push((code.value(), "refusal", ident.span().start().line));
                                body.function.refused.insert(code.value());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        fn tokens_in_group(body: &mut Body<'_>, stream: proc_macro2::TokenStream, guard: bool) {
            tokens(body, stream, guard)
        }
        tokens(self, mac.tokens.clone(), mac.path.is_ident("ensure"));
    }
    fn visit_lit_str(&mut self, lit: &'ast syn::LitStr) {
        let s = lit.value();
        for (prefix, kind) in [("", "refusal"), ("override ", "override")] {
            for code in bracket_codes(&s, prefix) {
                self.function.emissions.push((code, kind, lit.span().start().line));
            }
        }
        self.function.refused.extend(bracket_codes(&s, ""));
        self.function
            .overridden
            .extend(bracket_codes(&s, "override "));
        if s.starts_with("CAS_SKIP_") && !s.contains(' ') {
            self.function.flags.insert(s.clone());
            self.function.conditional.insert(s.clone());
        }
        self.function.policy |= [
            "REFUSED",
            "REJECTED",
            "SAFETY GUARD",
            "HELD BY A PEER",
            "supervisor-only",
            "worker-only",
        ]
        .iter()
        .any(|marker| s.contains(marker));
    }
}
#[derive(Default)]
struct Flags(BTreeSet<String>);
impl<'ast> Visit<'ast> for Flags {
    fn visit_path_segment(&mut self, segment: &'ast syn::PathSegment) {
        let name = segment.ident.to_string();
        self.0.insert(name);
        visit::visit_path_segment(self, segment);
    }
    fn visit_member(&mut self, member: &'ast syn::Member) {
        if let syn::Member::Named(name) = member {
            let name = name.to_string();
            self.0.insert(name);
        }
    }
    fn visit_lit_str(&mut self, lit: &'ast syn::LitStr) {
        let s = lit.value();
        if s.starts_with("CAS_SKIP_") {
            self.0.insert(s);
        }
    }
}
struct Functions<'a> {
    path: &'a str,
    lines: Vec<&'a str>,
    output: &'a mut Vec<Function>,
}
impl Functions<'_> {
    fn annotations(&self, start: usize, end: usize) -> BTreeMap<String, String> {
        let mut annotations = BTreeMap::new();
        let mut annotation_start = start.saturating_sub(1);
        while annotation_start > 0
            && (self.lines[annotation_start - 1]
                .trim_start()
                .starts_with("//")
                || self.lines[annotation_start - 1].trim().is_empty())
        {
            annotation_start -= 1;
        }
        for line in &self.lines[annotation_start..end.min(self.lines.len())] {
            if let Some((_, declaration)) = line.split_once("gate-audit[")
                && let Some((key, reason)) = declaration.split_once("]:")
                && !reason.trim().is_empty()
            {
                annotations
                    .insert(key.trim().into(), reason.trim().into());
            }
        }
        annotations
    }
    fn function(
        &mut self,
        name: String,
        signature: &syn::Signature,
        block: &syn::Block,
        span: proc_macro2::Span,
    ) {
        let start = span.start().line;
        let end = block.span().end().line;
        let mut f = Function {
            name,
            site: format!("{}:{start}", self.path),
            ..Default::default()
        };
        f.annotations = self.annotations(start, end);
        let output = &signature.output;
        // The actual type paths, rather than spelling in comments, identify producers.
        struct Types(BTreeSet<String>);
        impl<'ast> Visit<'ast> for Types {
            fn visit_path_segment(&mut self, s: &'ast syn::PathSegment) {
                self.0.insert(s.ident.to_string());
                visit::visit_path_segment(self, s);
            }
        }
        let mut types = Types(BTreeSet::new());
        types.visit_return_type(output);
        f.producer = if types.0.contains("CallToolResult") {
            Some("MCP: server_handler::call_tool → facts::observe_tool_result")
        } else if types.0.contains("HookOutput") {
            Some("hook: cli::hook::process_hook_input → facts::observe_hook_output")
        } else {
            None
        };
        for argument in &signature.inputs {
            if let syn::FnArg::Typed(argument) = argument
                && let syn::Type::Path(ty) = argument.ty.as_ref()
                && ty.path.is_ident("bool")
                && let syn::Pat::Ident(name) = argument.pat.as_ref()
            {
                f.flags.insert(name.ident.to_string());
            }
        }
        let mut body = Body { function: &mut f };
        body.visit_signature(signature);
        body.visit_block(block);
        self.output.push(f);
    }
}
impl<'ast> Visit<'ast> for Functions<'_> {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !test_only(&item.attrs) {
            visit::visit_item_mod(self, item);
        }
    }
    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        if test_only(&item.attrs) {
            return;
        }
        let mut f = Function {
            name: item.ident.to_string(),
            site: format!("{}:{}", self.path, item.span().start().line),
            ..Default::default()
        };
        f.annotations = self.annotations(item.span().start().line, item.span().end().line);
        Body { function: &mut f }.visit_expr(&item.expr);
        self.output.push(f);
    }
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !test_only(&item.attrs) {
            self.function(
                item.sig.ident.to_string(),
                &item.sig,
                &item.block,
                item.span(),
            );
        }
    }
    fn visit_impl_item_const(&mut self, item: &'ast syn::ImplItemConst) {
        if test_only(&item.attrs) {
            return;
        }
        let mut f = Function {
            name: item.ident.to_string(),
            site: format!("{}:{}", self.path, item.span().start().line),
            ..Default::default()
        };
        f.annotations = self.annotations(item.span().start().line, item.span().end().line);
        Body { function: &mut f }.visit_expr(&item.expr);
        self.output.push(f);
    }
    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !test_only(&item.attrs) {
            self.function(
                item.sig.ident.to_string(),
                &item.sig,
                &item.block,
                item.span(),
            );
        }
    }
}
fn files(directory: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let name = p.file_name().unwrap().to_string_lossy();
        if p.is_dir() {
            if !matches!(
                name.as_ref(),
                "target" | "node_modules" | "tests" | "fixtures" | ".git"
            ) {
                files(&p, output);
            }
        } else if !name.ends_with("_tests.rs") && name != "tests.rs"
            && p.extension()
                .is_some_and(|x| matches!(x.to_str(), Some("rs" | "sh" | "py" | "registry")))
        {
            output.push(p);
        }
    }
}

/// Scan declarations, AST control-flow candidates, result types and script flags.
/// Registry tuples are (code, stage, title, source), from the same build scan.
pub fn scan(root: &Path, registry: &[(String, String, String, String)]) -> Audit {
    let mut paths = Vec::new();
    for dir in ["cas-cli/src", "crates", "scripts"] {
        files(&root.join(dir), &mut paths);
    }
    paths.sort();
    let mut functions = Vec::new();
    let mut script_rows = Vec::new();
    let mut all_refused = BTreeSet::new();
    let mut all_overridden = BTreeSet::new();
    let mut audit = Audit::default();
    for path in paths {
        let relative = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .to_string();
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        if path.extension().is_some_and(|e| e == "rs") {
            match syn::parse_file(&source) {
                Ok(file) => Functions {
                    path: &relative,
                    lines: source.lines().collect(),
                    output: &mut functions,
                }
                .visit_file(&file),
                Err(e) => audit.gaps.push(format!("{relative}: AST scan failed: {e}")),
            }
        } else {
            let codes = bracket_codes(&source, "");
            all_refused.extend(codes.clone());
            for (index, line) in source
                .lines()
                .enumerate()
                .filter(|(_, line)| !line.trim_start().starts_with('#'))
            {
                let mut rest = line;
                while let Some(start) = rest.find("CAS_SKIP_") {
                    rest = &rest[start..];
                    let end = rest
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .unwrap_or(rest.len());
                    let flag = &rest[..end];
                    let recorded = source.contains("gates record-override");
                    let exclusion = source
                        .lines()
                        .find_map(|line| {
                            line.split_once(&format!("gate-audit[{flag}]:"))
                                .map(|(_, reason)| reason.trim())
                        })
                        .filter(|reason| !reason.is_empty());
                    script_rows.push([
                        format!("script bypass candidate {flag}"),
                        format!("{relative}:{}", index + 1),
                        codes.iter().cloned().collect::<Vec<_>>().join(", "),
                        "yes".into(),
                        if recorded { "yes" } else { "NO" }.into(),
                        if recorded {
                            "script: gates record-override"
                        } else {
                            exclusion.unwrap_or("UNCLASSIFIED")
                        }
                        .into(),
                    ]);
                    if !recorded && exclusion.is_none() {
                        audit
                            .gaps
                            .push(format!("{relative}:{} unclassified {flag}", index + 1));
                    }
                    rest = &rest[end..];
                }
            }
            if !codes.is_empty() {
                script_rows.push([
                    "script result producer".into(),
                    relative.clone(),
                    codes.iter().cloned().collect::<Vec<_>>().join(", "),
                    "yes".into(),
                    if source.contains("gates record-override") {
                        "yes"
                    } else {
                        "—"
                    }
                    .into(),
                    if source.contains("gates record-refusal") {
                        "script: gates record-refusal"
                    } else {
                        "C-row facts deferred to cas-d777c"
                    }
                    .into(),
                ]);
            }
        }
    }
    for f in &functions {
        for (code, kind, line) in &f.emissions {
            let path = f.site.rsplit_once(':').unwrap().0;
            audit.rows.push([
                format!("direct {kind} emission ({})", f.name),
                format!("{path}:{line}"), code.clone(),
                if *kind == "refusal" { "[CODE]" } else { "—" }.into(),
                if *kind == "override" { "[override CODE]" } else { "—" }.into(),
                "producer tag; consumed at request boundary".into(),
            ]);
        }
    }
    loop {
        let mut known = BTreeMap::<String, (BTreeSet<String>, BTreeSet<String>)>::new();
        let mut occurrences = BTreeMap::<String, usize>::new();
        for f in &functions {
            *occurrences.entry(f.name.clone()).or_default() += 1;
        }
        for f in &functions {
            if occurrences[&f.name] != 1 {
                continue;
            }
            let entry = known.entry(f.name.clone()).or_default();
            entry.0.extend(f.refused.clone());
            entry.1.extend(f.overridden.clone());
        }
        let mut changed = false;
        for f in &mut functions {
            let prior = (f.refused.len(), f.overridden.len());
            for call in &f.calls {
                if let Some((refused, overridden)) = known.get(call) {
                    f.refused.extend(refused.clone());
                    f.overridden.extend(overridden.clone());
                }
            }
            changed |= prior != (f.refused.len(), f.overridden.len());
        }
        if !changed {
            break;
        }
    }
    for f in &functions {
        all_refused.extend(f.refused.clone());
        all_overridden.extend(f.overridden.clone());
    }
    let registered: BTreeSet<_> = registry.iter().map(|r| r.0.clone()).collect();
    for f in &functions {
        for code in f.refused.union(&f.overridden) {
            if !registered.contains(code) {
                audit
                    .gaps
                    .push(format!("{} uses unregistered code {code}", f.site));
            }
        }
        if let Some(boundary) = f.producer {
            audit.rows.push([
                format!("result producer {}", f.name),
                f.site.clone(),
                f.refused.iter().cloned().collect::<Vec<_>>().join(", "),
                "via common observer".into(),
                "explicit tags only".into(),
                boundary.into(),
            ]);
        }
        if f.name.contains("false_positive") || f.calls.contains("record_false_positive") {
            audit.rows.push([
                format!("false-positive path {}", f.name),
                f.site.clone(),
                "runtime registered code".into(),
                "—".into(),
                "—".into(),
                "facts::record_false_positive; rejects unregistered codes".into(),
            ]);
        }
        if f.policy && f.refused.is_empty() {
            audit.rows.push([
                format!("policy refusal candidate {}", f.name),
                f.site.clone(),
                "—".into(),
                "—".into(),
                "—".into(),
                f.annotations
                    .get("input")
                    .or_else(|| f.annotations.get("forward"))
                    .cloned()
                    .unwrap_or_else(|| "UNCLASSIFIED".into()),
            ]);
        }
        if f.policy
            && f.refused.is_empty()
            && !f.annotations.contains_key("input")
            && !f.annotations.contains_key("forward")
        {
            audit.gaps.push(format!(
                "{} {} has an unclassified policy refusal",
                f.site, f.name
            ));
        }
        for flag in &f.flags {
            let reason = f
                .annotations
                .get(flag)
                .or_else(|| f.annotations.get("selector"))
                .or_else(|| f.annotations.get("input"));
            let conditional = f.conditional.contains(flag);
            let covered =
                !f.overridden.is_empty() || f.refused.iter().any(|c| all_overridden.contains(c));
            let (kind, boundary) = if let Some(reason) = reason {
                if let Some(codes) = reason
                    .strip_prefix("bypass ")
                    .and_then(|r| r.split_whitespace().next())
                {
                    for code in codes.split(',') {
                        if !registered.contains(code)
                            || !all_refused.contains(code)
                            || !all_overridden.contains(code)
                        {
                            audit.gaps.push(format!("{} {flag}: bypass {code} needs declaration, refusal and override tags",f.site));
                        }
                    }
                    (
                        "bypass acceptor",
                        format!("{reason}; common result observer"),
                    )
                } else {
                    ("excluded selector", reason.clone())
                }
            } else if !conditional {
                (
                    "forwarded candidate",
                    "delegates acceptance; no local conditional bypass".into(),
                )
            } else if !candidate(flag) && !f.policy && f.refused.is_empty() {
                ("excluded execution selector", "Boolean control has no policy refusal or registered gate in this function or statically resolved callees".into())
            } else {
                audit.gaps.push(format!(
                    "{} {} unclassified bypass candidate {flag}",
                    f.site, f.name
                ));
                (
                    "UNCLASSIFIED",
                    "missing local classification / override tag".into(),
                )
            };
            audit.rows.push([
                format!("{kind}: {flag} ({})", f.name),
                f.site.clone(),
                f.overridden
                    .union(&f.refused)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
                if f.refused.is_empty() { "—" } else { "yes" }.into(),
                if covered { "yes" } else { "—" }.into(),
                boundary,
            ]);
        }
    }
    for (code, stage, title, site) in registry {
        let nonrefusing = matches!(&code[..1], "W" | "N") || code == "G-57";
        let deferred = &code[..1] == "C";
        let tagged = all_refused.contains(code);
        if !tagged && !nonrefusing && !deferred {
            audit.gaps.push(format!(
                "{site} {code}: registered refusal has no emitted code"
            ));
        }
        audit.rows.push([
            format!("registered {stage}: {title}"),
            site.clone(),
            code.clone(),
            if tagged {
                "yes"
            } else if nonrefusing {
                "notice / exemption"
            } else if deferred {
                "deferred C-row"
            } else {
                "NO"
            }
            .into(),
            if all_overridden.contains(code) {
                "yes"
            } else {
                "—"
            }
            .into(),
            if deferred {
                "C-row facts deferred to cas-d777c"
            } else if nonrefusing {
                "notice / exemption"
            } else {
                "MCP / hook / script observer"
            }
            .into(),
        ]);
    }
    audit.rows.extend(script_rows);
    audit
        .rows
        .sort_by(|a, b| a[1].cmp(&b[1]).then(a[0].cmp(&b[0])));
    audit.gaps.sort();
    audit.gaps.dedup();
    audit
}
