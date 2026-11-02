//! Hosted queue validation without changing the caller's checkout or hub.
use std::path::PathBuf;

use clap::Args;

#[derive(Args, Debug, Clone)]
pub struct ReleasePrecheckArgs {
    /// Commit to push to precheck/<full SHA> and dispatch on hosted CI
    pub sha: String,
    /// Report the existing run, including elapsed time, without dispatching again
    #[arg(long)]
    pub status: bool,
    /// Write the SHA and hosted run receipt here (default: Git common directory)
    #[arg(long)]
    pub receipt: Option<PathBuf>,
}

pub fn execute(args: &ReleasePrecheckArgs, cli: &super::Cli) -> anyhow::Result<()> {
    let mut command = std::process::Command::new("python3");
    // Ship the dispatcher with the binary; a fresh checkout needs no scripts,
    // Node dependencies, Playwright installation or Rust toolchain.
    command.args([
        "-c",
        include_str!("../../../scripts/release-precheck.py"),
        &args.sha,
    ]);
    if args.status {
        command.arg("--status");
    }
    if cli.json {
        command.arg("--json");
    }
    if let Some(path) = &args.receipt {
        command.arg("--receipt").arg(path);
    }
    anyhow::ensure!(
        command.status()?.success(),
        "Hosted precheck did not pass; see the run receipt above"
    );
    Ok(())
}
