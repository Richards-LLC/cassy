//! Discover authority-conditioned refusals independently of gate markers.
use super::{Audit, Body, Function, bracket_codes, test_only};
use std::collections::{BTreeMap, BTreeSet};
use std::{path::Path, process::Command};
use syn::{spanned::Spanned, visit::{self, Visit}};

// New files under these policy modules are covered without a gate marker.
// Existing G/H/C declaration files extend the scope (passed by the caller).
const POLICY_ROOTS: &[&str] = &[
    "cas-cli/src/cli/factory", "cas-cli/src/cli/gates_cmd.rs",
    "cas-cli/src/gates/", "cas-cli/src/hooks/",
    "cas-cli/src/mcp/tools/core/task/lifecycle/",
    "cas-cli/src/mcp/tools/core/workflow/",
    "cas-cli/src/mcp/tools/core/verification", "cas-cli/src/mcp/tools/core/qa_pass",
    "cas-cli/src/factory", "cas-cli/src/verification", "cas-cli/src/qa_",
    "cas-cli/src/release", "cas-cli/src/cli/release", "cas-cli/src/ui/factory/",
    "crates/cas-factory/src/",
];

pub(super) fn policy_module(path: &str, declared: &BTreeSet<String>) -> bool {
    declared.contains(path) || POLICY_ROOTS.iter().any(|root| path.starts_with(root))
}

// Value references exclude field/method spellings: model.role must never
// borrow an unrelated local role binding.
#[derive(Default)]
struct References(BTreeSet<String>);
impl<'ast> Visit<'ast> for References {
    fn visit_expr_path(&mut self, expr: &'ast syn::ExprPath) {
        if let Some(name) = expr.path.get_ident() { self.0.insert(name.to_string()); }
        visit::visit_expr_path(self, expr);
    }
    fn visit_expr_field(&mut self, expr: &'ast syn::ExprField) {
        if let syn::Expr::Path(receiver) = expr.base.as_ref()
            && let Some(receiver) = receiver.path.get_ident()
            && let syn::Member::Named(field) = &expr.member
        { self.0.insert(format!("{receiver}.{field}")); }
        visit::visit_expr_field(self, expr);
    }
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        use syn::{parse::Parser, punctuated::Punctuated, Token};
        if let Ok(args) = Punctuated::<syn::Expr, Token![,]>::parse_terminated.parse2(mac.tokens.clone()) {
            for value in args { self.visit_expr(&value); }
        } else {
            let first = mac.tokens.clone().into_iter().take_while(|token|
                !matches!(token, proc_macro2::TokenTree::Punct(mark) if mark.as_char() == ','))
                .collect::<proc_macro2::TokenStream>();
            if let Ok(value) = syn::parse2::<syn::Expr>(first) { self.visit_expr(&value); }
        }
    }
}

#[derive(Default, Clone)]
struct Payload {
    codes: BTreeSet<String>,
    authority: bool,
    calls: BTreeSet<String>,
    variables: BTreeSet<String>,
}
impl Payload {
    fn expression(expr: &syn::Expr) -> Self {
        let mut f = Function::default();
        Body { function: &mut f }.visit_expr(expr);
        let mut names = References::default();
        names.visit_expr(expr);
        Self { codes: f.refused, authority: f.authority, calls: f.calls, variables: names.0 }
    }
    fn mac(mac: &syn::Macro) -> Self {
        if let Ok(expr) = syn::parse2::<syn::Expr>(mac.tokens.clone()) {
            return Self::expression(&expr);
        }
        let mut f = Function::default();
        Body { function: &mut f }.visit_macro(mac);
        let mut names = References::default();
        names.visit_macro(mac);
        Self { codes: f.refused, authority: f.authority, calls: f.calls, variables: names.0 }
    }
    fn reads_authority(&self, helpers: &BTreeSet<String>, bindings: &BTreeMap<String, Payload>, seen: &mut BTreeSet<String>) -> bool {
        self.authority || self.calls.iter().any(|call| helpers.contains(call)) || self.variables.iter().any(|variable|
            seen.insert(variable.clone()) && bindings.get(variable).is_some_and(|value| value.reads_authority(helpers, bindings, seen)))
    }
    fn resolve(&self, functions: &BTreeMap<String, BTreeSet<String>>, bindings: &BTreeMap<String, Payload>, seen: &mut BTreeSet<String>) -> BTreeSet<String> {
        let mut codes = self.codes.clone();
        for call in &self.calls {
            if let Some(tags) = functions.get(call) { codes.extend(tags.clone()); }
        }
        for variable in &self.variables {
            if seen.insert(variable.clone()) {
                if let Some(value) = bindings.get(variable) {
                    codes.extend(value.resolve(functions, bindings, seen));
                }
            }
        }
        codes
    }
}


