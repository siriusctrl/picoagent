use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use tokio::time::sleep;

use super::{
    common::join_url,
    openai_oauth_credentials::{CODEX_CLIENT_ID, now_seconds, oauth_error, required_string},
};

#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub device_auth_id: String,
    pub user_code: String,
    pub verification_url: String,
    pub interval: Duration,
    pub expires_at: u64,
}

pub(crate) struct AuthorizationGrant {
    pub(crate) code: String,
    pub(crate) verifier: String,
}

pub(crate) async fn request_device_code(
    client: &Client,
    auth_base_url: &str,
) -> Result<DeviceCode> {
    let response = client
        .post(join_url(auth_base_url, "api/accounts/deviceauth/usercode"))
        .header("originator", "fiasco")
        .json(&json!({"client_id": CODEX_CLIENT_ID}))
        .send()
        .await
        .context("failed to request OpenAI device code")?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .context("invalid OpenAI device-code response")?;
    if !status.is_success() {
        bail!(
            "OpenAI device-code request failed with HTTP {status}: {}",
            oauth_error(&body)
        );
    }
    let device_auth_id = required_string(&body, "device_auth_id")?;
    let user_code = body
        .get("user_code")
        .or_else(|| body.get("usercode"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .context("OpenAI device-code response omitted `user_code`")?
        .to_owned();
    let interval = body
        .get("interval")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .max(1);
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(15 * 60);
    Ok(DeviceCode {
        device_auth_id,
        user_code,
        verification_url: join_url(auth_base_url, "codex/device"),
        interval: Duration::from_secs(interval),
        expires_at: now_seconds().saturating_add(expires_in),
    })
}

pub(crate) async fn poll_device_code(
    client: &Client,
    auth_base_url: &str,
    device: &DeviceCode,
) -> Result<AuthorizationGrant> {
    while now_seconds() < device.expires_at {
        let response = client
            .post(join_url(auth_base_url, "api/accounts/deviceauth/token"))
            .header("originator", "fiasco")
            .json(&json!({
                "device_auth_id": device.device_auth_id,
                "user_code": device.user_code,
            }))
            .send()
            .await
            .context("failed to poll OpenAI device authorization")?;
        let status = response.status();
        if status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND {
            sleep(device.interval).await;
            continue;
        }
        let body: Value = response
            .json()
            .await
            .context("invalid OpenAI device authorization response")?;
        if !status.is_success() {
            bail!(
                "OpenAI device authorization failed with HTTP {status}: {}",
                oauth_error(&body)
            );
        }
        return Ok(AuthorizationGrant {
            code: required_string(&body, "authorization_code")?,
            verifier: required_string(&body, "code_verifier")?,
        });
    }
    bail!("OpenAI device authorization timed out")
}
