use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskState {
    Queued,
    Running,
    Completed,
    Failed,
    TimedOut,
}

impl BackgroundTaskState {
    pub(super) fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::TimedOut)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTaskRecord {
    pub version: u32,
    pub id: String,
    pub kind: String,
    pub name: String,
    pub state: BackgroundTaskState,
    pub delivered: bool,
    pub result: Option<String>,
    pub error: Option<String>,
    pub child_run_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl BackgroundTaskRecord {
    pub fn model_content(&self) -> String {
        match self.state {
            BackgroundTaskState::Completed => self.result.clone().unwrap_or_default(),
            BackgroundTaskState::Failed => {
                format!(
                    "background task failed: {}",
                    self.error.as_deref().unwrap_or("unknown error")
                )
            }
            BackgroundTaskState::TimedOut => {
                "background task exceeded its execution timeout".to_owned()
            }
            BackgroundTaskState::Queued | BackgroundTaskState::Running => {
                "background task is still running".to_owned()
            }
        }
    }

    pub fn status(&self) -> &'static str {
        match self.state {
            BackgroundTaskState::Queued => "queued",
            BackgroundTaskState::Running => "running",
            BackgroundTaskState::Completed => "completed",
            BackgroundTaskState::Failed => "failed",
            BackgroundTaskState::TimedOut => "timed_out",
        }
    }
}