pub(super) fn identity_env(key: &str) -> bool {
    matches!(key, "CAS_AGENT_ROLE" | "CAS_AGENT_ID" | "CAS_AGENT_NAME" | "CAS_SESSION_ID" | "CAS_FACTORY_SESSION")
}

/// Identity-producing APIs. Generic role/owner fields and names are not roots.
pub(super) fn authority_symbol(name: &str) -> bool {
    matches!(name, "current_agent_id" | "get_agent_id" | "resolve_caller_agent_id"
        | "resolve_live_supervisor_authority" | "resolve_registered_caller"
        | "is_supervisor_from_env" | "is_worker_from_env")
}

pub(super) struct Site {
    path: String,
    function: String,
    line: usize,
    key: String,
    payload: Payload,
    condition: Payload,
    bindings: BTreeMap<String, Payload>,
}
#[derive(Default)]
struct Bindings {
    values: BTreeMap<String, Payload>,
    counts: BTreeMap<String, usize>,
    invalid: BTreeSet<String>,
}
impl<'ast> Visit<'ast> for Bindings {
    fn visit_local(&mut self, local: &'ast syn::Local) {
        if let syn::Pat::Ident(pat) = &local.pat {
            let name = pat.ident.to_string();
            *self.counts.entry(name.clone()).or_default() += 1;
            if pat.mutability.is_some() { self.invalid.insert(name.clone()); }
            if let Some(init) = &local.init { self.values.insert(name, Payload::expression(&init.expr)); }
        }
        visit::visit_local(self, local);
    }
    fn visit_expr_assign(&mut self, expr: &'ast syn::ExprAssign) {
        if let syn::Expr::Path(path) = expr.left.as_ref() && let Some(name) = path.path.get_ident() {
            self.invalid.insert(name.to_string());
        }
        visit::visit_expr_assign(self, expr);
    }
}

fn bindings(signature: &syn::Signature, block: &syn::Block) -> BTreeMap<String, Payload> {
    let mut local = Bindings::default(); local.visit_block(block);
    let mut values = local.values.into_iter().filter(|(name, _)|
        local.counts.get(name) == Some(&1) && !local.invalid.contains(name)).collect::<BTreeMap<_, _>>();
    struct Types(BTreeSet<String>);
    impl<'ast> Visit<'ast> for Types {
        fn visit_path_segment(&mut self, segment: &'ast syn::PathSegment) {
            self.0.insert(segment.ident.to_string()); visit::visit_path_segment(self, segment);
        }
    }
    for arg in &signature.inputs {
        let syn::FnArg::Typed(arg) = arg else { continue; };
        let syn::Pat::Ident(name) = arg.pat.as_ref() else { continue; };
        let mut types = Types(BTreeSet::new()); types.visit_type(&arg.ty);
        let name = name.ident.to_string();
        if types.0.contains("HookInput") {
            for field in ["agent_role", "session_id", "agent_id"] {
                values.insert(format!("{name}.{field}"), Payload { authority: true, ..Default::default() });
            }
        } else if types.0.iter().any(|ty| matches!(ty.as_str(), "QueueStartPrincipal" | "InvocationContext" | "ProxyCaller" | "MachinePrincipal"))
            || matches!(name.as_str(), "caller_id" | "current_agent_id")
        {
            values.insert(name, Payload { authority: true, ..Default::default() });
        }
    }
    values
}

#[derive(Clone)]
pub(super) struct AuthorityValue {
    returns: Vec<Payload>,
    bindings: BTreeMap<String, Payload>,
}
impl AuthorityValue {
    fn reads(&self, helpers: &BTreeSet<String>) -> bool {
        self.returns.iter().any(|value| value.reads_authority(helpers, &self.bindings, &mut BTreeSet::new()))
    }
}

