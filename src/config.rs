// src/config.rs — Configuration loading from TOML + env overrides

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub raiderio: RaiderIoConfig,
    pub storage: StorageConfig,
    pub server: ServerConfig,
    pub concurrency: ConcurrencyConfig,
    #[serde(default)]
    pub guilds: Vec<GuildConfig>,
    #[serde(default)]
    pub players: Vec<PlayerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RaiderIoConfig {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_user_agent")]
    pub user_agent: String,
    /// Maximum retries on transient errors / 429s
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Base backoff in milliseconds
    #[serde(default = "default_base_backoff_ms")]
    pub base_backoff_ms: u64,
    /// Maximum backoff in milliseconds
    #[serde(default = "default_max_backoff_ms")]
    pub max_backoff_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_database_path")]
    pub database_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConcurrencyConfig {
    /// Max concurrent Raider.IO requests
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_raiderio: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GuildConfig {
    pub region: String,
    pub realm: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlayerConfig {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub characters: Vec<CharacterRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CharacterRef {
    pub region: String,
    pub realm: String,
    pub name: String,
}

// ─── defaults ───────────────────────────────────────────────────────────────

fn default_base_url() -> String {
    "https://raider.io/api/v1".into()
}
fn default_user_agent() -> String {
    "mplus-tracker/0.1".into()
}
fn default_max_retries() -> u32 {
    5
}
fn default_base_backoff_ms() -> u64 {
    1_000
}
fn default_max_backoff_ms() -> u64 {
    120_000
}
fn default_database_path() -> String {
    "/data/mplus.sqlite".into()
}
fn default_host() -> String {
    "0.0.0.0".into()
}
fn default_port() -> u16 {
    8080
}
fn default_max_concurrent() -> usize {
    3
}

// ─── loader ─────────────────────────────────────────────────────────────────

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {path}"))?;
        let mut cfg: Config =
            toml::from_str(&content).with_context(|| format!("Failed to parse config: {path}"))?;

        // Allow env-var overrides for critical settings
        if let Ok(db) = std::env::var("DATABASE_PATH") {
            cfg.storage.database_path = db;
        }
        if let Ok(host) = std::env::var("SERVER_HOST") {
            cfg.server.host = host;
        }
        if let Ok(port) = std::env::var("SERVER_PORT") {
            cfg.server.port = port.parse().context("Invalid SERVER_PORT")?;
        }
        if let Ok(n) = std::env::var("MAX_CONCURRENT_RAIDERIO") {
            cfg.concurrency.max_concurrent_raiderio = n.parse().context("Invalid MAX_CONCURRENT_RAIDERIO")?;
        }

        Ok(cfg)
    }
}

impl Default for RaiderIoConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            user_agent: default_user_agent(),
            max_retries: default_max_retries(),
            base_backoff_ms: default_base_backoff_ms(),
            max_backoff_ms: default_max_backoff_ms(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            database_path: default_database_path(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
        }
    }
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            max_concurrent_raiderio: default_max_concurrent(),
        }
    }
}

/// Return the path to the config file, checking env var first then a default.
pub fn config_path() -> String {
    std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".into())
}
