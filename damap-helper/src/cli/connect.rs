use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use dialoguer::{Confirm, Input};
use reqwest::blocking::Client;

use crate::auth::{Oidc, Tokens};
use crate::config::{self, Config, Credentials, DamapConfig};
use crate::damap::{Damap, InstanceConfig};

const DEFAULT_URL: &str = "http://localhost:8085";

pub fn setup(url: Option<String>) -> Result<Damap> {
    let http = http_client()?;
    let previous_url = Config::load()?.map(|c| c.damap.frontend_url);
    let (damap, instance) = match url {
        Some(url) => reach(&http, &url)?,
        None => ask_for_damap(&http, previous_url.clone())?,
    };
    let oidc = discover(http, &damap, &instance)?;
    let session = match Credentials::load()?.refresh_token {
        Some(token) if previous_url.as_deref() == Some(damap.url()) => oidc.refresh(&token).ok(),
        _ => None,
    };
    let tokens = match session {
        Some(tokens)
            if Confirm::new()
                .with_prompt("Stay logged in?")
                .default(true)
                .interact()? =>
        {
            tokens
        }
        _ => oidc.login()?,
    };
    config::create_here()?;
    save_session(damap, tokens)
}

pub fn resume() -> Result<Damap> {
    let Some(config) = Config::load()? else {
        bail!("this folder is not set up yet; run `damap-helper setup` first");
    };
    let http = http_client()?;
    let (damap, instance) = reach(&http, &config.damap.frontend_url)?;
    let oidc = discover(http, &damap, &instance)?;
    let tokens = match Credentials::load()?.refresh_token {
        Some(refresh_token) => match oidc.refresh(&refresh_token) {
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

pub(super) fn http_client() -> Result<Client> {
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
    let previous = Config::load()?;
    Config {
        damap: DamapConfig {
            frontend_url: damap.url().to_string(),
            dmp_id: previous.as_ref().and_then(|config| config.damap.dmp_id),
        },
        ai: previous.and_then(|config| config.ai),
    }
    .save()?;
    match tokens.refresh_token {
        Some(refresh_token) => {
            let mut credentials = Credentials::load()?;
            credentials.refresh_token = Some(refresh_token);
            credentials.save()?
        }
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
        anyhow!("{e:#}\nIs DAMAP running? To use a different URL, run `damap-helper setup`.")
    })?;
    Ok((damap, instance))
}

fn ask_for_damap(http: &Client, previous: Option<String>) -> Result<(Damap, InstanceConfig)> {
    let mut default = previous.unwrap_or_else(|| DEFAULT_URL.to_string());
    loop {
        let url: String = Input::new()
            .with_prompt("DAMAP URL (as opened in the browser)")
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