/// Only returned scalar identity/predicate values propagate helper provenance.
/// Reading the role for a log or building an unrelated context is insufficient.
pub(super) fn authority_value(signature: &syn::Signature, block: &syn::Block) -> Option<AuthorityValue> {
    struct Output(bool);
    impl<'ast> Visit<'ast> for Output {
        fn visit_path_segment(&mut self, segment: &'ast syn::PathSegment) {
            self.0 |= matches!(segment.ident.to_string().as_str(), "bool" | "String" | "str" | "AgentRole" | "Agent" | "InvocationContext" | "QueueStartPrincipal");
            visit::visit_path_segment(self, segment);
        }
    }
    let mut output = Output(false); output.visit_return_type(&signature.output);
    if !output.0 { return None; }
    struct Returns(Vec<Payload>);
    impl<'ast> Visit<'ast> for Returns {
        fn visit_expr_return(&mut self, expr: &'ast syn::ExprReturn) {
            if let Some(value) = &expr.expr { self.0.push(Payload::expression(value)); }
        }
        fn visit_item_fn(&mut self, _: &'ast syn::ItemFn) {}
        fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
    }
    let mut values = Returns(Vec::new()); values.visit_block(block);
    if let Some(syn::Stmt::Expr(value, None)) = block.stmts.last() { values.0.push(Payload::expression(value)); }
    Some(AuthorityValue { returns: values.0, bindings: bindings(signature, block) })
}

fn canonical(expr: &syn::Expr) -> String {
    fn normalize(stream: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
        stream.into_iter().map(|token| match token {
            proc_macro2::TokenTree::Group(group) => proc_macro2::TokenTree::Group(
                proc_macro2::Group::new(group.delimiter(), normalize(group.stream()))),
            proc_macro2::TokenTree::Literal(lit) => {
                match syn::parse_str::<syn::LitStr>(&lit.to_string()) {
                    Ok(text) => {
                        let value = text.value();
                        // These are role wire values/environment keys. Other
                        // strings include diagnostic labels passed to guards;
                        // rewording them must never create a new policy site.
                        let value = if identity_env(&value) || matches!(value.as_str(),
                            "worker" | "supervisor" | "operator" | "sub_agent" | "task-verifier" | "ci")
                        { value.as_str() } else { "<text>" };
                        proc_macro2::TokenTree::Literal(proc_macro2::Literal::string(value))
                    }
                    Err(_) => proc_macro2::TokenTree::Literal(lit),
                }
            }
            token => token,
        }).collect()
    }
    let source = expr.span().source_text().unwrap_or_default();
    source.parse::<proc_macro2::TokenStream>().map(|tokens| normalize(tokens).to_string()).unwrap_or(source)
}


struct Errors<'a> {
    path: &'a str,
    function: String,
    bindings: BTreeMap<String, Payload>,
    condition: Option<(Payload, String)>,
    sites: &'a mut Vec<Site>,
    nested: bool,
}
impl Errors<'_> {
    fn record(&mut self, line: usize, kind: &str, payload: Payload) {
        if let Some((condition, key)) = &self.condition {
            self.sites.push(Site {
                path: self.path.into(), function: self.function.clone(), line, payload,
                condition: condition.clone(), bindings: self.bindings.clone(),
                key: format!("{}#{}#{kind}#{key}", self.path, self.function),
            });
        }
    }
    fn body(&mut self, signature: &syn::Signature, block: &syn::Block) {
        let prior_name = std::mem::replace(&mut self.function, signature.ident.to_string());
        let prior_bindings = std::mem::take(&mut self.bindings);
        let prior_condition = self.condition.take();
        self.bindings = bindings(signature, block);
        self.visit_block(block);
        self.bindings = prior_bindings;
        self.function = prior_name;
        self.condition = prior_condition;
    }
}
impl<'ast> Visit<'ast> for Errors<'_> {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !test_only(&item.attrs) { visit::visit_item_mod(self, item); }
    }
    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        if !test_only(&item.attrs) { visit::visit_item_impl(self, item); }
    }
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !test_only(&item.attrs) { self.body(&item.sig, &item.block); }
    }
    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !test_only(&item.attrs) { self.body(&item.sig, &item.block); }
    }
    fn visit_expr_if(&mut self, expr: &'ast syn::ExprIf) {
        self.visit_expr(&expr.cond);
        let prior = self.condition.replace((Payload::expression(&expr.cond), canonical(&expr.cond)));
        self.visit_block(&expr.then_branch);
        if let Some((_, branch)) = &expr.else_branch { self.visit_expr(branch); }
        self.condition = prior;
    }
    fn visit_expr_match(&mut self, expr: &'ast syn::ExprMatch) {
        self.visit_expr(&expr.expr);
        let prior = self.condition.replace((Payload::expression(&expr.expr), canonical(&expr.expr)));
        for arm in &expr.arms { self.visit_arm(arm); }
        self.condition = prior;
    }
    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        let prior = self.condition.clone();
        if let Some((_, guard)) = &arm.guard {
            self.condition = Some((Payload::expression(guard), canonical(guard)));
        }
        self.visit_expr(&arm.body);
        self.condition = prior;
    }
    fn visit_expr_call(&mut self, expr: &'ast syn::ExprCall) {
        let name = match expr.func.as_ref() {
            syn::Expr::Path(path) => path.path.segments.last().map(|s| s.ident.to_string()),
            _ => None,
        };
        let refusal = match name.as_deref() {
            Some("Err" | "error" | "invalid_params" | "internal_error") => true,
            Some("with_pre_tool_permission") => expr.args.first().and_then(super::literal).as_deref() == Some("deny"),
            _ => false,
        };
        if !self.nested && refusal {
            self.record(expr.span().start().line, name.as_deref().unwrap(), Payload::expression(&syn::Expr::Call(expr.clone())));
            self.nested = true;
            visit::visit_expr_call(self, expr);
            self.nested = false;
        } else { visit::visit_expr_call(self, expr); }
    }
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let Some(name) = mac.path.segments.last() else { return; };
        if self.nested { return; }
        if name.ident == "ensure" {
            use syn::{parse::Parser, punctuated::Punctuated, Token};
            if let Ok(args) = Punctuated::<syn::Expr, Token![,]>::parse_terminated.parse2(mac.tokens.clone())
                && let Some(condition) = args.first()
            {
                let prior = self.condition.replace((Payload::expression(condition), canonical(condition)));
                self.record(mac.span().start().line, "ensure", Payload::mac(mac));
                self.condition = prior;
            }
        } else if name.ident == "bail" {
            self.record(mac.span().start().line, "bail", Payload::mac(mac));
        }
    }
}

