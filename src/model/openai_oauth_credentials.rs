use std::{
    fmt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const REFRESH_SKEW_SECONDS: u64 = 60;

#[derive(Clone, Serialize, Deserialize)]
pub struct OAuthCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

impl fmt::Debug for OAuthCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthCredentials")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("account_id", &self.account_id)
            .finish()
    }
}

pub(crate) struct CredentialStore<'a> {
    auth_path: &'a Path,
    codex_auth_path: Option<&'a Path>,
}

impl<'a> CredentialStore<'a> {
    pub(crate) fn new(auth_path: &'a Path, codex_auth_path: Option<&'a Path>) -> Self {
        Self {
            auth_path,
            codex_auth_path,
        }
    }

    pub(crate) async fn load(&self) -> Result<OAuthCredentials> {
        match read_auth_file(self.auth_path).await {
            Ok(Some(credentials)) => return Ok(credentials),
            Ok(None) => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to load Fiasco auth file `{}`",
                        self.auth_path.display()
                    )
                });
            }
        }
        if let Some(path) = self.codex_auth_path
            && let Some(credentials) = read_auth_file(path)
                .await
                .with_context(|| format!("failed to import Codex auth file `{}`", path.display()))?
        {
            return Ok(credentials);
        }
        bail!("OpenAI OAuth credentials not found; run device login or provide a Codex auth file")
    }

    pub(crate) async fn save(&self, credentials: &OAuthCredentials) -> Result<()> {
        if let Some(parent) = self.auth_path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("failed to create auth directory `{}`", parent.display())
            })?;
        }
        let temporary = self.auth_path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(credentials)?;
        tokio::fs::write(&temporary, data)
            .await
            .with_context(|| format!("failed to write auth file `{}`", temporary.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
                .await
                .with_context(|| format!("failed to secure auth file `{}`", temporary.display()))?;
        }
        tokio::fs::rename(&temporary, self.auth_path)
            .await
            .with_context(|| {
                format!("failed to install auth file `{}`", self.auth_path.display())
            })?;
        Ok(())
    }
}

pub(crate) fn default_codex_auth_path() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .map(|home| home.join("auth.json"))
}

pub(crate) fn required_string(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("response omitted `{key}`"))
}

pub(crate) fn oauth_error(value: &Value) -> String {
    value
        .get("error_description")
        .or_else(|| value.pointer("/error/message"))
        .or_else(|| value.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("unknown OAuth error")
        .to_owned()
}

pub(crate) fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn is_expiring(credentials: &OAuthCredentials) -> bool {
    credentials.expires_at <= now_seconds().saturating_add(REFRESH_SKEW_SECONDS)
}

pub(crate) fn jwt_expiry(token: &str) -> Option<u64> {
    jwt_payload(token)?.get("exp")?.as_u64()
}

pub(crate) fn account_id_from_jwt(token: &str) -> Option<String> {
    let payload = jwt_payload(token)?;
    payload
        .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
        .or_else(|| payload.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

async fn read_auth_file(path: &Path) -> Result<Option<OAuthCredentials>> {
    let data = match tokio::fs::read(path).await {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let value: Value = serde_json::from_slice(&data).context("auth file is not valid JSON")?;
    if let Ok(credentials) = serde_json::from_value::<OAuthCredentials>(value.clone()) {
        return Ok(Some(credentials));
    }
    let Some(tokens) = value.get("tokens") else {
        return Ok(None);
    };
    let access_token = required_string(tokens, "access_token")?;
    let refresh_token = required_string(tokens, "refresh_token")?;
    let expires_at =
        jwt_expiry(&access_token).unwrap_or_else(|| now_seconds().saturating_add(3600));
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| account_id_from_jwt(&access_token));
    Ok(Some(OAuthCredentials {
        access_token,
        refresh_token,
        expires_at,
        account_id,
    }))
}

fn jwt_payload(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_debug_redacts_tokens() {
        let credentials = OAuthCredentials {
            access_token: "access-secret".to_owned(),
            refresh_token: "refresh-secret".to_owned(),
            expires_at: 42,
            account_id: Some("account".to_owned()),
        };

        let debug = format!("{credentials:?}");
        assert!(!debug.contains("access-secret"));
        assert!(!debug.contains("refresh-secret"));
        assert!(debug.contains("[REDACTED]"));
    }
}
