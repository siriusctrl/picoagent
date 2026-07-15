use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use regex::Regex;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::artifact::ArtifactRef;

use super::{
    ArtifactLookup, ArtifactSearchMatch, TrajectoryArtifactSearch, TrajectoryArtifactSource,
};

mod rg;

const MAX_SIDECAR_BYTES: u64 = 64 * 1024;

/// Searches the current run's textual artifacts. Directory entries are
/// indexed once per query, while sidecars and artifact contents are opened
/// lazily in newest-message order by `LocalTrajectoryReader`.
pub struct LocalRunArtifactSource {
    workspace: PathBuf,
}

impl LocalRunArtifactSource {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }
}

#[async_trait]
impl TrajectoryArtifactSource for LocalRunArtifactSource {
    async fn begin_search(&self, run_id: &str) -> Result<Box<dyn TrajectoryArtifactSearch>> {
        let artifact_directory = self
            .workspace
            .join(".pico")
            .join("runs")
            .join(safe_component(run_id))
            .join("artifacts");
        let mut directory = match tokio::fs::read_dir(&artifact_directory).await {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Box::new(LocalArtifactSearch::empty(
                    self.workspace.clone(),
                    run_id.to_owned(),
                )));
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read {}", artifact_directory.display()));
            }
        };
        let canonical_directory = tokio::fs::canonicalize(&artifact_directory)
            .await
            .with_context(|| format!("resolve {}", artifact_directory.display()))?;
        let mut sidecars = HashMap::<String, Vec<PathBuf>>::new();
        while let Some(entry) = directory.next_entry().await? {
            let path = entry.path();
            let Some((call_key, _)) = sidecar_key(&path) else {
                continue;
            };
            sidecars.entry(call_key).or_default().push(path);
        }
        for paths in sidecars.values_mut() {
            paths.sort();
        }

        Ok(Box::new(LocalArtifactSearch {
            workspace: self.workspace.clone(),
            run_id: run_id.to_owned(),
            canonical_directory: Some(canonical_directory),
            sidecars,
            loaded: HashMap::new(),
            verified_artifacts: HashSet::new(),
        }))
    }
}

struct LocalArtifactSearch {
    workspace: PathBuf,
    run_id: String,
    canonical_directory: Option<PathBuf>,
    sidecars: HashMap<String, Vec<PathBuf>>,
    loaded: HashMap<PathBuf, ArtifactRef>,
    verified_artifacts: HashSet<(String, String)>,
}

impl LocalArtifactSearch {
    fn empty(workspace: PathBuf, run_id: String) -> Self {
        Self {
            workspace,
            run_id,
            canonical_directory: None,
            sidecars: HashMap::new(),
            loaded: HashMap::new(),
            verified_artifacts: HashSet::new(),
        }
    }

    async fn artifacts_for_lookup(&mut self, lookup: &ArtifactLookup) -> Result<Vec<ArtifactRef>> {
        let key = safe_component(&lookup.call_id);
        let mut sidecars = self.sidecars.get(&key).cloned().unwrap_or_default();
        if let Some(expected_sha256) = &lookup.sha256 {
            if expected_sha256.len() != 64
                || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                bail!("artifact lookup sha256 must contain 64 hexadecimal characters");
            }
            let expected_prefix = &expected_sha256[..12];
            sidecars.retain(|path| {
                sidecar_key(path)
                    .is_some_and(|(_, prefix)| prefix.eq_ignore_ascii_case(expected_prefix))
            });
        } else if sidecars.len() != 1 {
            // A call id is only an execution label, not immutable content
            // identity. A single sidecar remains a useful compatibility path,
            // but multiple candidates are intentionally treated as ambiguous.
            return Ok(Vec::new());
        }

