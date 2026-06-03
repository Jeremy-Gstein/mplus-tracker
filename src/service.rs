// src/service.rs — business logic layer

use anyhow::Result;
use chrono::DateTime;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{error, info, warn};

use crate::{
    config::Config,
    db::Database,
    hash::run_hash,
    models::{Character, CharacterSummary, Run},
    raiderio::RaiderIoClient,
};

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub rio: RaiderIoClient,
    pub config: Arc<Config>,
    /// Semaphore to limit concurrent Raider.IO requests
    pub semaphore: Arc<Semaphore>,
}

impl AppState {
    pub fn new(db: Database, rio: RaiderIoClient, config: Config) -> Self {
        let max = config.concurrency.max_concurrent_raiderio;
        Self {
            db,
            rio,
            semaphore: Arc::new(Semaphore::new(max)),
            config: Arc::new(config),
        }
    }
}

// ─── Guild update ─────────────────────────────────────────────────────────────

pub struct GuildUpdateResult {
    pub region: String,
    pub realm: String,
    pub name: String,
    pub members_added: usize,
    pub members_updated: usize,
    pub members: Vec<CharacterSummary>,
}

pub async fn update_guild(
    state: &AppState,
    region: &str,
    realm: &str,
    name: &str,
) -> Result<GuildUpdateResult> {
    let _permit = state.semaphore.acquire().await?;
    let profile = state.rio.get_guild_profile(region, realm, name).await?;

    let members_raw = profile.members.unwrap_or_default();
    let mut added = 0usize;
    let mut updated = 0usize;
    let mut summaries = Vec::new();

    for member in &members_raw {
        let char_region = member
            .character
            .region
            .as_deref()
            .unwrap_or(region);

        let existing = state
            .db
            .find_character_id(char_region, &member.character.realm, &member.character.name)
            .await?;

        let _id = state
            .db
            .upsert_character(
                char_region,
                &member.character.realm,
                &member.character.name,
                Some(name),
                Some(realm),
            )
            .await?;

        if existing.is_some() {
            updated += 1;
        } else {
            added += 1;
        }

        summaries.push(CharacterSummary {
            region: char_region.to_string(),
            realm: member.character.realm.clone(),
            name: member.character.name.clone(),
            guild_name: Some(name.to_string()),
        });
    }

    info!(
        guild = name,
        realm,
        region,
        added,
        updated,
        "Guild updated"
    );

    Ok(GuildUpdateResult {
        region: region.to_string(),
        realm: realm.to_string(),
        name: name.to_string(),
        members_added: added,
        members_updated: updated,
        members: summaries,
    })
}

// ─── Character update ─────────────────────────────────────────────────────────

pub struct CharacterUpdateResult {
    pub character: CharacterSummary,
    pub runs_inserted: usize,
    pub runs_ignored: usize,
}

pub async fn update_character(
    state: &AppState,
    region: &str,
    realm: &str,
    name: &str,
) -> Result<CharacterUpdateResult> {
    let _permit = state.semaphore.acquire().await?;

    let profile = state
        .rio
        .get_character_profile(region, realm, name)
        .await?;

    // Upsert character
    let char_id = state
        .db
        .upsert_character(region, realm, name, None, None)
        .await?;

    let mut inserted = 0usize;
    let mut ignored = 0usize;

    for rio_run in profile.mythic_plus_recent_runs.unwrap_or_default() {
        let source_run_id = match &rio_run.id {
            Some(serde_json::Value::Number(n)) => Some(n.to_string()),
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            _ => None,
        };

        let hash = run_hash(
            region,
            realm,
            name,
            &rio_run.short_name,
            rio_run.mythic_level,
            &rio_run.completed_at,
            source_run_id.as_deref(),
        );

        let completed_at = match DateTime::parse_from_rfc3339(&rio_run.completed_at) {
            Ok(dt) => dt.with_timezone(&chrono::Utc),
            Err(e) => {
                warn!(
                    run = ?rio_run.short_name,
                    ts = rio_run.completed_at,
                    error = %e,
                    "Could not parse completed_at, skipping run"
                );
                continue;
            }
        };

        let run = Run {
            character_id: char_id,
            dungeon_short: rio_run.short_name.clone(),
            key_level: rio_run.mythic_level,
            completed_at,
            within_time: rio_run.num_keystone_upgrades > 0,
            season: None,
            url: rio_run.url.clone(),
            source_run_id: source_run_id.clone(),
            hash,
        };

        if state.db.insert_run(&run).await? {
            inserted += 1;
        } else {
            ignored += 1;
        }
    }

    info!(
        name,
        realm,
        region,
        inserted,
        ignored,
        "Character updated"
    );

    let char = CharacterSummary {
        region: region.to_string(),
        realm: realm.to_string(),
        name: name.to_string(),
        guild_name: None,
    };

    Ok(CharacterUpdateResult {
        character: char,
        runs_inserted: inserted,
        runs_ignored: ignored,
    })
}

// ─── Update all ───────────────────────────────────────────────────────────────

pub struct UpdateAllResult {
    pub total_characters: usize,
    pub updated_ok: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

pub async fn update_all_characters(state: &AppState) -> Result<UpdateAllResult> {
    let chars: Vec<Character> = state.db.list_all_characters().await?;
    let total = chars.len();
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut errors = Vec::new();

    // Process concurrently, bounded by the semaphore inside update_character
    let mut handles = Vec::new();
    for c in chars {
        let state = state.clone();
        let h = tokio::spawn(async move {
            let r = update_character(&state, &c.region, &c.realm, &c.name).await;
            (c.name.clone(), r)
        });
        handles.push(h);
    }

    for handle in handles {
        match handle.await {
            Ok((_name, Ok(_))) => ok += 1,
            Ok((name, Err(e))) => {
                failed += 1;
                let msg = format!("{name}: {e}");
                error!(error = msg, "update_character failed");
                errors.push(msg);
            }
            Err(e) => {
                failed += 1;
                errors.push(format!("Task panic: {e}"));
            }
        }
    }

    info!(total, ok, failed, "update_all completed");

    Ok(UpdateAllResult {
        total_characters: total,
        updated_ok: ok,
        failed,
        errors,
    })
}
