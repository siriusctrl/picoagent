use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use tokio::{
    fs::OpenOptions,
    io::AsyncWriteExt,
    sync::{Mutex, MutexGuard},
};

use crate::tools::{ToolContext, paths::display_path};

#[derive(Default)]
pub(super) struct GraphStore {
    lock: Mutex<()>,
}

impl GraphStore {
    pub(super) async fn lock(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().await
    }

    pub(super) fn directory(context: &ToolContext) -> Result<PathBuf> {
        ensure!(
            !context.run_id.is_empty()
                && context
                    .run_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
            "run id is not a safe path component"
        );
        Ok(context
            .workspace
            .join(".fiasco")
            .join("runs")
            .join(&context.run_id)
            .join("graphs"))
    }

    pub(super) fn display_path(context: &ToolContext, path: &Path) -> String {
        display_path(&context.workspace, path)
    }

    pub(super) async fn create_next(
        &self,
        context: &ToolContext,
        content: &[u8],
    ) -> Result<(String, PathBuf)> {
        let _guard = self.lock().await;
        let directory = Self::directory(context)?;
        tokio::fs::create_dir_all(&directory)
            .await
            .with_context(|| format!("create graph directory {}", directory.display()))?;

        for number in 1_u64..=u64::MAX {
            let id = format!("g{number}");
            let path = directory.join(format!("{id}.yaml"));
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .await
            {
                Ok(mut file) => {
                    let result = async {
                        file.write_all(content).await?;
                        file.sync_all().await
                    }
                    .await;
                    if let Err(error) = result {
                        drop(file);
                        let _ = tokio::fs::remove_file(&path).await;
                        return Err(error)
                            .with_context(|| format!("initialize graph {}", path.display()));
                    }
                    return Ok((id, path));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| format!("create graph {}", path.display()));
                }
            }
        }
        bail!("graph id space is exhausted")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::tempdir;

    use super::*;

    fn context(workspace: &Path) -> ToolContext {
        ToolContext {
            run_id: "run-1".to_owned(),
            call_id: "call".to_owned(),
            workspace: workspace.to_owned(),
        }
    }

    #[tokio::test]
    async fn concurrent_initialization_allocates_distinct_short_ids() {
        let workspace = tempdir().unwrap();
        let store = Arc::new(GraphStore::default());
        let first = {
            let store = store.clone();
            let context = context(workspace.path());
            tokio::spawn(async move { store.create_next(&context, b"first").await.unwrap() })
        };
        let second = {
            let store = store.clone();
            let context = context(workspace.path());
            tokio::spawn(async move { store.create_next(&context, b"second").await.unwrap() })
        };

        let mut created = vec![first.await.unwrap(), second.await.unwrap()];
        created.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            created
                .iter()
                .map(|entry| entry.0.as_str())
                .collect::<Vec<_>>(),
            ["g1", "g2"]
        );
        for (_, path) in created {
            assert!(!tokio::fs::read(path).await.unwrap().is_empty());
        }
    }

    #[test]
    fn rejects_an_unsafe_internal_run_id() {
        let workspace = tempdir().unwrap();
        let mut context = context(workspace.path());
        context.run_id = "../other".to_owned();
        assert!(GraphStore::directory(&context).is_err());
    }
}
