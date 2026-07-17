use std::{
    fs::File,
    io::SeekFrom,
    path::Path,
    process::{ExitStatus, Stdio},
};

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::{
    fs::OpenOptions,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};
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
        let capture_path = artifact_dir.join(format!(".bash-{nonce}.capture.tmp"));
        let combined_path = artifact_dir.join(format!(".bash-{nonce}.combined.tmp"));
        let stdout = File::create(&capture_path).context("create shell output spool")?;
        let stderr = stdout
            .try_clone()
            .context("clone shell output spool for stderr")?;
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

        snapshot_capture(&capture_path, &combined_path).await?;
        finalize_output(&combined_path, &status).await?;

        Ok(RawToolOutput::file(
            combined_path,
            "text/plain; charset=utf-8",
            !status.success(),
        ))
    }
}

async fn snapshot_capture(capture_path: &Path, output_path: &Path) -> Result<()> {
    let capture = tokio::fs::File::open(capture_path)
        .await
        .context("open shell output capture")?;
    let observed_bytes = capture.metadata().await?.len();
    tokio::fs::remove_file(capture_path)
        .await
        .context("unlink shell output capture")?;

    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output_path)
        .await
        .context("create shell output result")?;
    let mut capture = capture.take(observed_bytes);
    tokio::io::copy(&mut capture, &mut output)
        .await
        .context("snapshot shell output capture")?;
    output.flush().await?;
    Ok(())
}

async fn finalize_output(path: &Path, status: &ExitStatus) -> Result<()> {
    let mut output = OpenOptions::new()
        .read(true)
        .append(true)
        .open(path)
        .await?;
    let output_bytes = output.metadata().await?.len();

    if status.success() {
        if output_bytes == 0 {
            output.write_all(b"(no output)").await?;
        }
    } else {
        if output_bytes > 0 {
            let separator = status_separator(&mut output, output_bytes).await?;
            output.write_all(separator).await?;
        }
        output
            .write_all(unsuccessful_status(status, output_bytes == 0).as_bytes())
            .await?;
    }
    output.flush().await?;
    Ok(())
}

async fn status_separator(
    output: &mut tokio::fs::File,
    output_bytes: u64,
) -> Result<&'static [u8]> {
    let tail_len = output_bytes.min(2) as usize;
    let mut tail = [0_u8; 2];
    output
        .seek(SeekFrom::End(-(tail_len as i64)))
        .await
        .context("seek shell output tail")?;
    output
        .read_exact(&mut tail[..tail_len])
        .await
        .context("read shell output tail")?;
    if tail[..tail_len].ends_with(b"\n\n") {
        Ok(b"")
    } else if tail[..tail_len].ends_with(b"\n") {
        Ok(b"\n")
    } else {
        Ok(b"\n\n")
    }
}

fn unsuccessful_status(status: &ExitStatus, no_output: bool) -> String {
    let no_output = if no_output { " (no output)" } else { "" };
    if let Some(code) = status.code() {
        return format!("Command exited with code {code}{no_output}");
    }
    #[cfg(unix)]
    if let Some(signal) = status.signal() {
        return format!("Command terminated by signal {signal}{no_output}");
    }
    format!("Command terminated without an exit code{no_output}")
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn finalize_empty_output(command: &str) -> String {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("output.tmp");
        tokio::fs::write(&path, b"").await.unwrap();
        let status = Command::new("bash")
            .arg("-c")
            .arg(command)
            .status()
            .await
            .unwrap();
        finalize_output(&path, &status).await.unwrap();
        tokio::fs::read_to_string(path).await.unwrap()
    }

    #[tokio::test]
    async fn empty_success_gets_a_placeholder() {
        assert_eq!(finalize_empty_output(":").await, "(no output)");
    }

    #[tokio::test]
    async fn empty_nonzero_exit_gets_one_status_line() {
        assert_eq!(
            finalize_empty_output("exit 1").await,
            "Command exited with code 1 (no output)"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn empty_signal_termination_gets_one_status_line() {
        assert_eq!(
            finalize_empty_output("kill -TERM $$").await,
            "Command terminated by signal 15 (no output)"
        );
    }
}
