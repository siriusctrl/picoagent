use std::{fs::File, process::Stdio};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::{fs::OpenOptions, io::AsyncWriteExt};
use ulid::Ulid;

use crate::{
    model::ToolSpec,
    tools::{RawToolOutput, Tool, ToolContext},
};

const DESCRIPTION: &str = include_str!("description.md");

#[derive(Debug, Default)]
pub struct BashTool;

#[derive(Debug, Deserialize)]
struct BashArgs {
    command: String,
}

#[async_trait]
impl Tool for BashTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".to_owned(),
            description: DESCRIPTION.trim().to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(&self, context: ToolContext, arguments: Value) -> Result<RawToolOutput> {
        let args: BashArgs = serde_json::from_value(arguments).context("invalid bash arguments")?;
        let artifact_dir = context
            .workspace
            .join(".pico/runs")
            .join(&context.run_id)
            .join("artifacts");
        tokio::fs::create_dir_all(&artifact_dir).await?;
        let nonce = Ulid::new();
        let stdout_path = artifact_dir.join(format!(".bash-{nonce}.stdout.tmp"));
        let stderr_path = artifact_dir.join(format!(".bash-{nonce}.stderr.tmp"));
        let combined_path = artifact_dir.join(format!(".bash-{nonce}.combined.tmp"));
        let stdout = File::create(&stdout_path).context("create shell stdout spool")?;
        let stderr = File::create(&stderr_path).context("create shell stderr spool")?;
        let mut command = Command::new("bash");
        command
            .arg("-lc")
            .arg(&args.command)
            .current_dir(&context.workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .with_context(|| format!("run bash command `{}`", args.command))?;
        #[cfg(unix)]
        let mut process_group = ProcessGroup::new(child.id());
        let status = child
            .wait()
            .await
            .with_context(|| format!("wait for bash command `{}`", args.command))?;
        #[cfg(unix)]
        process_group.terminate();

        let exit_code = status
            .code()
            .map_or_else(|| "signal".to_owned(), |code| code.to_string());
        let mut combined = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&combined_path)
            .await?;
        combined
            .write_all(format!("Exit code: {exit_code}\n\n--- stdout ---\n").as_bytes())
            .await?;
        let mut stdout = tokio::fs::File::open(&stdout_path).await?;
        tokio::io::copy(&mut stdout, &mut combined).await?;
        combined.write_all(b"\n\n--- stderr ---\n").await?;
        let mut stderr = tokio::fs::File::open(&stderr_path).await?;
        tokio::io::copy(&mut stderr, &mut combined).await?;
        combined.flush().await?;
        drop(combined);
        tokio::fs::remove_file(stdout_path).await?;
        tokio::fs::remove_file(stderr_path).await?;

        Ok(RawToolOutput::file(
            combined_path,
            "text/plain; charset=utf-8",
            !status.success(),
        ))
    }
}

#[cfg(unix)]
struct ProcessGroup {
    id: Option<u32>,
}

#[cfg(unix)]
impl ProcessGroup {
    fn new(id: Option<u32>) -> Self {
        Self { id }
    }

    fn terminate(&mut self) {
        if let Some(id) = self.id.take()
            && let Ok(id) = i32::try_from(id)
        {
            // The child is placed in a process group whose id equals its pid.
            // A negative pid asks kill(2) to terminate the complete group.
            unsafe {
                libc::kill(-id, libc::SIGKILL);
            }
        }
    }
}

#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.terminate();
    }
}
