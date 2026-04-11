use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub user_name: String,
    pub assistant_name: String,
    pub default_model: String,
    pub global_prompt: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            user_name: "<USERNAME>".into(),
            assistant_name: "assistant".into(),
            default_model: "gpt-5.3-codex".into(),
            global_prompt: "You are a helpful assistant with specialized coding skillsets. Focus on clean code, idiomatic patterns, and efficient logic.".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
    pub config_file: PathBuf,
    pub workspace: PathBuf,
    pub projects: PathBuf,
    pub sessions: PathBuf,
    pub agents: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Self> {
        let home = dirs::home_dir().context("couldn't resolve home directory")?;
        let root = home.join(".salsa");
        Ok(Self {
            config_file: root.join("config.yaml"),
            workspace: root.join("workspace"),
            projects: root.join("projects"),
            sessions: root.join("sessions"),
            agents: root.join("agents"),
            root,
        })
    }
}

pub fn bootstrap() -> Result<(Config, Paths)> {
    let paths = Paths::resolve()?;

    for dir in [
        &paths.root,
        &paths.workspace,
        &paths.projects,
        &paths.sessions,
        &paths.agents,
    ] {
        fs::create_dir_all(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
    }

    let config = if paths.config_file.exists() {
        let text = fs::read_to_string(&paths.config_file)
            .with_context(|| format!("reading {}", paths.config_file.display()))?;
        serde_yaml::from_str(&text)
            .with_context(|| format!("parsing {}", paths.config_file.display()))?
    } else {
        let config = Config::default();
        let text = serde_yaml::to_string(&config).context("serializing default config")?;
        fs::write(&paths.config_file, text)
            .with_context(|| format!("writing {}", paths.config_file.display()))?;
        config
    };

    Ok((config, paths))
}

pub fn tilde_path(p: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rel) = p.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    p.display().to_string()
}