        let mut artifacts = Vec::new();
        for path in sidecars {
            let artifact = self.load_sidecar(&path).await?;
            if artifact.run_id != self.run_id
                || artifact.call_id != lookup.call_id
                || !is_textual(&artifact.media_type)
                || lookup
                    .sha256
                    .as_ref()
                    .is_some_and(|sha256| !artifact.sha256.eq_ignore_ascii_case(sha256))
            {
                continue;
            }
            artifacts.push(artifact);
        }
        Ok(artifacts)
    }

    async fn load_sidecar(&mut self, path: &Path) -> Result<ArtifactRef> {
        if let Some(artifact) = self.loaded.get(path) {
            return Ok(artifact.clone());
        }
        let metadata = tokio::fs::metadata(path)
            .await
            .with_context(|| format!("inspect artifact sidecar {}", path.display()))?;
        if metadata.len() > MAX_SIDECAR_BYTES {
            bail!(
                "artifact sidecar exceeds {MAX_SIDECAR_BYTES} bytes: {}",
                path.display()
            );
        }
        let sidecar = tokio::fs::read(path)
            .await
            .with_context(|| format!("read artifact sidecar {}", path.display()))?;
        let artifact: ArtifactRef = serde_json::from_slice(&sidecar)
            .with_context(|| format!("parse artifact sidecar {}", path.display()))?;
        self.loaded.insert(path.to_owned(), artifact.clone());
        Ok(artifact)
    }

    async fn search_artifact(
        &mut self,
        artifact: &ArtifactRef,
        pattern: &Regex,
    ) -> Result<Option<String>> {
        let Some(canonical_directory) = &self.canonical_directory else {
            return Ok(None);
        };
        let artifact_path = resolve_artifact_path(&self.workspace, &artifact.path);
        let canonical_path = tokio::fs::canonicalize(&artifact_path)
            .await
            .with_context(|| format!("resolve artifact {}", artifact_path.display()))?;
        if !canonical_path.starts_with(canonical_directory) {
            bail!(
                "artifact path escapes current run directory: {}",
                artifact.path
            );
        }
        let metadata = tokio::fs::metadata(&canonical_path)
            .await
            .with_context(|| format!("inspect artifact {}", canonical_path.display()))?;
        if !metadata.is_file() {
            bail!(
                "artifact is not a regular file: {}",
                canonical_path.display()
            );
        }
        if metadata.len() != artifact.bytes {
            bail!(
                "artifact byte length changed for {}: sidecar={}, file={}",
                artifact.path,
                artifact.bytes,
                metadata.len()
            );
        }
        let expected_artifact_id = format!("sha256:{}", artifact.sha256);
        if artifact.artifact_id != expected_artifact_id {
            bail!(
                "artifact identity disagrees with its sha256 for {}",
                artifact.path
            );
        }

        let verification_key = (artifact.artifact_id.clone(), artifact.path.clone());
        if !self.verified_artifacts.contains(&verification_key) {
            let actual_sha256 = hash_file(&canonical_path).await?;
            if actual_sha256 != artifact.sha256 {
                bail!(
                    "artifact content hash changed for {}: sidecar={}, file={}",
                    artifact.path,
                    artifact.sha256,
                    actual_sha256
                );
            }
            self.verified_artifacts.insert(verification_key);
        }

        rg::search_file(&canonical_path, metadata.len(), pattern).await
    }
}

#[async_trait]
impl TrajectoryArtifactSearch for LocalArtifactSearch {
    async fn find(
        &mut self,
        lookups: &[ArtifactLookup],
        pattern: &Regex,
    ) -> Result<Option<ArtifactSearchMatch>> {
        for lookup in lookups {
            for artifact in self.artifacts_for_lookup(lookup).await? {
                if let Some(snippet) = self.search_artifact(&artifact, pattern).await? {
                    return Ok(Some(ArtifactSearchMatch {
                        lookup: lookup.clone(),
                        snippet,
                    }));
                }
            }
        }
        Ok(None)
    }
}

async fn hash_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("open artifact for integrity check {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .with_context(|| format!("hash artifact {}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn resolve_artifact_path(workspace: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_owned()
    } else {
        workspace.join(path)
    }
}

fn sidecar_key(path: &Path) -> Option<(String, String)> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".artifact.json")?;
    let (call_key, hash_prefix) = stem.rsplit_once('-')?;
    (hash_prefix.len() == 12 && hash_prefix.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| (call_key.to_owned(), hash_prefix.to_owned()))
}

fn safe_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_owned()
    } else {
        sanitized
    }
}

fn is_textual(media_type: &str) -> bool {
    media_type.starts_with("text/")
        || media_type.contains("json")
        || media_type.contains("xml")
        || media_type.contains("yaml")
}
