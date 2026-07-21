use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct MemoryPaths {
    pub user: PathBuf,
    pub project: PathBuf,
}

impl MemoryPaths {
    pub fn new(home: impl Into<PathBuf>, workspace: impl Into<PathBuf>) -> Self {
        Self {
            user: home.into().join("memory/user"),
            project: workspace.into().join(".fiasco/memory/project"),
        }
    }

    pub fn runtime_reminder_section(&self) -> String {
        format!(
            "user: {}\nproject: {}",
            self.user.display(),
            self.project.display()
        )
    }
}
