//! JSON-only CLI parity for the Jev MCP and library client.
use crate::jev::JevClient;
use anyhow::Context;
use clap::{Args, Subcommand};
use serde_json::Value;
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Subcommand)]
pub enum JevCommands {
    /// Evaluate state and print a TypeSafe JSON response.
    Ask(JevAskArgs),
    /// Evaluate 1–50 JSONL records and print an ordered JSON response array.
    Batch(JevBatchArgs),
    /// Print an advisory failure label; unavailable evaluations print nothing.
    ClassifyFailure(JevFailureArgs),
}
#[derive(Debug, Clone, Args)]
pub struct JevAskArgs {
    /// Literal text, @file, or - to read standard input.
    #[arg(long)]
    pub state: String,
    /// JSON question map or @file.
    #[arg(long)]
    pub questions: String,
    /// Return a typed unavailable value if Jev cannot evaluate.
    #[arg(long)]
    pub advisory: bool,
}
#[derive(Debug, Clone, Args)]
pub struct JevBatchArgs {
    /// JSONL file of states (or {"state": ...} records); - reads stdin.
    #[arg(long)]
    pub input: String,
    /// JSON question map or @file.
    #[arg(long)]
    pub questions: String,
    /// Write the JSON response array to this file instead of stdout.
    #[arg(long)]
    pub out: Option<PathBuf>,
    #[arg(long)]
    pub advisory: bool,
}
#[derive(Debug, Clone, Args)]
pub struct JevFailureArgs {
    #[arg(long)]
    pub log: PathBuf,
    #[arg(long)]
    pub source: String,
    #[arg(long, default_value = "HEAD^")]
    pub base: String,
    #[arg(long, default_value = "HEAD")]
    pub head: String,
}
pub fn execute(command: &JevCommands, cas_root: &Path) -> anyhow::Result<()> {
    if let JevCommands::ClassifyFailure(args) = command {
        if let Some(label) = crate::jev::failure::label(
            cas_root,
            &std::env::current_dir()?,
            &args.log,
            "",
            &args.source,
            &args.base,
            &args.head,
        ) {
            println!("{label}");
        }
        return Ok(());
    }
    let client = JevClient::from_project(cas_root)?;
    let (value, out) = match command {
        JevCommands::Ask(args) => {
            let state = Value::String(read_argument(&args.state, true)?);
            let questions: Value = serde_json::from_str(&read_argument(&args.questions, false)?)
                .context("Invalid Jev questions JSON")?;
            (
                serde_json::to_value(client.ask(
                    &state,
                    &questions,
                    "cli:jev.ask",
                    args.advisory,
                )?)?,
                None,
            )
        }
        JevCommands::Batch(args) => {
            let input = if args.input == "-" {
                read_stdin()?
            } else {
                fs::read_to_string(&args.input).context("Could not read Jev batch input")?
            };
            let states = parse_jsonl(&input)?;
            let questions: Value = serde_json::from_str(&read_argument(&args.questions, false)?)
                .context("Invalid Jev questions JSON")?;
            (
                serde_json::to_value(client.batch(
                    &states,
                    &questions,
                    "cli:jev.batch",
                    args.advisory,
                )?)?,
                args.out.as_ref(),
            )
        }
        JevCommands::ClassifyFailure(_) => unreachable!("handled above"),
    };
    let json = format!("{}\n", serde_json::to_string_pretty(&value)?);
    if let Some(path) = out {
        fs::write(path, json).context("Could not write Jev batch output")?;
    } else {
        print!("{json}");
    }
    Ok(())
}
fn read_argument(value: &str, stdin: bool) -> anyhow::Result<String> {
    if stdin && value == "-" {
        read_stdin()
    } else if let Some(path) = value.strip_prefix('@') {
        fs::read_to_string(path).context("Could not read Jev input file")
    } else {
        Ok(value.into())
    }
}
fn read_stdin() -> anyhow::Result<String> {
    let mut text = String::new();
    io::stdin().read_to_string(&mut text)?;
    Ok(text)
}
fn parse_jsonl(input: &str) -> anyhow::Result<Vec<Value>> {
    let mut states = Vec::new();
    for (index, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("Invalid Jev JSONL at line {}", index + 1))?;
        states.push(value.get("state").cloned().unwrap_or(value));
        anyhow::ensure!(states.len() <= 50, "Jev batch requires 1–50 records");
    }
    anyhow::ensure!(!states.is_empty(), "Jev batch requires 1–50 records");
    Ok(states)
}
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn jev_cli_parses_ask_batch_and_jsonl() {
        let cli = crate::cli::Cli::try_parse_from([
            "cas",
            "jev",
            "ask",
            "--state",
            "-",
            "--questions",
            "@q.json",
            "--advisory",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(crate::cli::Commands::Jev(JevCommands::Ask(_)))
        ));
        let cli = crate::cli::Cli::try_parse_from([
            "cas",
            "jev",
            "batch",
            "--input",
            "rows.jsonl",
            "--questions",
            "{}",
            "--out",
            "answers.json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(crate::cli::Commands::Jev(JevCommands::Batch(_)))
        ));
        assert_eq!(
            parse_jsonl("\n{\"state\":\"first\"}\n{\"message\":\"second\"}\n").unwrap(),
            vec![
                serde_json::json!("first"),
                serde_json::json!({"message":"second"})
            ]
        );
        assert!(parse_jsonl("").is_err());
        assert!(parse_jsonl(&"{}\n".repeat(51)).is_err());
        assert!(
            parse_jsonl("{}\ninvalid")
                .unwrap_err()
                .to_string()
                .contains("line 2")
        );
    }
    #[test]
    fn jev_failure_cli_parses_log_and_exact_change_refs() {
        let cli = crate::cli::Cli::try_parse_from([
            "cas",
            "jev",
            "classify-failure",
            "--log",
            "failed.log",
            "--source",
            "gate:ci-script-tests",
            "--base",
            "base-tip",
            "--head",
            "merged-tip",
        ])
        .unwrap();
        let Some(crate::cli::Commands::Jev(JevCommands::ClassifyFailure(args))) = cli.command
        else {
            panic!("wrong command")
        };
        assert_eq!(args.log, PathBuf::from("failed.log"));
        assert_eq!(args.source, "gate:ci-script-tests");
        assert_eq!(args.base, "base-tip");
        assert_eq!(args.head, "merged-tip");
    }
}
