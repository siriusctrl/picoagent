use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{io::AsyncWriteExt, process::Command};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    RunStart,
    RunEnd,
    ToolBefore,
    ToolAfter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookInvocation {
    pub event: HookEvent,
    pub payload: Value,
}

/// A hook may replace the payload for the next hook in the same pipeline.
/// Omitting `payload`, or writing no stdout, leaves it unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookOutput {
    #[serde(default)]
    pub payload: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct CommandHook {
    pub name: String,
    pub event: HookEvent,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
}

impl CommandHook {
    pub fn new(
        name: impl Into<String>,
        event: HookEvent,
        program: impl Into<String>,
        args: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            event,
            program: program.into(),
            args,
            cwd: None,
        }
    }

    pub fn from_command_line(
        name: impl Into<String>,
        event: HookEvent,
        command_line: &str,
    ) -> Result<Self> {
        let mut parts = shell_words::split(command_line).context("invalid hook command line")?;
        if parts.is_empty() {
            bail!("hook command must not be empty");
        }
        let program = parts.remove(0);
        Ok(Self::new(name, event, program, parts))
    }

    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    async fn run(&self, invocation: &HookInvocation, fallback_cwd: &Path) -> Result<HookOutput> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .current_dir(self.cwd.as_deref().unwrap_or(fallback_cwd))
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to start hook `{}`", self.name))?;
        let input = serde_json::to_vec(invocation)?;
        let mut stdin = child.stdin.take().context("hook stdin was unavailable")?;
        let write_result = stdin.write_all(&input).await;
        drop(stdin);
        let output = child.wait_with_output().await?;
        if !output.status.success() {
            bail!(
                "hook `{}` exited with {}: {}",
                self.name,
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        write_result?;
        if output.stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(HookOutput::default());
        }
        serde_json::from_slice(&output.stdout)
            .with_context(|| format!("hook `{}` did not emit valid JSON", self.name))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookRunResult {
    pub payload: Value,
    pub executed: Vec<String>,
}

/// Hooks execute synchronously in registration order. Each JSON output becomes
/// the next hook's input, making ordering deterministic and observable.
#[derive(Debug, Clone, Default)]
pub struct HookPipeline {
    hooks: Vec<CommandHook>,
}

impl HookPipeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, hook: CommandHook) {
        self.hooks.push(hook);
    }

    pub fn hooks(&self) -> &[CommandHook] {
        &self.hooks
    }

    pub async fn run(&self, event: HookEvent, payload: Value, cwd: &Path) -> Result<HookRunResult> {
        let mut payload = payload;
        let mut executed = Vec::new();
        for hook in self.hooks.iter().filter(|hook| hook.event == event) {
            let invocation = HookInvocation {
                event,
                payload: payload.clone(),
            };
            if let Some(replacement) = hook.run(&invocation, cwd).await?.payload {
                payload = replacement;
            }
            executed.push(hook.name.clone());
        }
        Ok(HookRunResult { payload, executed })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn hooks_run_in_registration_order_and_chain_payloads() {
        let mut pipeline = HookPipeline::new();
        pipeline.register(CommandHook::new(
            "first",
            HookEvent::ToolBefore,
            "sh",
            vec![
                "-c".into(),
                "cat >/dev/null; printf '%s' '{\"payload\":{\"step\":1}}'".into(),
            ],
        ));
        pipeline.register(CommandHook::new(
            "second",
            HookEvent::ToolBefore,
            "sh",
            vec!["-c".into(), "input=$(cat); case \"$input\" in *'\"step\":1'*) printf '%s' '{\"payload\":{\"step\":2}}';; *) exit 9;; esac".into()],
        ));
        pipeline.register(CommandHook::new(
            "other-event",
            HookEvent::RunEnd,
            "false",
            Vec::new(),
        ));

        let result = pipeline
            .run(HookEvent::ToolBefore, json!({ "step": 0 }), Path::new("."))
            .await
            .unwrap();
        assert_eq!(result.executed, ["first", "second"]);
        assert_eq!(result.payload, json!({ "step": 2 }));
    }

    #[tokio::test]
    async fn nonzero_exit_is_reported() {
        let mut pipeline = HookPipeline::new();
        pipeline.register(CommandHook::new(
            "broken",
            HookEvent::RunStart,
            "sh",
            vec!["-c".into(), "printf 'reason' >&2; exit 2".into()],
        ));
        let error = pipeline
            .run(HookEvent::RunStart, Value::Null, Path::new("."))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("reason"));
    }
}
