//! Local state: which DAMAP instance to talk to, and the login session.
//!
//! Both live in a `.damap` folder inside the research folder. Like git with
//! `.git`, we look for it in the current directory and its parents, and
//! create it in the current directory on first login. The refresh token is
//! kept in its own file, readable only by the user and ignored by git.

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const DIR_NAME: &str = ".damap";
const CONFIG_FILE: &str = "config.toml";
const CREDENTIALS_FILE: &str = "credentials.toml";

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    /// Base URL of the DAMAP backend, e.g. `http://localhost:8080`.
    pub url: String,
}

#[derive(Serialize, Deserialize)]
pub struct Credentials {
    pub refresh_token: String,
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
    pub fn load() -> Result<Option<Self>> {
        match find_dir()? {
            Some(dir) => read_toml(&dir.join(CREDENTIALS_FILE)),
            None => Ok(None),
        }
    }

    pub fn save(&self) -> Result<()> {
        write_toml(&ensure_dir()?.join(CREDENTIALS_FILE), self, true)
    }

    /// Returns whether there was a session to remove.
    pub fn delete() -> Result<bool> {
        let Some(dir) = find_dir()? else {
            return Ok(false);
        };
        match fs::remove_file(dir.join(CREDENTIALS_FILE)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).context("could not remove saved credentials"),
        }
    }
}

/// The nearest `.damap` folder in the current directory or its parents.
fn find_dir() -> Result<Option<PathBuf>> {
    let cwd = env::current_dir().context("could not determine the current directory")?;
    Ok(cwd
        .ancestors()
        .map(|dir| dir.join(DIR_NAME))
        .find(|candidate| candidate.is_dir()))
}

/// The research folder: the one holding the nearest `.damap`.
pub fn project_root() -> Result<Option<PathBuf>> {
    Ok(find_dir()?.and_then(|dir| dir.parent().map(Path::to_path_buf)))
}

/// Makes the current directory a project folder by creating `.damap` in it.
pub fn create_here() -> Result<PathBuf> {
    let dir = env::current_dir()?.join(DIR_NAME);
    fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    // Keep the login token out of git, should the research folder be a repo.
    fs::write(dir.join(".gitignore"), format!("{CREDENTIALS_FILE}\n"))
        .with_context(|| format!("could not write {}", dir.join(".gitignore").display()))?;
    Ok(dir)
}

/// The nearest `.damap` folder, created in the current directory if there is none.
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
