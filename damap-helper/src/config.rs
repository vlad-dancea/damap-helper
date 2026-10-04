//! Local state: which DAMAP instance to talk to, and the login session.
//!
//! Both live in a `.damap-helper` folder inside the research folder. Like git with
//! `.git`, we look for it in the current directory and its parents, and
//! create it in the current directory on first login. Secrets (the refresh
//! token and the AI API key) are kept in their own file, readable only by the
//! user and ignored by git.

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const DIR_NAME: &str = ".damap-helper";
const CONFIG_FILE: &str = "config.toml";
const CREDENTIALS_FILE: &str = "credentials.toml";

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    /// Base URL of the DAMAP backend, e.g. `http://localhost:8085`.
    pub url: String,
    /// The language model that reviews changes; none means no reviews.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai: Option<AiConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    /// Base URL of an OpenAI-compatible API, e.g. `http://localhost:11434/v1`.
    pub url: String,
    /// One of the models it offers, e.g. `qwen3:8b`.
    pub model: String,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Credentials {
    /// The DAMAP login session; logging out removes only this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// The key for the API in [`AiConfig`], if it needs one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

impl Config {
    pub fn load() -> Result<Option<Self>> {
        match find_dir()? {
            Some(dir) => read_toml(&dir.join(CONFIG_FILE)),
            None => Ok(None),
        }
    }

    pub fn save(&self) -> Result<()> {
        write_toml(&ensure_dir()?.join(CONFIG_FILE), self, false)
    }
}

impl Credentials {
    /// The saved secrets; none if nothing is saved yet.
    pub fn load() -> Result<Self> {
        let saved = match find_dir()? {
            Some(dir) => read_toml(&dir.join(CREDENTIALS_FILE))?,
            None => None,
        };
        Ok(saved.unwrap_or_default())
    }

    pub fn save(&self) -> Result<()> {
        write_toml(&ensure_dir()?.join(CREDENTIALS_FILE), self, true)
    }
}

/// The nearest `.damap-helper` folder in the current directory or its parents.
fn find_dir() -> Result<Option<PathBuf>> {
    let cwd = env::current_dir().context("could not determine the current directory")?;
    Ok(cwd
        .ancestors()
        .map(|dir| dir.join(DIR_NAME))
        .find(|candidate| candidate.is_dir()))
}

/// The research folder: the one holding the nearest `.damap-helper`.
pub fn project_root() -> Result<Option<PathBuf>> {
    Ok(find_dir()?.and_then(|dir| dir.parent().map(Path::to_path_buf)))
}

/// Makes the current directory a project folder by creating `.damap-helper` in it.
pub fn create_here() -> Result<PathBuf> {
    let dir = env::current_dir()?.join(DIR_NAME);
    fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    // Keep secrets out of git, should the research folder be a repo.
    fs::write(dir.join(".gitignore"), format!("{CREDENTIALS_FILE}\n"))
        .with_context(|| format!("could not write {}", dir.join(".gitignore").display()))?;
    Ok(dir)
}

/// The nearest `.damap-helper` folder, created in the current directory if there is none.
fn ensure_dir() -> Result<PathBuf> {
    match find_dir()? {
        Some(dir) => Ok(dir),
        None => create_here(),
    }
}

fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("could not read {}", path.display())),
    };
    let value =
        toml::from_str(&text).with_context(|| format!("{} is not valid", path.display()))?;
    Ok(Some(value))
}

fn write_toml<T: Serialize>(path: &Path, value: &T, private: bool) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;

    let mut file = options
        .open(path)
        .with_context(|| format!("could not write {}", path.display()))?;
    file.write_all(toml::to_string(value)?.as_bytes())?;
    Ok(())
}
