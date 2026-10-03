//! Connecting to DAMAP: setting up a folder, and resuming its saved session.

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use dialoguer::Input;
use reqwest::blocking::Client;

use crate::auth::{Oidc, Tokens};
use crate::config::{self, Config, Credentials};
use crate::damap::{Damap, InstanceConfig};

const DEFAULT_URL: &str = "http://localhost:8080";

/// Asks for the DAMAP URL and logs in, making the current directory a
/// project folder. Replaces any session saved here before.
pub fn init(url: Option<String>) -> Result<Damap> {
    let http = http_client()?;
    let (damap, instance) = match url {
        Some(url) => reach(&http, &url)?,
        None => ask_for_damap(&http, Config::load()?.map(|c| c.url))?,
    };
    let oidc = discover(http, &damap, &instance)?;
    let tokens = oidc.login()?;
    // Only now, so a failed init leaves no `.damap` behind.
    config::create_here()?;
    save_session(damap, tokens)
}

/// Reuses the project folder's saved DAMAP URL and session, logging in
/// again if the session has expired.
pub fn resume() -> Result<Damap> {
    let Some(config) = Config::load()? else {
        bail!("this folder is not set up yet; run `damap-helper init` first");
    };
    let http = http_client()?;
    let (damap, instance) = reach(&http, &config.url)?;
    let oidc = discover(http, &damap, &instance)?;
    let tokens = match Credentials::load()? {
        Some(credentials) => match oidc.refresh(&credentials.refresh_token) {
            Ok(tokens) => tokens,
            Err(e) => {
                println!("{e}. Please log in again.");
                oidc.login()?
            }
        },
        None => {
            println!("Not logged in.");
            oidc.login()?
        }
    };
    save_session(damap, tokens)
}

fn http_client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!("damap-helper/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
        .build()?)
}

fn discover(http: Client, damap: &Damap, instance: &InstanceConfig) -> Result<Oidc> {
    let title = instance.app_title.as_deref().unwrap_or("DAMAP");
    println!("Connected to {title} at {}", damap.url());
    Oidc::discover(http, &instance.issuer, &instance.client_id, &instance.scope)
}

fn save_session(mut damap: Damap, tokens: Tokens) -> Result<Damap> {
    Config {
        url: damap.url().to_string(),
    }
    .save()?;
    // Keycloak rotates refresh tokens, so store the newest one every time.
    match tokens.refresh_token {
        Some(refresh_token) => Credentials { refresh_token }.save()?,
        None => println!(
            "The login server issued no refresh token; you will have to log in again next time."
        ),
    }
    damap.set_access_token(tokens.access_token);
    Ok(damap)
}

fn reach(http: &Client, url: &str) -> Result<(Damap, InstanceConfig)> {
    let damap = Damap::new(http.clone(), url);
    let instance = damap.instance_config().map_err(|e| {
        anyhow!("{e:#}\nIs DAMAP running? To use a different URL, run `damap-helper init`.")
    })?;
    Ok((damap, instance))
}

/// Asks for a DAMAP URL until one answers like a DAMAP backend.
fn ask_for_damap(http: &Client, previous: Option<String>) -> Result<(Damap, InstanceConfig)> {
    let mut default = previous.unwrap_or_else(|| DEFAULT_URL.to_string());
    loop {
        let url: String = Input::new()
            .with_prompt("DAMAP backend URL")
            .default(default)
            .interact_text()?;
        let damap = Damap::new(http.clone(), &url);
        match damap.instance_config() {
            Ok(instance) => return Ok((damap, instance)),
            Err(e) => println!("{e:#}. Try again, or press Ctrl+C to stop."),
        }
        default = url;
    }
}
