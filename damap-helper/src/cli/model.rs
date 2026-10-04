//! Choosing the language model that checks changes against the DMP.

use anyhow::{Context, Result, bail};
use clap::Args;
use dialoguer::{Confirm, FuzzySelect, Input, Password};
use reqwest::blocking::Client;

use super::connect::http_client;
use crate::agent;
use crate::config::{AiConfig, AiKey, Config};

/// Ollama's OpenAI-compatible API, the obvious choice for a local model.
const DEFAULT_URL: &str = "http://localhost:11434/v1";

#[derive(Args)]
pub struct ModelArgs {
    /// Base URL of an OpenAI-compatible API for the model; asked for if not given.
    #[arg(long, env = "DAMAP_HELPER_AI_URL")]
    pub ai_url: Option<String>,
    /// API key for it, if it needs one; asked for along with the URL.
    #[arg(long, env = "DAMAP_HELPER_API_KEY", hide_env_values = true)]
    pub api_key: Option<String>,
    /// The model to use; chosen from the ones the API offers if not given.
    #[arg(long, env = "DAMAP_HELPER_MODEL")]
    pub model: Option<String>,
}

/// Sets up the model for this project folder, or leaves it without one if
/// the user declines.
pub fn setup(args: ModelArgs) -> Result<()> {
    let mut config = Config::load()?.context("this folder is not set up yet")?;
    let given = args.ai_url.is_some() || args.model.is_some();
    if !given
        && !Confirm::new()
            .with_prompt("Use an AI model to check changes against the DMP?")
            .default(true)
            .interact()?
    {
        return Ok(());
    }

    let http = http_client()?;
    let previous = config.ai.take();
    let (url, api_key, models) = match args.ai_url {
        Some(url) => {
            let models = agent::list_models(&http, &url, args.api_key.as_deref())?;
            (url, args.api_key, models)
        }
        None => ask_for_api(
            &http,
            previous.as_ref().map(|ai| ai.url.clone()),
            args.api_key,
        )?,
    };
    let model = choose_model(&url, models, args.model, previous.map(|ai| ai.model))?;

    match &api_key {
        Some(key) => AiKey {
            api_key: key.clone(),
        }
        .save()?,
        None => AiKey::delete()?,
    }
    println!("Changes will be checked with {model}.");
    config.ai = Some(AiConfig { url, model });
    config.save()?;
    Ok(())
}

/// Asks for the API's URL and key until it lists its models.
fn ask_for_api(
    http: &Client,
    previous: Option<String>,
    given_key: Option<String>,
) -> Result<(String, Option<String>, Vec<String>)> {
    let mut default = previous.unwrap_or_else(|| DEFAULT_URL.to_string());
    loop {
        let url: String = Input::new()
            .with_prompt("AI API URL (OpenAI-compatible)")
            .default(default)
            .interact_text()?;
        let api_key = match &given_key {
            Some(key) => Some(key.clone()),
            None => {
                let key = Password::new()
                    .with_prompt("API key (empty for none)")
                    .allow_empty_password(true)
                    .interact()?;
                Some(key).filter(|key| !key.is_empty())
            }
        };
        match agent::list_models(http, &url, api_key.as_deref()) {
            Ok(models) => return Ok((url, api_key, models)),
            Err(e) => println!("{e:#}. Try again, or press Ctrl+C to stop."),
        }
        default = url;
    }
}

fn choose_model(
    url: &str,
    models: Vec<String>,
    given: Option<String>,
    previous: Option<String>,
) -> Result<String> {
    if let Some(model) = given {
        if !models.contains(&model) {
            // Some servers list only part of what they serve, so let it be.
            println!("Note: {url} does not list {model}; using it anyway.");
        }
        return Ok(model);
    }
    if models.is_empty() {
        bail!("{url} offers no models; install one, or pass --model");
    }
    let default = previous
        .and_then(|previous| models.iter().position(|model| *model == previous))
        .unwrap_or(0);
    let index = FuzzySelect::new()
        .with_prompt("Model (type to filter)")
        .items(&models)
        .default(default)
        .interact()?;
    Ok(models[index].clone())
}
