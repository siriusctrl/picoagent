use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use picoagent::memory::MemoryScope;

#[derive(Parser)]
#[command(name = "pico", version, about = "A small headless agent harness")]
pub(crate) struct Cli {
    #[arg(long, global = true, default_value = ".")]
    pub workspace: PathBuf,
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Run one autonomous task.
    Run {
        prompt: String,
        #[arg(long, value_enum, default_value = "text")]
        output: OutputFormat,
    },
    /// Continue an interrupted or failed run from its last complete message.
    Resume {
        run_id: String,
        #[arg(long, value_enum, default_value = "text")]
        output: OutputFormat,
    },
    /// Print persisted metadata and the final output for a run.
    Inspect { run_id: String },
    /// Authenticate an OpenAI OAuth provider with the device-code flow.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Inspect and maintain long-term memory.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Inspect discovered Agent Skills.
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum OutputFormat {
    Text,
    Ndjson,
}

#[derive(Subcommand)]
pub(crate) enum AuthCommand {
    Login,
}

#[derive(Subcommand)]
pub(crate) enum MemoryCommand {
    /// Ask the configured general-task model to semantically compact memory.
    Consolidate {
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum ScopeArg {
    User,
    Project,
}

impl From<ScopeArg> for MemoryScope {
    fn from(value: ScopeArg) -> Self {
        match value {
            ScopeArg::User => Self::User,
            ScopeArg::Project => Self::Project,
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum SkillsCommand {
    List,
}
