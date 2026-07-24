use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "fiasco",
    version,
    about = "Orchestrate multiple agents and background jobs"
)]
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
    /// Run one orchestrated task.
    Run {
        prompt: String,
        #[arg(long, value_enum, default_value = "text")]
        output: OutputFormat,
    },
    /// Continue an interrupted or failed run after repairing its message tail.
    Resume {
        run_id: String,
        #[arg(long, value_enum, default_value = "text")]
        output: OutputFormat,
    },
    /// Inspect a run's committed transcript or persisted summary.
    Inspect {
        run_id: String,
        /// Continue following newly completed message lines.
        #[arg(long, conflicts_with_all = ["output", "summary"])]
        follow: bool,
        /// Write complete message records instead of opening the viewer.
        #[arg(long, value_enum, conflicts_with_all = ["follow", "summary"])]
        output: Option<InspectOutput>,
        /// Print persisted metadata and final output.
        #[arg(long, conflicts_with_all = ["follow", "output"])]
        summary: bool,
    },
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

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum InspectOutput {
    Ndjson,
}

#[derive(Subcommand)]
pub(crate) enum AuthCommand {
    Login,
}

#[derive(Subcommand)]
pub(crate) enum MemoryCommand {
    /// Run semantic memory consolidation through the ordinary agent.
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

#[derive(Subcommand)]
pub(crate) enum SkillsCommand {
    List,
}
