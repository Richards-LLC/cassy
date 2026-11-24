//! cas-d68f: park names every symbol the delivery left without a caller,
//! through the real worker close.
use super::blast_radius_park_tests::{ID, LANE, call, fixture, git};
use crate::test_support::TestEnvGuard;
use std::path::Path;

type Files = &'static [(&'static str, &'static str)];

/// Nov 23 v4.53: the Commander redesign dropped the last `hasResult` caller
/// and parked; the verifier then rejected it for that one helper. The same
/// redesign dropped the only literal `status-chip--idle`, which markup still
/// builds from its prefix.
const NOV23_BASE: Files = &[
    ("hub-web/src/account-usage-check.ts", "export class AccountUsageChecks {\n  private states = new Map<string, { result?: number }>();\n  private key(machine: string, account: string): string { return JSON.stringify([machine, account]); }\n  hasResult(machine: string, account: string): boolean { return Boolean(this.states.get(this.key(machine, account))?.result); }\n  result(machine: string, account: string): number | undefined { return this.states.get(this.key(machine, account))?.result; }\n}\n"),
    ("hub-web/src/launch-rows.ts", "import { AccountUsageChecks } from \"./account-usage-check\";\nexport function accountRow(checks: AccountUsageChecks, machine: string, account: string): string {\n  const meter = checks.result(machine, account);\n  return checks.hasResult(machine, account) ? `<li class=\"launch-account-label\">${meter}<span class=\"status-chip status-chip--idle\">Off</span></li>` : \"\";\n}\n"),
    ("hub-web/src/launch-rows.css", ".launch-account-label { display: flex; }\n.status-chip--idle,\n.status-chip--off { color: gray; }\n"),
    // Markup builds the other chip classes, so `status-chip--idle` stays in use.
    ("hub-web/src/status.ts", "export const chip = (state: string): string => `status-chip status-chip--${state}`;\n"),
    ("hub-web/src/main.ts", "import { accountRow } from \"./launch-rows\";\nconsole.log(accountRow);\n"),
];
const NOV23_REDESIGN: Files = &[(
    "hub-web/src/launch-rows.ts",
    "import { AccountUsageChecks } from \"./account-usage-check\";\nexport function accountRow(checks: AccountUsageChecks, machine: string, account: string): string {\n  return `<li class=\"launch-account-label\">${checks.result(machine, account) ?? \"Not reported\"}</li>`;\n}\n",
)];

fn write(root: &Path, files: Files) {
    for (path, body) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
}

/// Parks `delivery` on a target holding `base`; returns the reply and the
/// `path name` pairs it lists as left without a caller.
async fn park(base: Files, delivery: &[Files]) -> (String, Vec<String>) {
    let mut env = TestEnvGuard::temp_home();
    let f = fixture(&mut env);
    let repo = f.dir.path();
    write(repo, base);
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "seed sources"]);
    git(repo, &["push", "-q", "origin", "main"]);
    git(&f.worker_path, &["merge", "-q", "--no-edit", "main"]);
    for files in delivery {
        write(&f.worker_path, files);
    }
    git(&f.worker_path, &["add", "."]);
    git(&f.worker_path, &["commit", "-q", "-m", &format!("refactor({ID}): redesign")]);
    git(&f.worker_path, &["push", "-q", "origin", LANE]);
    let reply = call(&f.worker, serde_json::json!({"action": "close", "id": ID, "reason": "redesign"})).await;
    let listed = reply
        .lines()
        .filter_map(|line| {
            let (location, rest) = line.strip_prefix("- ")?.split_once(' ')?;
            let (path, number) = location.rsplit_once(':')?;
            number.parse::<usize>().ok()?;
            let (name, _) = rest.split_once(" (")?;
            Some(format!("{path} {name}"))
        })
        .collect();
    (reply, listed)
}

#[tokio::test]
async fn park_names_every_symbol_the_delivery_left_without_a_caller_cas_d68f() {
    let cases: [(&str, Files, &[Files], &[&str]); 4] = [
        ("nov23_has_result", NOV23_BASE, &[NOV23_REDESIGN], &["hub-web/src/account-usage-check.ts hasResult"]),
        (
            "every_orphan_in_one_refusal",
            &[
                ("hub-web/src/accounts-model.ts", "export function accountMeters(used: number): number[] { return [used]; }\nexport function accountLabel(name: string): string { return name; }\n"),
                ("hub-web/src/accounts-panel.ts", "import { accountLabel, accountMeters } from \"./accounts-model\";\nexport function panel(name: string): string { return `<div class=\"accounts-bar\">${accountMeters(1)}${accountLabel(name)}</div>`; }\n"),
                ("hub-web/src/accounts-panel.css", ".accounts-bar { outline: 1px solid; }\n"),
                ("hub-web/src/main.ts", "import { panel } from \"./accounts-panel\";\nconsole.log(panel(\"x\"));\n"),
                ("crates/widget/src/meter.rs", "pub fn meter_width() -> u32 { 4 }\n"),
                ("crates/widget/src/gauge.rs", "pub fn gauge() -> u32 { crate::meter::meter_width() }\n"),
            ],
            &[&[
                ("hub-web/src/accounts-panel.ts", "import { accountLabel } from \"./accounts-model\";\nexport function panel(name: string): string { return `<div class=\"accounts-ledger\">${accountLabel(name)}</div>`; }\n"),
                ("crates/widget/src/gauge.rs", "pub fn gauge() -> u32 { 4 }\n"),
            ]],
            &[
                "crates/widget/src/meter.rs meter_width",
                "hub-web/src/accounts-model.ts accountMeters",
                "hub-web/src/accounts-panel.css accounts-bar",
            ],
        ),
        (
            "already_unused_at_the_base",
            &[
                ("hub-web/src/legacy.ts", "export function legacyHelper(): number { return 1; }\n"),
                ("hub-web/src/legacy.css", ".legacy-chip { color: red; }\n"),
            ],
            &[&[
                ("hub-web/src/legacy.ts", "export function legacyHelper(): number { return 2; }\n"),
                ("hub-web/src/legacy.css", ".legacy-chip { color: blue; }\n"),
            ]],
            &[],
        ),
        (
            "refactor_deletes_the_orphan_too",
            NOV23_BASE,
            &[NOV23_REDESIGN, &[(
                "hub-web/src/account-usage-check.ts",
                "export class AccountUsageChecks {\n  private states = new Map<string, { result?: number }>();\n  private key(machine: string, account: string): string { return JSON.stringify([machine, account]); }\n  result(machine: string, account: string): number | undefined { return this.states.get(this.key(machine, account))?.result; }\n}\n",
            )]],
            &[],
        ),
    ];
    for (case, base, delivery, expected) in cases {
        let (reply, listed) = park(base, delivery).await;
        assert_eq!(listed, expected, "{case}: {reply}");
        assert_eq!(crate::gates::codes_in(&reply).contains(&"G-169"), !expected.is_empty(), "{case}: {reply}");
    }
}
