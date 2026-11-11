//! Discover error-producing syntax independently of declarations and bypasses.
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
    calls: BTreeSet<String>,
    variables: BTreeSet<String>,
}
impl Payload {
    fn expression(expr: &syn::Expr) -> Self {
        let mut f = Function::default();
        Body { function: &mut f }.visit_expr(expr);
        let mut names = Flags::default();
        names.visit_expr(expr);
        Self { codes: f.refused, calls: f.calls, variables: names.0 }
    }
    fn mac(mac: &syn::Macro) -> Self {
        if let Ok(expr) = syn::parse2::<syn::Expr>(mac.tokens.clone()) {
            return Self::expression(&expr);
        }
        let mut f = Function::default();
        Body { function: &mut f }.visit_macro(mac);
        let mut names = Flags::default();
        names.visit_macro(mac);
        Self { codes: f.refused, calls: f.calls, variables: names.0 }
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

pub(super) struct Site {
    path: String,
    function: String,
    line: usize,
    kind: &'static str,
    payload: Payload,
    bindings: BTreeMap<String, Payload>,
    exclusion: Option<String>,
}

fn exclusion(lines: &[&str], line: usize) -> Option<String> {
    // A per-site annotation, never the surrounding function/module's waiver.
    [line.saturating_sub(1), line.saturating_sub(2)].into_iter()
        .filter_map(|n| lines.get(n))
        .find_map(|line| line.trim_start().strip_prefix("//")
            .or_else(|| line.trim_start().strip_prefix('#'))
            .and_then(|comment| comment.trim_start().strip_prefix("gate-audit[error]:"))
            .map(str::trim).filter(|reason| !reason.is_empty()).map(str::to_owned))
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
    lines: Vec<&'a str>,
    bindings: BTreeMap<String, Payload>,
    forwarded: BTreeSet<String>,
    sites: &'a mut Vec<Site>,
    nested: bool,
}
impl Errors<'_> {
    fn record(&mut self, line: usize, kind: &'static str, payload: Payload, automatic: Option<String>) {
        self.sites.push(Site {
            path: self.path.into(), function: self.function.clone(), line, kind, payload,
            bindings: self.bindings.clone(), exclusion: exclusion(&self.lines, line).or(automatic),
        });
    }
    fn body(&mut self, name: String, block: &syn::Block) {
        self.function = name;
        let mut bindings = Bindings::default(); bindings.visit_block(block);
        self.bindings = bindings.values.into_iter().filter(|(name, _)|
            bindings.counts.get(name) == Some(&1) && !bindings.invalid.contains(name)).collect();
        self.visit_block(block);
        self.bindings.clear();
    }
}
impl<'ast> Visit<'ast> for Errors<'_> {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !test_only(&item.attrs) { visit::visit_item_mod(self, item); }
    }
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !test_only(&item.attrs) { self.body(item.sig.ident.to_string(), &item.block); }
    }
    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !test_only(&item.attrs) { self.body(item.sig.ident.to_string(), &item.block); }
    }
    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        let prior = self.forwarded.clone();
        if let syn::Pat::TupleStruct(pat) = &arm.pat
            && pat.path.segments.last().is_some_and(|s| s.ident == "Err")
            && let Some(syn::Pat::Ident(binding)) = pat.elems.first()
        { self.forwarded.insert(binding.ident.to_string()); }
        visit::visit_arm(self, arm);
        self.forwarded = prior;
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
            let payload = Payload::expression(&syn::Expr::Call(expr.clone()));
            let automatic = if name.as_deref() == Some("Err") {
                expr.args.first().and_then(|value| match value {
                    syn::Expr::Path(path) if path.path.get_ident().is_some_and(|id| self.forwarded.contains(&id.to_string())) =>
                        Some("forwards the matched operation error unchanged; does not originate a policy decision".into()),
                    syn::Expr::Call(call) => if let syn::Expr::Path(path) = call.func.as_ref() {
                        path.path.segments.last().filter(|s| matches!(s.ident.to_string().as_str(),
                            "Io" | "Database" | "Json" | "Yaml" | "Parse" | "Type" | "TaskNotFound" | "NotInitialized"))
                            .map(|s| format!("typed {} data/I/O error; no authority decision", s.ident))
                    } else { None },
                    _ => None,
                })
            } else { None };
            self.record(expr.span().start().line, "Rust error", payload, automatic);
            self.nested = true;
            visit::visit_expr_call(self, expr);
            self.nested = false;
        } else { visit::visit_expr_call(self, expr); }
    }
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if !self.nested && mac.path.segments.last().is_some_and(|s| matches!(s.ident.to_string().as_str(), "bail" | "ensure")) {
            self.record(mac.span().start().line, "Rust refusal macro", Payload::mac(mac), None);
        }
    }
}

pub(super) fn rust(path: &str, source: &str, file: &syn::File, sites: &mut Vec<Site>) {
    Errors { path, function: String::new(), lines: source.lines().collect(), bindings: BTreeMap::new(), forwarded: BTreeSet::new(), sites, nested: false }.visit_file(file);
}

pub(super) fn finish(sites: Vec<Site>, functions: &[Function], registered: &BTreeSet<String>, audit: &mut Audit) {
    let mut names = BTreeMap::<String, Vec<&Function>>::new();
    for f in functions { names.entry(f.name.clone()).or_default().push(f); }
    let functions = names.into_iter().filter_map(|(name, functions)|
        (functions.len() == 1).then(|| (name, functions[0].refused.clone()))).collect();
    for site in sites {
        let codes = site.payload.resolve(&functions, &site.bindings, &mut BTreeSet::new());
        let covered = !codes.is_empty() && codes.iter().all(|code| registered.contains(code));
        let reason = if covered { "registered code in error payload or its resolved value/helper".into() }
            else if let Some(reason) = site.exclusion { reason }
            else {
                audit.gaps.push(format!("{}:{} {}: unclassified {}", site.path, site.line, site.function, site.kind));
                "UNCLASSIFIED: needs a gate tag or per-site gate-audit[error] exclusion".into()
            };
        audit.rows.push([
            format!("{} ({})", site.kind, site.function), format!("{}:{}", site.path, site.line),
            codes.into_iter().collect::<Vec<_>>().join(", "),
            if covered { "resolved" } else { "—" }.into(), "—".into(), reason,
        ]);
    }
}
