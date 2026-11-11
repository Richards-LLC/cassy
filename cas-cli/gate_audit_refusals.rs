//! Discover authority-conditioned refusals independently of gate markers.
use super::{Audit, Body, Function, Flags, bracket_codes, test_only};
use std::collections::{BTreeMap, BTreeSet};
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
        let mut names = Flags::default();
        names.visit_expr(expr);
        Self { codes: f.refused, authority: f.authority, calls: f.calls, variables: names.0 }
    }
    fn mac(mac: &syn::Macro) -> Self {
        if let Ok(expr) = syn::parse2::<syn::Expr>(mac.tokens.clone()) {
            return Self::expression(&expr);
        }
        let mut f = Function::default();
        Body { function: &mut f }.visit_macro(mac);
        let mut names = Flags::default();
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


/// Stable identity fields and predicates, never human diagnostic wording.
pub(super) fn authority_symbol(name: &str) -> bool {
    matches!(name, "role" | "agent_role" | "caller_role" | "caller_id" | "agent_id"
        | "assignee" | "owner" | "holder" | "lease_holder" | "supervisor_id" | "worker_id"
        | "AgentRole" | "Worker" | "Supervisor" | "Operator" | "CAS_AGENT_ROLE"
        | "current_agent_id" | "get_agent_id" | "resolve_caller_agent_id"
        | "resolve_live_supervisor_authority" | "resolve_registered_caller")
        || ["is_supervisor", "is_worker", "is_operator", "is_factory_agent", "caller_is_",
            "require_supervisor", "require_operator", "has_live_supervisor", "owns_"].iter().any(|prefix| name.starts_with(prefix))
}

pub(super) struct Site {
    path: String,
    function: String,
    line: usize,
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
        let mut flags = Flags::default(); flags.visit_expr(&expr.left);
        self.invalid.extend(flags.0);
        visit::visit_expr_assign(self, expr);
    }
}


struct Errors<'a> {
    path: &'a str,
    function: String,
    bindings: BTreeMap<String, Payload>,
    condition: Option<Payload>,
    sites: &'a mut Vec<Site>,
    nested: bool,
}
impl Errors<'_> {
    fn record(&mut self, line: usize, payload: Payload) {
        if let Some(condition) = &self.condition {
            self.sites.push(Site {
                path: self.path.into(), function: self.function.clone(), line, payload,
                condition: condition.clone(), bindings: self.bindings.clone(),
            });
        }
    }
    fn body(&mut self, name: String, block: &syn::Block) {
        let prior_name = std::mem::replace(&mut self.function, name);
        let prior_bindings = std::mem::take(&mut self.bindings);
        let prior_condition = self.condition.take();
        let mut bindings = Bindings::default(); bindings.visit_block(block);
        self.bindings = bindings.values.into_iter().filter(|(name, _)|
            bindings.counts.get(name) == Some(&1) && !bindings.invalid.contains(name)).collect();
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
        if !test_only(&item.attrs) { self.body(item.sig.ident.to_string(), &item.block); }
    }
    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !test_only(&item.attrs) { self.body(item.sig.ident.to_string(), &item.block); }
    }
    fn visit_expr_if(&mut self, expr: &'ast syn::ExprIf) {
        self.visit_expr(&expr.cond);
        let prior = self.condition.replace(Payload::expression(&expr.cond));
        self.visit_block(&expr.then_branch);
        if let Some((_, branch)) = &expr.else_branch { self.visit_expr(branch); }
        self.condition = prior;
    }
    fn visit_expr_match(&mut self, expr: &'ast syn::ExprMatch) {
        self.visit_expr(&expr.expr);
        let prior = self.condition.replace(Payload::expression(&expr.expr));
        for arm in &expr.arms { self.visit_arm(arm); }
        self.condition = prior;
    }
    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        let prior = self.condition.clone();
        if let Some((_, guard)) = &arm.guard {
            self.condition = Some(Payload::expression(guard));
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
            self.record(expr.span().start().line, Payload::expression(&syn::Expr::Call(expr.clone())));
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
                let prior = self.condition.replace(Payload::expression(condition));
                self.record(mac.span().start().line, Payload::mac(mac));
                self.condition = prior;
            }
        } else if name.ident == "bail" {
            self.record(mac.span().start().line, Payload::mac(mac));
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
    let mut conditions = Vec::<(usize, bool)>::new();
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
            conditions.push((indent, words.find_iter(condition).any(|word| authority_symbol(word.as_str()))));
        }
        if error.is_match(line) && conditions.last().is_some_and(|(_, authority)| *authority) {
            let mut codes = bracket_codes(line, "");
            for previous in lines[..index].iter().rev().take(3) {
                if previous.trim().is_empty() { break; }
                codes.extend(bracket_codes(previous, ""));
            }
            sites.push(Site {
                path: path.into(), function: "script".into(), line: index + 1,
                payload: Payload { codes, ..Default::default() }, condition: Payload { authority: true, ..Default::default() }, bindings: BTreeMap::new(),
            });
        }
        if trimmed.ends_with("; fi") { conditions.pop(); }
    }
}

pub(super) fn finish(sites: Vec<Site>, functions: &[Function], registered: &BTreeSet<String>, audit: &mut Audit) {
    let mut names = BTreeMap::<String, Vec<&Function>>::new();
    for f in functions { names.entry(f.name.clone()).or_default().push(f); }
    let helpers = names.iter().filter_map(|(name, functions)|
        (functions.len() == 1 && functions[0].authority).then(|| name.clone())).collect();
    let functions = names.into_iter().filter_map(|(name, functions)|
        (functions.len() == 1).then(|| (name, functions[0].refused.clone()))).collect();
    for site in sites {
        if !site.condition.reads_authority(&helpers, &site.bindings, &mut BTreeSet::new()) { continue; }
        let codes = site.payload.resolve(&functions, &site.bindings, &mut BTreeSet::new());
        let covered = !codes.is_empty() && codes.iter().all(|code| registered.contains(code));
        if !covered {
            audit.gaps.push(format!("{}:{} {}: uncoded authority refusal", site.path, site.line, site.function));
        }
        audit.rows.push([
            format!("authority refusal ({})", site.function), format!("{}:{}", site.path, site.line),
            codes.into_iter().collect::<Vec<_>>().join(", "),
            if covered { "resolved" } else { "NO" }.into(), "—".into(),
            "controlling condition reads caller role, identity, ownership, registration or authority; ordinary error conditions excluded by construction".into(),
        ]);
    }
}
