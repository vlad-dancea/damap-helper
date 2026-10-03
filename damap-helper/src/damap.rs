//! Thin client for the DAMAP REST API.

use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder};
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// The public part of `GET /api/config`. DAMAP serves it without login, and
/// it tells us which Keycloak realm and client to log in with.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceConfig {
    pub issuer: String,
    #[serde(rename = "clientID")]
    pub client_id: String,
    pub scope: String,
    pub app_title: Option<String>,
}

/// One entry of `GET /api/dmps/list`.
#[derive(Debug, Deserialize)]
pub struct DmpListItem {
    pub id: i64,
    pub title: Option<String>,
}

pub struct Damap {
    http: Client,
    url: String,
    access_token: Option<String>,
}

impl Damap {
    pub fn new(http: Client, url: &str) -> Self {
        Self {
            http,
            url: url.trim_end_matches('/').to_string(),
            access_token: None,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn set_access_token(&mut self, token: String) {
        self.access_token = Some(token);
    }

    pub fn instance_config(&self) -> Result<InstanceConfig> {
        let response = self
            .http
            .get(format!("{}/api/config", self.url))
            .send()
            .with_context(|| format!("could not reach {}", self.url))?;
        if !response.status().is_success() {
            bail!(
                "{} answered {} on /api/config; is this a DAMAP backend?",
                self.url,
                response.status()
            );
        }
        response
            .json()
            .with_context(|| format!("{} does not look like a DAMAP backend", self.url))
    }

    /// DMPs the logged-in user owns or is a contributor on.
    pub fn list_dmps(&self) -> Result<Vec<DmpListItem>> {
        self.get_json("/api/dmps/list")
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let request = self.http.get(format!("{}{path}", self.url));
        send_json(self.authorize(request), path)
    }

    fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
        match &self.access_token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }
}

fn send_json<T: DeserializeOwned>(request: RequestBuilder, path: &str) -> Result<T> {
    let response = request
        .send()
        .with_context(|| format!("request to {path} failed"))?;
    match response.status() {
        status if status.is_success() => response
            .json()
            .with_context(|| format!("unexpected response from {path}")),
        StatusCode::UNAUTHORIZED => bail!("DAMAP rejected the login token on {path}"),
        StatusCode::FORBIDDEN => bail!("not allowed to access {path}"),
        status => bail!("{path} failed with {status}"),
    }
}
