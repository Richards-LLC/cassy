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

// Values are resolved at their read position. No later declaration or
// assignment can change the provenance already captured by an earlier guard.
#[derive(Default, Clone)]
struct Payload {
    codes: BTreeSet<String>,
    authority: bool,
    calls: BTreeSet<String>,
}
impl Payload {
    fn merge(&mut self, other: &Self) {
        self.codes.extend(other.codes.clone());
        self.calls.extend(other.calls.clone());
        self.authority |= other.authority;
    }
    fn reads_authority(&self, helpers: &BTreeSet<String>) -> bool {
        self.authority || self.calls.iter().any(|call| helpers.contains(call))
    }
    fn resolve(&self, functions: &BTreeMap<String, BTreeSet<String>>) -> BTreeSet<String> {
        let mut codes = self.codes.clone();
        for call in &self.calls {
            if let Some(tags) = functions.get(call) { codes.extend(tags.clone()); }
        }
        codes
    }
}

#[derive(Default, Clone)]
struct Value {
    payload: Payload,
    parts: BTreeMap<String, Value>,
}
impl Value {
    fn summary(&self) -> Payload {
        let mut result = self.payload.clone();
        for value in self.parts.values() { result.merge(&value.summary()); }
        result
    }
    fn merge(&mut self, other: &Self) {
        self.payload.merge(&other.payload);
        for (key, value) in &other.parts { self.parts.entry(key.clone()).or_default().merge(value); }
    }
    fn project(&self, key: &str) -> Self {
        self.parts.get(key).cloned().unwrap_or_else(|| Self { payload: self.payload.clone(), ..Default::default() })
    }
    fn typed(ty: &syn::Type) -> Self {
        match ty {
            syn::Type::Reference(ty) => Self::typed(&ty.elem),
            syn::Type::Paren(ty) => Self::typed(&ty.elem),
            syn::Type::Tuple(ty) => Self { parts: ty.elems.iter().enumerate()
                .map(|(i, ty)| (i.to_string(), Self::typed(ty))).collect(), ..Default::default() },
            syn::Type::Path(ty) => {
                let name = ty.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
                if name == "HookInput" {
                    Self { parts: ["agent_role", "session_id", "agent_id"].into_iter().map(|field|
                        (field.into(), Self { payload: Payload { authority: true, ..Default::default() }, ..Default::default() })).collect(), ..Default::default() }
                } else {
                    Self { payload: Payload { authority: matches!(name.as_str(), "QueueStartPrincipal" | "InvocationContext" | "ProxyCaller" | "MachinePrincipal"), ..Default::default() }, ..Default::default() }
                }
            }
            _ => Self::default(),
        }
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
}

#[derive(Clone)]
pub(super) struct AuthorityValue(Vec<Payload>);
impl AuthorityValue {
    fn reads(&self, helpers: &BTreeSet<String>) -> bool {
        self.0.iter().any(|value| value.reads_authority(helpers))
    }
}

/// Summarize returned identity/predicate values using the same lexical flow
/// as refusal discovery. Incidental reads do not taint a helper's result.
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
    let mut flow = Flow::new("", signature);
    let tail = flow.block(block);
    flow.returns.push(tail.summary());
    Some(AuthorityValue(flow.returns))
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


// Each frame owns declarations; assignment updates the nearest frame.
// Branches join possible values, while closures get isolated captured state.
struct Flow<'a> {
    path: &'a str,
    function: String,
    scopes: Vec<BTreeMap<String, Value>>,
    condition: Option<(Payload, String)>,
    sites: Vec<Site>,
    returns: Vec<Payload>,
    nested: bool,
}
impl<'a> Flow<'a> {
    fn new(path: &'a str, signature: &syn::Signature) -> Self {
        let mut flow = Self { path, function: signature.ident.to_string(), scopes: vec![BTreeMap::new()],
            condition: None, sites: Vec::new(), returns: Vec::new(), nested: false };
        for arg in &signature.inputs {
            if let syn::FnArg::Typed(arg) = arg { flow.bind(&arg.pat, Value::typed(&arg.ty), true); }
        }
        flow
    }
    fn bind(&mut self, pat: &syn::Pat, value: Value, parameter: bool) {
        match pat {
            syn::Pat::Ident(pat) => {
                let mut value = value;
                if parameter && matches!(pat.ident.to_string().as_str(), "caller_id" | "current_agent_id") { value.payload.authority = true; }
                self.scopes.last_mut().unwrap().insert(pat.ident.to_string(), value.clone());
                if let Some((_, subpat)) = &pat.subpat { self.bind(subpat, value, parameter); }
            }
            syn::Pat::Tuple(pat) => self.bind_elements(pat.elems.iter().collect(), value, parameter),
            syn::Pat::TupleStruct(pat) => self.bind_elements(pat.elems.iter().collect(), value, parameter),
            syn::Pat::Struct(pat) => for field in &pat.fields { self.bind(&field.pat, value.project(&member(&field.member)), parameter); },
            syn::Pat::Slice(pat) => self.bind_elements(pat.elems.iter().collect(), value, parameter),
            syn::Pat::Reference(pat) => self.bind(&pat.pat, value, parameter),
            syn::Pat::Paren(pat) => self.bind(&pat.pat, value, parameter),
            syn::Pat::Type(pat) => { let mut value = value; value.merge(&Value::typed(&pat.ty)); self.bind(&pat.pat, value, parameter); }
            syn::Pat::Or(pat) => for pat in &pat.cases { self.bind(pat, value.clone(), parameter); },
            _ => {}
        }
    }
    fn bind_elements(&mut self, patterns: Vec<&syn::Pat>, value: Value, parameter: bool) {
        let rest = patterns.iter().position(|pat| matches!(pat, syn::Pat::Rest(_)));
        let count = patterns.len();
        for (index, pat) in patterns.into_iter().enumerate() {
            let slot = if rest.is_some_and(|rest| index > rest) {
                value.parts.len().saturating_sub(count - index)
            } else { index };
            self.bind(pat, value.project(&slot.to_string()), parameter);
        }
    }
    fn assign(&mut self, place: &syn::Expr, value: Value) {
        match place {
            syn::Expr::Path(path) if path.path.get_ident().is_some() => {
                let name = path.path.get_ident().unwrap().to_string();
                if let Some(scope) = self.scopes.iter_mut().rev().find(|scope| scope.contains_key(&name)) { scope.insert(name, value); }
            }
            syn::Expr::Tuple(tuple) => for (i, place) in tuple.elems.iter().enumerate() { self.assign(place, value.project(&i.to_string())); },
            syn::Expr::Array(array) => for (i, place) in array.elems.iter().enumerate() { self.assign(place, value.project(&i.to_string())); },
            syn::Expr::Struct(item) => for field in &item.fields { self.assign(&field.expr, value.project(&member(&field.member))); },
            syn::Expr::Field(field) => {
                let mut base = self.expr(&field.base);
                base.parts.insert(member(&field.member), value);
                self.assign(&field.base, base);
            }
            syn::Expr::Paren(expr) => self.assign(&expr.expr, value),
            syn::Expr::Unary(expr) => self.assign(&expr.expr, value),
            _ => {}
        }
    }
    fn join(&mut self, other: &[BTreeMap<String, Value>]) {
        for (scope, other) in self.scopes.iter_mut().zip(other) {
            for (key, value) in scope { if let Some(other) = other.get(key) { value.merge(other); } }
        }
    }
    fn block(&mut self, block: &syn::Block) -> Value {
        self.scopes.push(BTreeMap::new());
        let mut tail = Value::default();
        for statement in &block.stmts {
            tail = Value::default();
            match statement {
                syn::Stmt::Local(local) => {
                    let value = local.init.as_ref().map(|init| self.expr(&init.expr)).unwrap_or_default();
                    if let Some(init) = &local.init && let Some((_, otherwise)) = &init.diverge {
                        let prior = self.condition.replace((value.summary(), canonical(&init.expr)));
                        self.expr(otherwise); self.condition = prior;
                    }
                    self.bind(&local.pat, value, false);
                }
                syn::Stmt::Expr(expr, semicolon) => { let value = self.expr(expr); if semicolon.is_none() { tail = value; } }
                syn::Stmt::Macro(mac) => { self.mac(&mac.mac); }
                syn::Stmt::Item(_) => {} // nested functions have their own collector and no captures
            }
        }
        self.scopes.pop();
        tail
    }
    fn record(&mut self, span: proc_macro2::Span, kind: &str, payload: Payload) {
        if let Some((condition, key)) = &self.condition {
            self.sites.push(Site { path: self.path.into(), function: self.function.clone(), line: span.start().line,
                key: format!("{}#{}#{kind}#{key}", self.path, self.function), payload, condition: condition.clone() });
        }
    }
    fn expr(&mut self, expr: &syn::Expr) -> Value {
        match expr {
            syn::Expr::Path(path) => path.path.get_ident().and_then(|name|
                self.scopes.iter().rev().find_map(|scope| scope.get(&name.to_string()).cloned())).unwrap_or_default(),
            syn::Expr::Field(field) => self.expr(&field.base).project(&member(&field.member)),
            syn::Expr::Tuple(tuple) => Value { parts: tuple.elems.iter().enumerate().map(|(i, value)| (i.to_string(), self.expr(value))).collect(), ..Default::default() },
            syn::Expr::Array(array) => Value { parts: array.elems.iter().enumerate().map(|(i, value)| (i.to_string(), self.expr(value))).collect(), ..Default::default() },
            syn::Expr::Struct(item) => {
                let mut value = item.rest.as_ref().map(|rest| self.expr(rest)).unwrap_or_default();
                for field in &item.fields { value.parts.insert(member(&field.member), self.expr(&field.expr)); }
                value
            }
            syn::Expr::Reference(expr) => self.expr(&expr.expr),
            syn::Expr::Paren(expr) => self.expr(&expr.expr),
            syn::Expr::Try(expr) => self.expr(&expr.expr),
            syn::Expr::Block(expr) => self.block(&expr.block),
            syn::Expr::Unsafe(expr) => self.block(&expr.block),
            syn::Expr::Async(expr) => self.closure(&[], &syn::Expr::Block(syn::ExprBlock { attrs: Vec::new(), label: None, block: expr.block.clone() })),
            syn::Expr::Return(expr) => {
                if let Some(value) = &expr.expr {
                    let mut value = self.expr(value).summary();
                    if let Some((condition, _)) = &self.condition { value.merge(condition); }
                    self.returns.push(value);
                }
                Value::default()
            }
            syn::Expr::Assign(expr) => { let value = self.expr(&expr.right); self.assign(&expr.left, value); Value::default() }
            syn::Expr::Binary(expr) => {
                let mut value = self.expr(&expr.left); value.payload.merge(&self.expr(&expr.right).summary());
                if matches!(expr.op, syn::BinOp::AddAssign(_) | syn::BinOp::SubAssign(_) | syn::BinOp::MulAssign(_) | syn::BinOp::DivAssign(_) | syn::BinOp::RemAssign(_) | syn::BinOp::BitXorAssign(_) | syn::BinOp::BitAndAssign(_) | syn::BinOp::BitOrAssign(_) | syn::BinOp::ShlAssign(_) | syn::BinOp::ShrAssign(_)) { self.assign(&expr.left, value.clone()); }
                value
            }
            syn::Expr::Let(expr) => { let value = self.expr(&expr.expr); self.bind(&expr.pat, value.clone(), false); value }
            syn::Expr::Closure(expr) => self.closure(&expr.inputs.iter().collect::<Vec<_>>(), &expr.body),
            syn::Expr::If(expr) => {
                self.scopes.push(BTreeMap::new());
                let condition = self.expr(&expr.cond).summary();
                let common = self.scopes.clone();
                let prior = self.condition.replace((condition.clone(), canonical(&expr.cond)));
                let mut value = self.block(&expr.then_branch);
                self.scopes.pop();
                let then_state = self.scopes.clone();
                self.scopes = common; self.scopes.pop();
                if let Some((_, branch)) = &expr.else_branch { value.merge(&self.expr(branch)); }
                self.join(&then_state);
                self.condition = prior;
                value.payload.merge(&condition);
                value
            }
            syn::Expr::Match(expr) => {
                let source = self.expr(&expr.expr);
                let common = self.scopes.clone();
                let prior = self.condition.clone();
                let mut value = Value::default();
                let mut joined = common.clone();
                for arm in &expr.arms {
                    self.scopes = common.clone(); self.scopes.push(BTreeMap::new());
                    self.bind(&arm.pat, source.clone(), false);
                    let (condition, key) = if let Some((_, guard)) = &arm.guard { (self.expr(guard).summary(), canonical(guard)) }
                        else { (source.summary(), canonical(&expr.expr)) };
                    self.condition = Some((condition, key));
                    value.merge(&self.expr(&arm.body)); self.scopes.pop();
                    let arm_state = self.scopes.clone(); self.scopes = joined; self.join(&arm_state); joined = self.scopes.clone();
                }
                self.scopes = joined; self.condition = prior;
                value.payload.merge(&source.summary());
                value
            }
            syn::Expr::While(expr) => {
                let before = self.scopes.clone(); self.scopes.push(BTreeMap::new());
                let condition = self.expr(&expr.cond).summary();
                let prior = self.condition.replace((condition, canonical(&expr.cond)));
                self.block(&expr.body); self.scopes.pop(); self.join(&before); self.condition = prior;
                Value::default()
            }
            syn::Expr::ForLoop(expr) => {
                let source = self.expr(&expr.expr); let before = self.scopes.clone(); self.scopes.push(BTreeMap::new());
                self.bind(&expr.pat, source, false); self.block(&expr.body); self.scopes.pop(); self.join(&before);
                Value::default()
            }
            syn::Expr::Loop(expr) => { let before = self.scopes.clone(); let value = self.block(&expr.body); self.join(&before); value }
            syn::Expr::Call(call) => {
                let name = match call.func.as_ref() { syn::Expr::Path(path) => path.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default(), _ => String::new() };
                let refusal = matches!(name.as_str(), "Err" | "error" | "invalid_params" | "internal_error")
                    || name == "with_pre_tool_permission" && call.args.first().and_then(super::literal).as_deref() == Some("deny");
                let prior_nested = self.nested; self.nested |= refusal;
                let mut value = self.expr(&call.func);
                value.payload.calls.insert(name.clone());
                value.payload.authority |= authority_symbol(&name)
                    || matches!(name.as_str(), "var" | "var_os") && call.args.first().and_then(super::literal).is_some_and(|key| identity_env(&key))
                    || name == "from_process" && matches!(call.func.as_ref(), syn::Expr::Path(path) if path.path.segments.iter().any(|s| s.ident == "InvocationContext"));
                if matches!(name.as_str(), "tag" | "refusal_tag" | "override_tag") {
                    if let Some(code) = call.args.first().and_then(super::literal) { value.payload.codes.insert(code); }
                }
                let constructor = name.chars().next().is_some_and(char::is_uppercase);
                for (index, arg) in call.args.iter().enumerate() {
                    let arg = self.expr(arg);
                    if constructor { value.parts.insert(index.to_string(), arg); }
                    else { value.payload.merge(&arg.summary()); }
                }
                self.nested = prior_nested;
                if refusal && !prior_nested { self.record(expr.span(), &name, value.summary()); }
                value
            }
            syn::Expr::MethodCall(call) => {
                let mut value = Value { payload: self.expr(&call.receiver).summary(), ..Default::default() };
                value.payload.authority |= authority_symbol(&call.method.to_string());
                for arg in &call.args { value.payload.merge(&self.expr(arg).summary()); }
                value
            }
            syn::Expr::Macro(expr) => self.mac(&expr.mac),
            _ => {
                struct Children<'a, 'b> { flow: &'a mut Flow<'b>, value: Value }
                impl<'ast> Visit<'ast> for Children<'_, '_> {
                    fn visit_expr(&mut self, expr: &'ast syn::Expr) { self.value.payload.merge(&self.flow.expr(expr).summary()); }
                }
                let mut children = Children { flow: self, value: Value::default() };
                visit::visit_expr(&mut children, expr); children.value
            }
        }
    }
    fn closure(&mut self, inputs: &[&syn::Pat], body: &syn::Expr) -> Value {
        let saved = self.scopes.clone(); self.scopes.push(BTreeMap::new());
        for input in inputs { self.bind(input, Value::default(), true); }
        let prior = self.condition.take(); let outer_returns = std::mem::take(&mut self.returns);
        let mut value = self.expr(body);
        for returned in &self.returns { value.payload.merge(returned); }
        self.returns = outer_returns; self.condition = prior; self.scopes = saved;
        value
    }
    fn mac(&mut self, mac: &syn::Macro) -> Value {
        use syn::{parse::Parser, punctuated::Punctuated, Token};
        let name = mac.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
        let mut raw = Function::default(); Body { function: &mut raw }.visit_macro(mac);
        let mut value = Value { payload: Payload { codes: raw.refused, ..Default::default() }, ..Default::default() };
        let args = Punctuated::<syn::Expr, Token![,]>::parse_terminated.parse2(mac.tokens.clone()).or_else(|_| {
            let first = mac.tokens.clone().into_iter().take_while(|token| !matches!(token, proc_macro2::TokenTree::Punct(mark) if mark.as_char() == ','))
                .collect::<proc_macro2::TokenStream>();
            syn::parse2::<syn::Expr>(first).map(|expr| std::iter::once(expr).collect())
        });
        let mut condition = None;
        if let Ok(args) = args { for (index, arg) in args.iter().enumerate() {
            let arg_value = self.expr(arg); value.payload.merge(&arg_value.summary());
            if index == 0 { condition = Some((arg_value.summary(), canonical(arg))); }
        } }
        if !self.nested && name == "ensure" {
            let prior = std::mem::replace(&mut self.condition, condition); self.record(mac.span(), "ensure", value.summary()); self.condition = prior;
        } else if !self.nested && name == "bail" { self.record(mac.span(), "bail", value.summary()); }
        value
    }
}
fn member(member: &syn::Member) -> String {
    match member { syn::Member::Named(name) => name.to_string(), syn::Member::Unnamed(index) => index.index.to_string() }
}

pub(super) fn rust(path: &str, _source: &str, file: &syn::File, sites: &mut Vec<Site>) {
    struct Collector<'a, 'b> { path: &'a str, sites: &'b mut Vec<Site> }
    impl<'ast> Visit<'ast> for Collector<'_, '_> {
        fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) { if !test_only(&item.attrs) { visit::visit_item_mod(self, item); } }
        fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) { if !test_only(&item.attrs) { visit::visit_item_impl(self, item); } }
        fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
            if !test_only(&item.attrs) { let mut flow = Flow::new(self.path, &item.sig); flow.block(&item.block); self.sites.extend(flow.sites); visit::visit_item_fn(self, item); }
        }
        fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
            if !test_only(&item.attrs) { let mut flow = Flow::new(self.path, &item.sig); flow.block(&item.block); self.sites.extend(flow.sites); visit::visit_impl_item_fn(self, item); }
        }
    }
    Collector { path, sites }.visit_file(file);
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
                payload: Payload { codes, ..Default::default() }, condition: Payload { authority: true, ..Default::default() },
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

