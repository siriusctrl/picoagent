use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, ensure};
use tokio::io::AsyncWriteExt;

use super::BackgroundTaskRecord;

#[derive(Debug, Clone)]
pub(in crate::agent::task) struct TaskRecordStore {
    directory: PathBuf,
}

impl TaskRecordStore {
    pub(in crate::agent::task) fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub(in crate::agent::task) async fn load(
        &self,
    ) -> Result<BTreeMap<String, BackgroundTaskRecord>> {
        let mut entries = match tokio::fs::read_dir(&self.directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read task directory {}", self.directory.display()));
            }
        };
        let mut records = BTreeMap::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = tokio::fs::read(&path)
                .await
                .with_context(|| format!("read task record {}", path.display()))?;
            let record: BackgroundTaskRecord = serde_json::from_slice(&bytes)
                .with_context(|| format!("parse task record {}", path.display()))?;
            record.validate()?;
            let file_id = path.file_stem().and_then(|value| value.to_str());
            ensure!(
                file_id == Some(record.id.as_str()),
                "task record id `{}` does not match file {}",
                record.id,
                path.display()
            );
            ensure!(
                records.insert(record.id.clone(), record).is_none(),
                "duplicate task record"
            );
        }
        Ok(records)
    }

    pub(in crate::agent::task) async fn write(&self, record: &BackgroundTaskRecord) -> Result<()> {
        record.validate()?;
        tokio::fs::create_dir_all(&self.directory)
            .await
            .with_context(|| format!("create task directory {}", self.directory.display()))?;
        if let Some(parent) = self.directory.parent() {
            sync_directory(parent).await?;
        }
        let path = self.directory.join(format!("{}.json", record.id));
        let temporary = self.directory.join(format!("{}.json.tmp", record.id));
        let bytes = serde_json::to_vec_pretty(record).context("serialize task record")?;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .await
            .with_context(|| format!("open task record {}", temporary.display()))?;
        file.write_all(&bytes).await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temporary, &path)
            .await
            .with_context(|| format!("replace task record {}", path.display()))?;
        sync_directory(&self.directory).await
    }
}

#[cfg(unix)]
async fn sync_directory(path: &std::path::Path) -> Result<()> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        std::fs::File::open(&path)
            .with_context(|| format!("open task directory {} for sync", path.display()))?
            .sync_all()
            .with_context(|| format!("sync task directory {}", path.display()))
    })
    .await
    .context("join task directory sync")?
}

#[cfg(not(unix))]
async fn sync_directory(_path: &std::path::Path) -> Result<()> {
    Ok(())
}
