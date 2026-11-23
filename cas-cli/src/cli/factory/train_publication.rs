use std::path::{Path, PathBuf};
use clap::{Args, Subcommand};
use crate::train_owner::publication;

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct PublicationArgs {
    #[command(subcommand)]
    command: PublicationCommand,
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
enum PublicationCommand {
    PublishBranch(BranchArgs),
    ArmPr(ArmArgs),
    PublishTag(TagArgs),
    CreateRelease(ReleaseArgs),
    Reconcile(ReconcileArgs),
    IsEnrolled(BranchQuery),
    IsBoundTag(TagQuery),
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
struct BranchArgs {
    #[arg(long)] branch: String,
    #[arg(long)] sha: String,
    #[arg(long, default_value = "origin")] remote: String,
}
#[derive(Args, Debug, Clone, PartialEq, Eq)]
struct ArmArgs {
    #[arg(long)] pr: u64,
    #[arg(long)] sha: String,
}
#[derive(Args, Debug, Clone, PartialEq, Eq)]
struct TagArgs {
    #[arg(long)] tag: String,
    #[arg(long)] sha: String,
    #[arg(long)] run: Option<u64>,
    #[arg(long)] attempt: Option<u64>,
    #[arg(long)] isolated_dry_run: bool,
}
#[derive(Args, Debug, Clone, PartialEq, Eq)]
struct ReleaseArgs {
    #[command(flatten)] publication: TagArgs,
    #[arg(long)] repo: String,
    #[arg(long)] notes: PathBuf,
    #[arg(long)] title: String,
    #[arg(long)] prerelease: bool,
    #[arg(required = true)] assets: Vec<PathBuf>,
}
#[derive(Args, Debug, Clone, PartialEq, Eq)]
struct ReconcileArgs {
    #[arg(long)] train: String,
    #[arg(long)] operation: String,
    #[arg(long)] run: u64,
    #[arg(long)] attempt: u64,
}
#[derive(Args, Debug, Clone, PartialEq, Eq)]
struct BranchQuery { #[arg(long)] branch: String }
#[derive(Args, Debug, Clone, PartialEq, Eq)]
struct TagQuery { #[arg(long)] tag: String }

pub(super) fn execute(args: &PublicationArgs, root: Option<&Path>) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let repo = crate::worktree::GitOperations::detect_repo_root(&cwd)?;
    let owner = root.map(|root| crate::mcp::CasCore::for_cli(root.to_path_buf()));
    let result = match &args.command {
        PublicationCommand::PublishBranch(args) => publication::publish_branch(owner.as_ref(), &repo, &args.branch, &args.sha, &args.remote),
        PublicationCommand::ArmPr(args) => publication::arm_pr(owner.as_ref(), &repo, args.pr, Some(&args.sha)),
        PublicationCommand::PublishTag(args) => publication::publish_tag(&repo, &args.tag, &args.sha, args.run, args.attempt, args.isolated_dry_run),
        PublicationCommand::CreateRelease(args) => {
            let mut command = vec!["release".to_string(), "create".into(), args.publication.tag.clone(), "--repo".into(), args.repo.clone(),
                "--verify-tag".into(), "--title".into(), args.title.clone(), "--notes-file".into(), args.notes.to_string_lossy().into_owned()];
            if args.prerelease { command.push("--prerelease".into()); }
            command.extend(args.assets.iter().map(|path| path.to_string_lossy().into_owned()));
            publication::create_release(&repo, &args.publication.tag, &args.publication.sha, &args.repo, args.publication.run, args.publication.attempt, args.publication.isolated_dry_run,
                &command.iter().map(String::as_str).collect::<Vec<_>>())
        }
        PublicationCommand::Reconcile(args) => publication::reconcile(&repo, &args.train, &args.operation, args.run, args.attempt),
        PublicationCommand::IsEnrolled(args) => {
            let enrolled = crate::worktree::git::train_authority::Snapshot::all(&repo).map_err(anyhow::Error::msg)?
                .iter().any(|snapshot| snapshot.record.target == args.branch.trim_start_matches("refs/heads/"));
            println!("{enrolled}");
            Ok(())
        }
        PublicationCommand::IsBoundTag(args) => {
            let bound = crate::worktree::git::train_authority::Snapshot::all(&repo).map_err(anyhow::Error::msg)?
                .iter().any(|snapshot| snapshot.record.external_operations.iter().any(|operation| operation.scope.tag == args.tag));
            println!("{bound}");
            Ok(())
        }
    };
    result.map_err(anyhow::Error::msg)
}
