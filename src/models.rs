// src/models.rs — domain types used across layers

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

// ─── DB row types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Character {
    pub id: i64,
    pub region: String,
    pub realm: String,
    pub name: String,
    pub guild_name: Option<String>,
    pub guild_realm: Option<String>,
    pub last_seen: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Player {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct Run {
    pub character_id: i64,
    pub dungeon_short: String,
    pub key_level: i64,
    pub completed_at: DateTime<Utc>,
    pub within_time: bool,
    pub season: Option<String>,
    pub url: Option<String>,
    pub source_run_id: Option<String>,
    pub hash: String,
}

// ─── Raider.IO API response shapes ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RioCharacterProfile {
    pub name: String,
    pub realm: String,
    pub region: String,
    pub mythic_plus_recent_runs: Option<Vec<RioRun>>,
}

#[derive(Debug, Deserialize)]
pub struct RioRun {
    pub dungeon: String,
    pub short_name: String,
    pub mythic_level: i64,
    pub completed_at: String,
    pub num_keystone_upgrades: i64,
    pub url: Option<String>,
    pub id: Option<serde_json::Value>, // may be int or string in API
}

#[derive(Debug, Deserialize)]
pub struct RioGuildProfile {
    pub name: String,
    pub realm: String,
    pub region: String,
    pub members: Option<Vec<RioGuildMember>>,
}

#[derive(Debug, Deserialize)]
pub struct RioGuildMember {
    pub character: RioMemberCharacter,
    pub rank: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct RioMemberCharacter {
    pub name: String,
    pub realm: String,
    pub region: Option<String>,
}

// ─── API response DTOs ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct CharacterSummary {
    pub region: String,
    pub realm: String,
    pub name: String,
    pub guild_name: Option<String>,
}

impl From<&Character> for CharacterSummary {
    fn from(c: &Character) -> Self {
        Self {
            region: c.region.clone(),
            realm: c.realm.clone(),
            name: c.name.clone(),
            guild_name: c.guild_name.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
    pub code: u16,
}

impl ApiError {
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self { error: msg.into(), code: 404 }
    }
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self { error: msg.into(), code: 400 }
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self { error: msg.into(), code: 500 }
    }
}