fn baseline(root: &Path, target: &str, audit: &mut Audit) -> Counts {
    let path = root.join(BASELINE);
    if !path.exists() { return Counts::new(); }
    let current = match std::fs::read(&path).map_err(|error| error.to_string()).and_then(|bytes| decode_baseline(&bytes)) {
        Ok(counts) => counts,
        Err(error) => { audit.gaps.push(format!("{BASELINE}: {error}")); return Counts::new(); }
    };
    // Only landed target history limits the allowance. Unmerged delivery
    // commits can refine a first seed; a landed reduction/deletion is final.
    // BASE_SHA is the existing CI comparison base; local calls use the merge
    // base with origin/main. Fixtures name an explicit target branch.
    if root.join(".git").exists() {
        let complete = Command::new("git").args(["rev-parse", "--is-shallow-repository"]).current_dir(root).output()
            .is_ok_and(|result| result.status.success() && result.stdout == b"false\n");
        if !complete {
            audit.gaps.push(format!("{BASELINE}: complete Git history is required to validate the ratchet"));
            return current;
        }
        let target = if target == "origin/main" {
            Command::new("git").args(["merge-base", "HEAD", target]).current_dir(root).output()
        } else {
            Command::new("git").args(["rev-parse", "--verify", &format!("{target}^{{commit}}")]).current_dir(root).output()
        };
        let target = match target {
            Ok(result) if result.status.success() => String::from_utf8_lossy(&result.stdout).trim().to_owned(),
            _ => { audit.gaps.push(format!("{BASELINE}: cannot resolve landed baseline target")); return current; }
        };
        let history = Command::new("git").args(["log", "--first-parent", "--no-renames", "--diff-filter=AMDT", "--format=%H", &target, "--", BASELINE]).current_dir(root).output();
        match history {
            Ok(history) if history.status.success() => {
                for sha in String::from_utf8_lossy(&history.stdout).lines() {
                    let older = (|| {
                        let tree = Command::new("git").args(["ls-tree", "--name-only", sha, "--", BASELINE]).current_dir(root).output().map_err(|error| error.to_string())?;
                        if !tree.status.success() { return Err(format!("cannot read baseline tree at {sha}")); }
                        if tree.stdout.is_empty() { return Ok(Counts::new()); }
                        let blob = Command::new("git").args(["show", &format!("{sha}:{BASELINE}")]).current_dir(root).output().map_err(|error| error.to_string())?;
                        if !blob.status.success() { return Err(format!("cannot read baseline history at {sha}")); }
                        decode_baseline(&blob.stdout)
                    })();
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

pub(super) fn finish(root: &Path, target: &str, sites: Vec<Site>, functions: &[Function], registered: &BTreeSet<String>, audit: &mut Audit) {
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
    let allowed = baseline(root, target, audit);
    for site in sites {
        if !site.condition.reads_authority(&helpers) { continue; }
        let codes = site.payload.resolve(&functions);
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
