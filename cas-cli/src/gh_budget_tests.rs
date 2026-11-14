//! cas-c24e: calls are counted by caller and class, and one rate-limit
//! response holds every later call off GitHub until the backoff expires.
use super::*;
use crate::bounded_process::{Deadline, run_command};
use crate::test_support::TestEnvGuard;

fn gh(stub: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(stub);
    command.args(args);
    run_command(&mut command, Deadline::after(Duration::from_secs(5)), Duration::from_secs(5)).unwrap()
}

#[cfg(unix)]
#[test]
fn a_rate_limit_backs_off_every_gh_call_and_calls_are_counted_by_caller_cas_c24e() {
    let mut env = TestEnvGuard::temp_home();
    env.set("CAS_TEST_GH_BUDGET", "1");
    let dir = tempfile::tempdir().unwrap();
    let stub = dir.path().join("gh");
    let calls = dir.path().join("calls");
    crate::test_paths::warm_stub(
        &stub,
        &format!(
            "#!/bin/sh\necho \"$1\" >> '{}'\ncase \"$2\" in graphql) echo 'GraphQL: API rate limit already exceeded for user ID 1.' >&2; exit 1 ;; view) echo 'body: our rate limit docs'; echo 'not found' >&2; exit 1 ;; esac\necho '{{}}'\n",
            calls.display()
        ),
    );
    let sent = || std::fs::read_to_string(&calls).unwrap_or_default().lines().count();

    assert!(gh(&stub, &["api", "-X", "GET", "repos/o/r/commits/0123456789abcdef/check-runs", "-F", "per_page=100"]).status.success());
    assert!(!gh(&stub, &["issue", "view", "7"]).status.success());
    assert!(backoff_remaining().is_none(), "only GitHub's refusal on stderr backs off, never a body");
    let query = "query=query($owner:String!){repository(owner:$owner){pullRequests(first:100){nodes{number}}}}";
    assert!(!gh(&stub, &["api", "graphql", "-f", query]).status.success());
    assert!(backoff_remaining().is_some_and(|left| left > Duration::from_secs(60)));

    let held = gh(&stub, &["pr", "list", "--head", "factory/x"]);
    let refusal = backoff_refusal(&held).expect("a held call names the backoff");

    assert_eq!(sent(), 3, "a call during backoff never reaches GitHub");
    assert!(gh(&stub, &["api", "rate_limit"]).status.success(), "the free rate-limit read still runs");

    set_backoff(Duration::ZERO);
    assert!(gh(&stub, &["pr", "list", "--head", "factory/x"]).status.success());
    assert_eq!(sent(), 5);

    let usage = usage(Duration::from_secs(3600), 10);
    assert_eq!((usage.total, usage.graphql, usage.refused), (5, 3, 1));
    let classes: Vec<&str> = usage.top.iter().map(|(_, class, _)| class.as_str()).collect();
    for class in ["api repos/:repo/commits/:id/check-runs", "api graphql repository.pullRequests", "pr list", "api rate_limit"] {
        assert!(classes.contains(&class), "{class} in {classes:?}");
    }
    assert!(usage.top.iter().all(|(caller, _, _)| caller.starts_with("gh_budget_tests.rs:")), "{:?}", usage.top);
}
