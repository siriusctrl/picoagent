use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use tokio::sync::Mutex;

use super::{
    ModelProvider, ModelRequest, ModelResponse,
    common::join_url,
    openai_compatible::OpenAiProtocol,
    openai_oauth_credentials::{
        CODEX_CLIENT_ID, CredentialStore, account_id_from_jwt, default_codex_auth_path,
        is_expiring, jwt_expiry, now_seconds, oauth_error, required_string,
    },
    openai_oauth_device,
    openai_request::responses_body,
    openai_stream,
};
pub use super::{openai_oauth_credentials::OAuthCredentials, openai_oauth_device::DeviceCode};
use crate::events::SharedEventSink;

const DEFAULT_AUTH_BASE_URL: &str = "https://auth.openai.com";
const DEVICE_CALLBACK_PATH: &str = "deviceauth/callback";

pub const DEFAULT_OPENAI_OAUTH_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

#[derive(Debug, Clone)]
pub struct OpenAiOAuthOptions {
    pub base_url: String,
    pub auth_path: PathBuf,
    pub codex_auth_path: Option<PathBuf>,
    pub auth_base_url: String,
}

impl OpenAiOAuthOptions {
    pub fn new(base_url: impl Into<String>, auth_path: impl Into<PathBuf>) -> Self {
        Self {
            base_url: base_url.into(),
            auth_path: auth_path.into(),
            codex_auth_path: default_codex_auth_path(),
            auth_base_url: DEFAULT_AUTH_BASE_URL.to_owned(),
        }
    }
}

#[derive(Clone)]
pub struct OpenAiOAuthProvider {
    client: Client,
    options: OpenAiOAuthOptions,
    refresh_lock: Arc<Mutex<()>>,
}

impl OpenAiOAuthProvider {
    pub fn new(base_url: impl Into<String>, auth_path: impl Into<PathBuf>) -> Self {
        Self::with_options(OpenAiOAuthOptions::new(base_url, auth_path))
    }

    pub fn with_options(options: OpenAiOAuthOptions) -> Self {
        Self {
            client: Client::new(),
            options,
            refresh_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn with_client(options: OpenAiOAuthOptions, client: Client) -> Self {
        Self {
            client,
            options,
            refresh_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn request_device_code(&self) -> Result<DeviceCode> {
        openai_oauth_device::request_device_code(&self.client, &self.options.auth_base_url).await
    }

    pub async fn poll_device_code(&self, device: &DeviceCode) -> Result<OAuthCredentials> {
        let grant = openai_oauth_device::poll_device_code(
            &self.client,
            &self.options.auth_base_url,
            device,
        )
        .await?;
        let credentials = self
            .exchange_device_code(&grant.code, &grant.verifier)
            .await?;
        self.store().save(&credentials).await?;
        Ok(credentials)
    }

    pub async fn credentials(&self) -> Result<OAuthCredentials> {
        let credentials = self.store().load().await?;
        if !is_expiring(&credentials) {
            return Ok(credentials);
        }
        self.refresh_credentials().await
    }

    async fn exchange_device_code(
        &self,
        authorization_code: &str,
        code_verifier: &str,
    ) -> Result<OAuthCredentials> {
        let redirect_uri = join_url(&self.options.auth_base_url, DEVICE_CALLBACK_PATH);
        let form = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "authorization_code")
            .append_pair("code", authorization_code)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("client_id", CODEX_CLIENT_ID)
            .append_pair("code_verifier", code_verifier)
            .finish();
        self.exchange_token(form, None).await
    }

    async fn refresh_credentials(&self) -> Result<OAuthCredentials> {
        let _guard = self.refresh_lock.lock().await;
        let current = self.store().load().await?;
        if !is_expiring(&current) {
            return Ok(current);
        }
        self.refresh(&current).await
    }

    async fn force_refresh(&self, stale_access_token: &str) -> Result<OAuthCredentials> {
        let _guard = self.refresh_lock.lock().await;
        let current = self.store().load().await?;
        if current.access_token != stale_access_token {
            return Ok(current);
        }
        self.refresh(&current).await
    }

    async fn refresh(&self, current: &OAuthCredentials) -> Result<OAuthCredentials> {
        let form = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "refresh_token")
            .append_pair("refresh_token", &current.refresh_token)
            .append_pair("client_id", CODEX_CLIENT_ID)
            .finish();
        let next = self
            .exchange_token(form, current.account_id.clone())
            .await?;
        self.store().save(&next).await?;
        Ok(next)
    }

    async fn exchange_token(
        &self,
        form: String,
        fallback_account_id: Option<String>,
    ) -> Result<OAuthCredentials> {
        let response = self
            .client
            .post(join_url(&self.options.auth_base_url, "oauth/token"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form)
            .send()
            .await
            .context("failed to exchange OpenAI OAuth token")?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .context("invalid OpenAI token response")?;
        if !status.is_success() {
            bail!(
                "OpenAI token exchange failed with HTTP {status}: {}",
                oauth_error(&body)
            );
        }
        let access_token = required_string(&body, "access_token")?;
        let refresh_token = required_string(&body, "refresh_token")?;
        let expires_at = body
            .get("expires_in")
            .and_then(Value::as_u64)
            .map(|seconds| now_seconds().saturating_add(seconds))
            .or_else(|| jwt_expiry(&access_token))
            .context("OpenAI token response omitted a usable expiry")?;
        let account_id = account_id_from_jwt(&access_token).or(fallback_account_id);
        Ok(OAuthCredentials {
            access_token,
            refresh_token,
            expires_at,
            account_id,
        })
    }

    async fn send(
        &self,
        request: &ModelRequest,
        credentials: &OAuthCredentials,
    ) -> Result<reqwest::Response> {
        let mut builder = self
            .client
            .post(join_url(&self.options.base_url, "responses"))
            .bearer_auth(&credentials.access_token)
            .header("originator", "picoagent")
            .json(&responses_body(request, None));
        if let Some(account_id) = &credentials.account_id {
            builder = builder.header("chatgpt-account-id", account_id);
        }
        super::common::send_streaming_request(builder, "OpenAI OAuth", request.stream_idle_timeout)
            .await
    }

    fn store(&self) -> CredentialStore<'_> {
        CredentialStore::new(
            &self.options.auth_path,
            self.options.codex_auth_path.as_deref(),
        )
    }
}

#[async_trait]
impl ModelProvider for OpenAiOAuthProvider {
    fn name(&self) -> &str {
        "openai-oauth"
    }

    fn resume_fingerprint(&self) -> String {
        super::stable_resume_fingerprint(
            self.name(),
            &[
                ("base_url", self.options.base_url.trim_end_matches('/')),
                ("protocol", "responses"),
            ],
        )
    }

    async fn complete(
        &self,
        request: ModelRequest,
        events: SharedEventSink,
    ) -> Result<ModelResponse> {
        let mut credentials = self.credentials().await?;
        let mut response = self.send(&request, &credentials).await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            credentials = self.force_refresh(&credentials.access_token).await?;
            response = self.send(&request, &credentials).await?;
        }
        let response = super::common::ensure_success(response, "OpenAI OAuth").await?;
        openai_stream::complete_response(
            response,
            OpenAiProtocol::Responses,
            &request.run_id,
            events,
            request.stream_idle_timeout,
        )
        .await
    }
}