pub(super) fn rust(path: &str, _source: &str, file: &syn::File, sites: &mut Vec<Site>) {
    Errors { path, function: String::new(), bindings: BTreeMap::new(), condition: None, sites, nested: false }.visit_file(file);
}

pub(super) fn script(path: &str, source: &str, declared: &BTreeSet<String>, sites: &mut Vec<Site>) {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.starts_with("test-") || name.starts_with("test_") { return; }
    if !declared.contains(path) && !["scripts/release", "scripts/factory", "scripts/worker", "scripts/assembly", "scripts/qa-", "scripts/fast-rows", "scripts/train-", "scripts/ci_tiers/"].iter().any(|root| path.starts_with(root)) { return; }
    let error = regex::Regex::new(r"\b(exit|return)\s+[1-9][0-9]*\b|\braise\b|\bsys\.exit\s*\(\s*[1-9]|^\s*(die|fatal|fail)\s+").unwrap();
    let words = regex::Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").unwrap();
    let lines = source.lines().collect::<Vec<_>>();
    let mut conditions = Vec::<(usize, Option<String>)>::new();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') { continue; }
        let indent = line.len() - line.trim_start().len();
        if path.ends_with(".py") && !trimmed.is_empty() {
            while conditions.last().is_some_and(|(depth, _)| *depth >= indent) { conditions.pop(); }
        }
        if matches!(trimmed, "fi" | "esac") { conditions.pop(); continue; }
        if trimmed.starts_with("if ") || trimmed.starts_with("elif ") || trimmed.starts_with("case ") {
            if trimmed.starts_with("elif ") { conditions.pop(); }
            let condition = trimmed.split("then").next().unwrap_or(trimmed).split(':').next().unwrap_or(trimmed);
            conditions.push((indent, words.find_iter(condition).any(|word| identity_env(word.as_str()) || authority_symbol(word.as_str()))
                .then(|| condition.split_whitespace().collect::<Vec<_>>().join(" "))));
        }
        if error.is_match(line) && let Some((_, Some(condition))) = conditions.last() {
            let mut codes = bracket_codes(line, "");
            for previous in lines[..index].iter().rev().take(3) {
                if previous.trim().is_empty() { break; }
                codes.extend(bracket_codes(previous, ""));
            }
            sites.push(Site {
                path: path.into(), function: "script".into(), line: index + 1,
                key: format!("{path}#script#exit#{condition}"),
                payload: Payload { codes, ..Default::default() }, condition: Payload { authority: true, ..Default::default() }, bindings: BTreeMap::new(),
            });
        }
        if trimmed.ends_with("; fi") { conditions.pop(); }
    }
}

const BASELINE: &str = "cas-cli/gate-audit-authority-baseline.json";
type Counts = BTreeMap<String, usize>;

