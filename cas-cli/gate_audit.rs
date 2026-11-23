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

#[path = "gate_audit_refusals.rs"]
mod refusals;

const DEFINITION: &str = "A factory check is a policy refusal of a request that is otherwise well-formed and executable. A bypass acceptor is any parameter, env var or flag that lets such a refused request proceed. Scope: checks guarding factory tasks, workers, merges, releases, verification, hooks and scripts protecting those; every registered factory check, bypass candidate, MCP/hook/script result producer and false-positive path is inventoried. Non-factory memory data quality, device APIs and execution confirmations are excluded with reasons. The marker-independent scan discovers only refusals controlled by caller identity/authority (roles, registration, ownership, assignee, leases or live-supervisor predicates). Ordinary I/O, parse, input-validation and not-found error conditions are excluded by construction. Workflow C-row fact emission is deferred to step 3 (cas-d777c, decision 10:54 AM ET).";

#[derive(Default)]
pub struct Audit {
    pub rows: Vec<[String; 6]>,
    pub gaps: Vec<String>,
    pub authority_uncoded: BTreeMap<String, usize>,
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
    /// First leading-comment line through the item's last line, 1-based.
    lines: (usize, usize),
    name: String,
    calls: BTreeSet<String>,
    methods: BTreeSet<String>,
    refused: BTreeSet<String>,
    overridden: BTreeSet<String>,
    flags: BTreeSet<String>,
    conditional: BTreeSet<String>,
    policy: bool,
    authority_value: Option<refusals::AuthorityValue>,
    annotations: BTreeMap<String, String>,
    script_sink: bool,
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
        || name.ends_with("_waiver")
        || name == "decision_reason"
}
fn test_only(attrs: &[syn::Attribute]) -> bool {
    fn requires_test(meta: &syn::Meta) -> bool {
        use syn::{punctuated::Punctuated, Token};
        match meta {
            syn::Meta::Path(path) => path.is_ident("test"),
            syn::Meta::List(list) if list.path.is_ident("all") || list.path.is_ident("any") => {
                let Ok(items) = list.parse_args_with(Punctuated::<syn::Meta, Token![,]>::parse_terminated) else { return false; };
                if list.path.is_ident("all") { items.iter().any(requires_test) }
                else { !items.is_empty() && items.iter().all(requires_test) }
            }
            _ => false,
        }
    }
    attrs.iter().any(|attr| {
        attr.path().segments.last().is_some_and(|segment| segment.ident == "test")
            || attr.path().is_ident("cfg") && matches!(&attr.meta, syn::Meta::List(list) if list.parse_args::<syn::Meta>().is_ok_and(|meta| requires_test(&meta)))
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
impl Body<'_> {
    fn condition(&mut self, expr: &syn::Expr) {
        let mut flags = Flags::default();
        flags.visit_expr(expr);
        let flags = flags.0.into_iter().filter(|name| candidate(name) || self.function.flags.contains(name)).collect::<BTreeSet<_>>();
        self.function.conditional.extend(flags.clone());
        self.function.flags.extend(flags);
    }
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
                    } else {
                        self.function.emissions.push(("runtime registered code".into(), if name == "override_tag" { "override" } else { "refusal" }, expr.span().start().line));
                    }
                }
            }
        }
        visit::visit_expr_call(self, expr);
    }
    fn visit_expr_method_call(&mut self, expr: &'ast syn::ExprMethodCall) {
        self.function.methods.insert(expr.method.to_string());
        // A method spelling alone is not a resolved free-function call:
        // `.get()`/`.find()` must not import unrelated gate handlers.
        if matches!(expr.receiver.as_ref(), syn::Expr::Path(path) if path.path.is_ident("self")) {
            self.function.calls.insert(expr.method.to_string());
        }
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
        self.condition(&expr.cond);
        visit::visit_expr_if(self, expr);
    }
    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        if let Some((_, condition)) = &arm.guard { self.condition(condition); }
        visit::visit_arm(self, arm);
    }
    fn visit_expr_match(&mut self, expr: &'ast syn::ExprMatch) {
        self.condition(&expr.expr);
        visit::visit_expr_match(self, expr);
    }
    fn visit_expr_while(&mut self, expr: &'ast syn::ExprWhile) {
        self.condition(&expr.cond);
        visit::visit_expr_while(self, expr);
    }
    fn visit_pat_ident(&mut self, pat: &'ast syn::PatIdent) {
        let name = pat.ident.to_string();
        if candidate(&name) {
            self.function.flags.insert(name);
        }
        visit::visit_pat_ident(self, pat);
    }
    fn visit_expr_path(&mut self, path: &'ast syn::ExprPath) {
        if path.path.segments.last().is_some_and(|segment| segment.ident == "FalsePositive") {
            self.function.calls.insert("record_false_positive".into());
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
        self.function.script_sink |= s.contains("gates record-refusal") || s.contains("gates record-override");
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

    }
}
#[derive(Default)]
struct Flags(BTreeSet<String>);
impl<'ast> Visit<'ast> for Flags {
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        fn collect(tokens: proc_macro2::TokenStream, names: &mut BTreeSet<String>) {
            for token in tokens {
                match token {
                    proc_macro2::TokenTree::Ident(name) => { names.insert(name.to_string()); }
                    proc_macro2::TokenTree::Group(group) => collect(group.stream(), names),
                    _ => {}
                }
            }
        }
        collect(mac.tokens.clone(), &mut self.0);
    }
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
    /// 0-based index of the leading comment block above 1-based line `start`.
    fn comment_start(&self, start: usize) -> usize {
        let mut annotation_start = start.saturating_sub(1);
        while annotation_start > 0
            && (self.lines[annotation_start - 1]
                .trim_start()
                .starts_with("//")
                || self.lines[annotation_start - 1].trim().is_empty())
        {
            annotation_start -= 1;
        }
        annotation_start
    }
    fn item(&self, name: String, start: usize, end: usize) -> Function {
        let annotations = self.annotations(start, end);
        Function {
            name,
            site: format!("{}:{start}", self.path),
            lines: (self.comment_start(start) + 1, end),
            policy: annotations.contains_key("declared_check"),
            annotations,
            ..Default::default()
        }
    }
    fn annotations(&self, start: usize, end: usize) -> BTreeMap<String, String> {
        let mut annotations = BTreeMap::new();
        for line in &self.lines[self.comment_start(start)..end.min(self.lines.len())] {
            // gate[] is a machine registry declaration. Human message wording
            // cannot turn a status update into a policy check.
            if line.trim_start().strip_prefix("//").map(str::trim_start).is_some_and(|line| (line.starts_with("gate[G-") || line.starts_with("gate[H-")) && !line.starts_with("gate[G-57 ")) {
                annotations.insert("declared_check".into(), String::new());
            }
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
        let mut f = self.item(name, span.start().line, block.span().end().line);
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
        f.authority_value = refusals::authority_value(signature, block);
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
        let mut f = self.item(item.ident.to_string(), item.span().start().line, item.span().end().line);
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
        let mut f = self.item(item.ident.to_string(), item.span().start().line, item.span().end().line);
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

// Named forwarding contracts resolve through actual AST calls, including
// methods. They never borrow a tag from an unrelated producer with the same
// code. Code propagation itself stays conservative about method resolution.
fn reaches(functions: &[Function], producer: &Function, target: &str) -> bool {
    let mut pending = producer.calls.union(&producer.methods).cloned().collect::<Vec<_>>();
    let mut seen = BTreeSet::new();
    while let Some(name) = pending.pop() {
        if name == target { return true; }
        if seen.insert(name.clone()) {
            for function in functions.iter().filter(|f| f.name == name) {
                pending.extend(function.calls.union(&function.methods).cloned());
            }
        }
    }
    false
}

/// Scan declarations, AST control-flow candidates, result types and script flags.
/// Registry tuples are (code, stage, title, source), from the same build scan.
pub fn scan(root: &Path, registry: &[(String, String, String, String)]) -> Audit {
    let target = std::env::var("BASE_SHA").ok().filter(|value| !value.trim().is_empty() && value.bytes().any(|byte| byte != b'0'))
        .unwrap_or_else(|| "origin/main".into());
    scan_against(root, registry, &target)
}

pub fn scan_against(root: &Path, registry: &[(String, String, String, String)], target: &str) -> Audit {
    let mut paths = Vec::new();
    for dir in ["cas-cli/src", "crates", "scripts"] {
        files(&root.join(dir), &mut paths);
    }
    paths.sort();
    let mut functions = Vec::new();
    let mut refusal_sites = Vec::new();
    let policy_files = registry.iter().filter(|(code, _, _, _)|
        code.starts_with('G') || code.starts_with('H') || code.starts_with('C'))
        .filter_map(|(_, _, _, site)| site.rsplit_once(':').map(|(path, _)| path.to_owned()))
        .collect::<BTreeSet<_>>();
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
                Ok(file) => {
                    Functions {
                    path: &relative,
                    lines: source.lines().collect(),
                    output: &mut functions,
                }
                    .visit_file(&file);
                    if refusals::policy_module(&relative, &policy_files) {
                        refusals::rust(&relative, &source, &file, &mut refusal_sites);
                    }
                },
                Err(e) => audit.gaps.push(format!("{relative}: AST scan failed: {e}")),
            }
        } else {
            refusals::script(&relative, &source, &policy_files, &mut refusal_sites);
            let executable = source.lines().filter(|line| !line.trim_start().starts_with('#')).collect::<Vec<_>>().join("\n");
            let codes = bracket_codes(&executable, "");
            let test_harness = relative.rsplit('/').next().is_some_and(|name| name.starts_with("test-") || name.starts_with("test_"));
            if !test_harness { all_refused.extend(codes.clone()); }
            for (index, line) in source.lines().enumerate().filter(|(_, line)| !line.trim_start().starts_with('#')) {
                if !test_harness {
                    for code in bracket_codes(line, "") {
                        audit.rows.push([
                            "direct script code emission".into(), format!("{relative}:{}", index + 1), code,
                            "[CODE]".into(), "—".into(), "script reporter / deferred C-row".into(),
                        ]);
                    }
                }
            }
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
                    let recorded = !test_harness && source.contains("gates record-override");
                    let exclusion = source
                        .lines()
                        .find_map(|line| {
                            line.split_once(&format!("gate-audit[{flag}]:"))
                                .map(|(_, reason)| reason.trim())
                        })
                        .filter(|reason| !reason.is_empty())
                        .or_else(|| relative.rsplit('/').next().filter(|name| name.starts_with("test-") || name.starts_with("test_")).map(|_| "excluded test harness: exercises runtime flags; does not authorize factory work"));
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
                    if test_harness { "excluded test harness" } else { "script result producer" }.into(),
                    relative.clone(),
                    codes.iter().cloned().collect::<Vec<_>>().join(", "),
                    "yes".into(),
                    if source.contains("gates record-override") {
                        "yes"
                    } else {
                        "—"
                    }
                    .into(),
                    if test_harness {
                        "fixture data; does not satisfy runtime code coverage"
                    } else if source.contains("gates record-refusal") {
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
    let mut carried = BTreeMap::<String, BTreeSet<String>>::new();
    for f in &functions {
        if let Some(source) = f.annotations.get("tags_from")
            && f.calls.contains("override_codes_in") && f.calls.contains("override_tag")
            && let Some((path, name)) = source.split_once('#')
            && reaches(&functions, f, name)
        {
            for producer in functions.iter().filter(|producer| producer.name == name && producer.site.rsplit_once(':').unwrap().0.ends_with(path)) {
                carried.entry(f.site.clone()).or_default().extend(producer.overridden.clone());
            }
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
            if let Some(codes) = carried.get(&f.site) { f.overridden.extend(codes.clone()); }
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
    refusals::finish(root, target, refusal_sites, &functions, &registered, &mut audit);
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
        for observer in ["observe_tool_result", "observe_hook_output", "observe_cli_output"] {
            if f.calls.contains(observer) && !f.site.starts_with("cas-cli/src/gates/") {
                audit.rows.push([
                    format!("fact recording boundary {}", f.name), f.site.clone(),
                    "registered codes in final result".into(), "deduplicated".into(), "explicit tags only".into(),
                    format!("facts::{observer}"),
                ]);
            }
        }
        if f.script_sink {
            audit.rows.push([
                format!("embedded script sink {}", f.name), f.site.clone(),
                f.refused.iter().cloned().collect::<Vec<_>>().join(", "),
                "gates record-refusal".into(), "explicit reporter".into(),
                "embedded Git hook EXIT trap preserves refusal status".into(),
            ]);
        }
        if f.name.contains("false_positive") || f.calls.iter().any(|call| call.contains("false_positive")) {
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
                        let forwarded = f.annotations.get("accepted_by").is_some_and(|names| {
                            names.split(',').any(|name| functions.iter().any(|producer| {
                                producer.name == name.trim() && producer.overridden.contains(code)
                                    && reaches(&functions, producer, &f.name)
                            }))
                        });
                        if !f.overridden.contains(code) && !forwarded {
                            audit.gaps.push(format!("{} {flag}: bypass {code} lacks a local tag or a named accepting caller", f.site));
                        }
                    }
                    (
                        "bypass acceptor",
                        format!("{reason}; accepting caller: {}; common result observer", f.annotations.get("accepted_by").map(String::as_str).unwrap_or("local tags")),
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
    // Producers and recording boundaries are discovered by type/call AST.
    // Dynamic MCP ToolRouter dispatch is checked at its one public entry;
    // helper result rows name that topology, not an inferred code-flow proof.
    for (entry, observer) in [("call_tool", "observe_tool_result"), ("process_hook_input", "observe_hook_output")] {
        if functions.iter().any(|f| f.producer.is_some())
            && !functions.iter().any(|f| f.name == entry && f.calls.contains(observer))
        {
            audit.gaps.push(format!("{entry}: result boundary does not invoke {observer}"));
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

/// Inputs of the registry tests themselves: any change can move their verdict.
const AUDIT_INPUTS: &[&str] = &[
    "cas-cli/gate_audit.rs", "cas-cli/gate_audit_refusals.rs", "cas-cli/gate-audit-authority-baseline.json",
    "cas-cli/build.rs", "scripts/check-test-shape.py", "scripts/release.sh", "scripts/fast-rows-pre-push.sh",
];

/// Whether `scan_against` reads this repository-relative path (see `files`).
fn scanned(path: &str) -> bool {
    let (directories, name) = path.rsplit_once('/').unwrap_or(("", path));
    ["cas-cli/src/", "crates/", "scripts/"].iter().any(|root| path.starts_with(root))
        && !directories.split('/').any(|d| matches!(d, "target" | "node_modules" | "tests" | "fixtures" | ".git"))
        && !name.ends_with("_tests.rs") && name != "tests.rs"
        && name.rsplit_once('.').is_some_and(|(_, extension)| matches!(extension, "rs" | "sh" | "py" | "registry"))
}

/// 1-based inclusive line ranges of `source` that the scan inventories:
/// items with codes, tags, declarations, result producers, bypass candidates
/// or authority refusals. Scripts have no item structure, so an audited
/// script counts whole. A source the scan cannot parse is a gap: all of it.
/// `tree` is every scanned function at HEAD, so authority helpers resolve
/// across files as in `scan_against`; it is parsed only when a refusal site needs it.
fn audited_lines(root: &Path, path: &str, source: &str, policy_files: &BTreeSet<String>, tree: &std::cell::OnceCell<Vec<Function>>) -> Vec<(usize, usize)> {
    let whole = vec![(1, usize::MAX)];
    if !path.ends_with(".rs") {
        let mut sites = Vec::new();
        refusals::script(path, source, policy_files, &mut sites);
        let audited = !sites.is_empty() || source.contains("gates record-") || source.contains("CAS_SKIP_")
            || source.lines().filter(|line| !line.trim_start().starts_with('#')).any(|line| !bracket_codes(line, "").is_empty());
        return if audited { whole } else { Vec::new() };
    }
    let Ok(file) = syn::parse_file(source) else { return whole };
    let mut functions = Vec::new();
    Functions { path, lines: source.lines().collect(), output: &mut functions }.visit_file(&file);
    let mut sites = Vec::new();
    if refusals::policy_module(path, policy_files) {
        refusals::rust(path, source, &file, &mut sites);
    }
    let authority = if sites.is_empty() { Vec::new() } else {
        // This revision's own items stand in for the tree's copy of this file.
        let others = tree.get_or_init(|| tree_functions(root)).iter()
            .filter(|f| f.site.rsplit_once(':').is_none_or(|(site, _)| site != path));
        refusals::authority_lines(&sites, others.chain(&functions))
    };
    functions.iter().filter(|f| {
        f.policy || f.script_sink || f.producer.is_some() || !f.annotations.is_empty() || !f.emissions.is_empty()
            || !f.refused.is_empty() || !f.overridden.is_empty() || !f.conditional.is_empty()
            || f.flags.iter().any(|flag| candidate(flag))
            || authority.iter().any(|line| (f.lines.0..=f.lines.1).contains(line))
    }).map(|f| f.lines).collect()
}

/// Every function `scan_against` collects, for cross-file helper resolution.
fn tree_functions(root: &Path) -> Vec<Function> {
    let mut paths = Vec::new();
    for dir in ["cas-cli/src", "crates", "scripts"] {
        files(&root.join(dir), &mut paths);
    }
    let mut functions = Vec::new();
    for path in paths.iter().filter(|path| path.extension().is_some_and(|e| e == "rs")) {
        let relative = path.strip_prefix(root).unwrap().to_string_lossy();
        if let Ok(source) = fs::read_to_string(path) && let Ok(file) = syn::parse_file(&source) {
            Functions { path: &relative, lines: source.lines().collect(), output: &mut functions }.visit_file(&file);
        }
    }
    functions
}

/// Whether the commits after `base` change anything `scan_against` audits, so
/// a capped worker check runs the registry tests only when their verdict can
/// move. Each hunk side is read against its own revision's AST, so deleted
/// tags count as well as added ones.
pub fn touches_audited(root: &Path, base: &str, registry: &[(String, String, String, String)]) -> Result<bool, String> {
    let git = |args: &[&str]| -> Result<String, String> {
        let output = std::process::Command::new("git").arg("-C").arg(root).args(args).output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let policy_files = registry.iter().filter(|(code, _, _, _)|
        code.starts_with('G') || code.starts_with('H') || code.starts_with('C'))
        .filter_map(|(_, _, _, site)| site.rsplit_once(':').map(|(path, _)| path.to_owned()))
        .collect::<BTreeSet<_>>();
    let diff = git(&["diff", "--unified=0", "--no-color", "--no-renames", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/", base, "HEAD", "--", "cas-cli", "crates", "scripts"])?;
    let mut changed = BTreeMap::<String, [Vec<usize>; 2]>::new();
    let mut path = None::<String>;
    let mut declarations = BTreeSet::new();
    for line in diff.lines() {
        if let Some(header) = line.strip_prefix("diff --git ") {
            path = header.rsplit_once(" b/").map(|(_, path)| path.to_owned());
        } else if let (Some(path), Some(hunk)) = (&path, line.strip_prefix("@@ -")) {
            let mut ranges = hunk.split(' ');
            let sides = [ranges.next(), ranges.next().and_then(|range| range.strip_prefix('+'))];
            let entry = changed.entry(path.clone()).or_default();
            for (side, range) in sides.into_iter().enumerate() {
                let range = range.unwrap_or_default();
                let (start, count) = range.split_once(',').unwrap_or((range, "1"));
                let (Ok(start), Ok(count)) = (start.parse::<usize>(), count.parse::<usize>()) else {
                    return Err(format!("unreadable hunk header: {line}"));
                };
                entry[side].extend(start..start + count);
            }
        } else if let Some(path) = &path
            && line.starts_with(['+', '-']) && !line.starts_with("+++") && !line.starts_with("---")
        {
            // gate[...] and gate-audit[...] comments are declarations anywhere in a scanned file.
            let text = line[1..].trim_start();
            if (text.starts_with("//") || text.starts_with('#')) && (text.contains("gate[") || text.contains("gate-audit[")) {
                declarations.insert(path.clone());
            }
        }
    }
    let tree = std::cell::OnceCell::new();
    for (path, [old, new]) in &changed {
        if AUDIT_INPUTS.contains(&path.as_str()) || path.starts_with("cas-cli/src/gates/") {
            return Ok(true);
        }
        if !scanned(path) {
            continue;
        }
        if declarations.contains(path) {
            return Ok(true);
        }
        for (revision, lines) in [(base, old), ("HEAD", new)] {
            if lines.is_empty() {
                continue;
            }
            let object = format!("{revision}:{path}");
            let source = git(&["show", object.as_str()])?;
            if audited_lines(root, path, &source, &policy_files, &tree).iter().any(|(first, last)| lines.iter().any(|line| first <= line && line <= last)) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
