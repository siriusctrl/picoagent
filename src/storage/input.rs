use std::collections::HashSet;

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::{
    model::{Message, Role},
    storage::RunState,
    trajectory::TrajectoryMessage,
};

use super::RunDirStore;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingInput {
    id: String,
    created_at: DateTime<Utc>,
    message: Message,
}

impl RunDirStore {
    pub(crate) async fn clear_pending_inputs(&self, run_id: &str) -> Result<()> {
        let _guard = self.input_lock.lock().await;
        let path = self.paths(run_id).pending_inputs;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
        }
    }

    pub(crate) async fn enqueue_user_input_with_id(
        &self,
        run_id: &str,
        input_id: String,
        text: String,
    ) -> Result<()> {
        ensure!(
            !text.trim().is_empty(),
            "steering message must not be empty"
        );
        self.enqueue_input_with_id(run_id, input_id, Message::text(Role::User, text))
            .await
    }

    pub(crate) async fn enqueue_runtime_input_with_id(
        &self,
        run_id: &str,
        input_id: String,
        text: String,
    ) -> Result<()> {
        ensure!(!text.trim().is_empty(), "runtime input must not be empty");
        self.enqueue_input_with_id(
            run_id,
            input_id,
            Message {
                role: Role::User,
                content: vec![crate::model::MessageContent::RuntimeReminder { text }],
            },
        )
        .await
    }

    async fn enqueue_input_with_id(
        &self,
        run_id: &str,
        input_id: String,
        message: Message,
    ) -> Result<()> {
        let _guard = self.input_lock.lock().await;
        let paths = self.paths(run_id);
        if tokio::fs::try_exists(&paths.metadata).await? {
            let run = self.load_run(run_id).await?;
            ensure!(
                matches!(
                    run.state,
                    RunState::Queued | RunState::Running | RunState::Open
                ),
                "run `{run_id}` is already {:?}",
                run.state
            );
        }
        tokio::fs::create_dir_all(&paths.directory)
            .await
            .with_context(|| {
                format!(
                    "create pending-input directory {}",
                    paths.directory.display()
                )
            })?;
        let input = PendingInput {
            id: input_id,
            created_at: Utc::now(),
            message,
        };
        let mut line = serde_json::to_vec(&input).context("serialize pending input")?;
        line.push(b'\n');
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&paths.pending_inputs)
            .await
            .with_context(|| format!("open {}", paths.pending_inputs.display()))?;
        file.write_all(&line).await?;
        file.flush().await?;
        file.sync_data().await?;
        Ok(())
    }

    pub(crate) async fn append_pending_inputs(
        &self,
        run_id: &str,
        trajectory: &mut Vec<TrajectoryMessage>,
    ) -> Result<Vec<TrajectoryMessage>> {
        let _guard = self.input_lock.lock().await;
        self.append_pending_inputs_locked(run_id, trajectory).await
    }

    pub(crate) fn pending_input_lock(&self) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        self.input_lock.clone()
    }

    pub(crate) async fn append_pending_inputs_locked(
        &self,
        run_id: &str,
        trajectory: &mut Vec<TrajectoryMessage>,
    ) -> Result<Vec<TrajectoryMessage>> {
        let path = self.paths(run_id).pending_inputs;
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        let inputs = parse_pending_inputs(&bytes, &path)?;

        let mut committed = trajectory
            .iter()
            .filter_map(|message| message.pending_input_id.clone())
            .collect::<HashSet<_>>();
        let mut appended = Vec::new();
        for input in inputs {
            if committed.contains(&input.id) {
                continue;
            }
            let record = self
                .append_pending_input_message(run_id, &input.message, input.id.clone())
                .await?;
            committed.insert(input.id);
            trajectory.push(record.clone());
            appended.push(record);
        }
        Ok(appended)
    }
}

fn parse_pending_inputs(bytes: &[u8], path: &std::path::Path) -> Result<Vec<PendingInput>> {
    let mut inputs = Vec::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        inputs.push(
            serde_json::from_slice::<PendingInput>(line)
                .with_context(|| format!("parse pending input in {}", path.display()))?,
        );
    }
    Ok(inputs)
}