fn decode_baseline(bytes: &[u8]) -> Result<Counts, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if value["schema"] != 1 { return Err("unsupported authority baseline schema".into()); }
    let sites = value["sites"].as_object().ok_or("authority baseline requires a sites object")?;
    sites.iter().map(|(key, count)| {
        let count = count.as_u64().filter(|count| *count > 0).ok_or("baseline counts must be positive")?;
        if key.is_empty() { return Err("baseline site identity is empty".into()); }
        Ok((key.clone(), usize::try_from(count).map_err(|error| error.to_string())?))
    }).collect()
}

fn baseline(root: &Path, audit: &mut Audit) -> Counts {
    let path = root.join(BASELINE);
    if !path.exists() { return Counts::new(); }
    let current = match std::fs::read(&path).map_err(|error| error.to_string()).and_then(|bytes| decode_baseline(&bytes)) {
        Ok(counts) => counts,
        Err(error) => { audit.gaps.push(format!("{BASELINE}: {error}")); return Counts::new(); }
    };
    // Real checkouts retain the minimum allowance from every version, so a
    // removal can never be silently undone by growing the baseline later.
    // Unversioned parser fixtures have no history; the test also exercises a
    // real Git history, including an attempted committed growth and shrink.
    if root.join(".git").exists() {
        let complete = Command::new("git").args(["rev-parse", "--is-shallow-repository"]).current_dir(root).output()
            .is_ok_and(|result| result.status.success() && result.stdout == b"false\n");
        if !complete {
            audit.gaps.push(format!("{BASELINE}: complete Git history is required to validate the ratchet"));
            return current;
        }
        let history = Command::new("git").args(["log", "--first-parent", "--diff-filter=AM", "--format=%H", "--", BASELINE]).current_dir(root).output();
        match history {
            Ok(history) if history.status.success() => {
                for sha in String::from_utf8_lossy(&history.stdout).lines() {
                    let blob = Command::new("git").args(["show", &format!("{sha}:{BASELINE}")]).current_dir(root).output();
                    let older = blob.map_err(|error| error.to_string()).and_then(|blob| {
                        if !blob.status.success() { return Err(format!("cannot read baseline history at {sha}")); }
                        decode_baseline(&blob.stdout)
                    });
                    match older {
                        Ok(older) => for (key, count) in &current {
                            if *count > older.get(key).copied().unwrap_or(0) {
                                audit.gaps.push(format!("{BASELINE}: baseline grew at {key} beyond {sha}"));
                            }
                        },
                        Err(error) => audit.gaps.push(format!("{BASELINE}: {error}")),
                    }
                }
            }
            _ => audit.gaps.push(format!("{BASELINE}: cannot validate baseline history")),
        }
    }
    current
}

pub(super) fn finish(root: &Path, sites: Vec<Site>, functions: &[Function], registered: &BTreeSet<String>, audit: &mut Audit) {
    let mut names = BTreeMap::<String, Vec<&Function>>::new();
    for f in functions { names.entry(f.name.clone()).or_default().push(f); }
    let mut helpers = BTreeSet::new();
    loop {
        let before = helpers.len();
        for (name, functions) in &names {
            if functions.len() == 1 && functions[0].authority_value.as_ref().is_some_and(|value| value.reads(&helpers)) {
                helpers.insert(name.clone());
            }
        }
        if helpers.len() == before { break; }
    }
    let functions = names.into_iter().filter_map(|(name, functions)|
        (functions.len() == 1).then(|| (name, functions[0].refused.clone()))).collect();
    let allowed = baseline(root, audit);
    for site in sites {
        if !site.condition.reads_authority(&helpers, &site.bindings, &mut BTreeSet::new()) { continue; }
        let codes = site.payload.resolve(&functions, &site.bindings, &mut BTreeSet::new());
        let covered = !codes.is_empty() && codes.iter().all(|code| registered.contains(code));
        let count = if covered { 0 } else {
            let count = audit.authority_uncoded.entry(site.key.clone()).or_default(); *count += 1; *count
        };
        let legacy = !covered && count <= allowed.get(&site.key).copied().unwrap_or(0);
        if !covered && !legacy {
            audit.gaps.push(format!("{}:{} {}: uncoded authority refusal", site.path, site.line, site.function));
        }
        audit.rows.push([
            format!("authority refusal ({})", site.function), format!("{}:{}", site.path, site.line),
            codes.into_iter().collect::<Vec<_>>().join(", "),
            if covered { "resolved" } else if legacy { "baseline" } else { "NO" }.into(), "—".into(),
            format!("caller-identity provenance; {}", site.key),
        ]);
    }
}
