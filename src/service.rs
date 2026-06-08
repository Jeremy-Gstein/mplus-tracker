// src/service.rs — business logic layer

use anyhow::Result;
use chrono::{DateTime, Months, Utc};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};

use crate::{
    config::Config,
    db::Database,
    hash::run_hash,
    models::{Character, CharacterSummary, Run},
    raiderio::RaiderIoClient,
};

/// How old a Raider.IO `last_crawled_at` timestamp must be before we consider
/// the character's key data stale and worth re-fetching.  Characters crawled
/// *within* this window are skipped to avoid burning rate-limit budget.
const STALE_AFTER_MONTHS: u32 = 3;

/// Returns `true` when a `last_crawled_at` string from Raider.IO indicates the
/// character was crawled recently enough that we can skip the key update.
///
/// "Recently enough" = crawled within the last [`STALE_AFTER_MONTHS`] months.
/// If the timestamp is missing or unparseable we treat it as stale (i.e. we
/// *do* update) so we never silently skip someone due to bad data.
fn crawled_recently(last_crawled_at: Option<&str>) -> bool {
    let Some(raw) = last_crawled_at else {
        return false; // no timestamp → assume stale, update
    };
    let Ok(crawled) = DateTime::parse_from_rfc3339(raw) else {
        warn!(ts = raw, "Could not parse last_crawled_at; treating as stale");
        return false;
    };
    let crawled_utc = crawled.with_timezone(&Utc);
    let threshold = Utc::now()
        .checked_sub_months(Months::new(STALE_AFTER_MONTHS))
        .unwrap_or(Utc::now());
    crawled_utc >= threshold
}

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
    /// Members whose Raider.IO `last_crawled_at` is recent enough that we
    /// skipped queuing a key-data update for them.
    pub members_skipped_fresh: usize,
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
    let mut skipped_fresh = 0usize;
    let mut summaries = Vec::new();

    for member in &members_raw {
        let char_region = member
            .character
            .region
            .as_deref()
            .unwrap_or(region);

        // ── Stale-crawl gate ──────────────────────────────────────────────
        // If Raider.IO crawled this character's profile recently (within the
        // last STALE_AFTER_MONTHS months) their key data is up-to-date and
        // we can skip queuing an individual character update.  We still upsert
        // them into the DB so roster membership stays current.
        if crawled_recently(member.character.last_crawled_at.as_deref()) {
            info!(
                name = %member.character.name,
                realm = %member.character.realm,
                last_crawled = ?member.character.last_crawled_at,
                "Skipping key update — crawled recently"
            );
            skipped_fresh += 1;
            // Still upsert into DB so we track roster membership.
            let _ = state
                .db
                .upsert_character(
                    char_region,
                    &member.character.realm,
                    &member.character.name,
                    Some(name),
                    Some(realm),
                )
                .await?;
            summaries.push(CharacterSummary {
                region: char_region.to_string(),
                realm: member.character.realm.clone(),
                name: member.character.name.clone(),
                guild_name: Some(name.to_string()),
            });
            continue;
        }

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
        skipped_fresh,
        "Guild updated"
    );

    Ok(GuildUpdateResult {
        region: region.to_string(),
        realm: realm.to_string(),
        name: name.to_string(),
        members_added: added,
        members_updated: updated,
        members_skipped_fresh: skipped_fresh,
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
        // Only track timed (non-depleted) runs
        if rio_run.num_keystone_upgrades == 0 {
            ignored += 1;
            debug!(
                dungeon = %rio_run.short_name,
                level   = rio_run.mythic_level,
                "Skipping depleted run (num_keystone_upgrades=0)"
            );
            continue;
        }

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
            within_time: true, // guaranteed by the num_keystone_upgrades > 0 gate above
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
    pub pruned: usize,
    pub errors: Vec<String>,
}

pub async fn update_all_characters(state: &AppState) -> Result<UpdateAllResult> {
    let chars: Vec<Character> = state.db.list_all_characters().await?;
    let total = chars.len();
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut pruned = 0usize;
    let mut errors = Vec::new();

    // Each task carries (region, realm, name) so we can prune on 400.
    let mut handles = Vec::new();
    for c in chars {
        let state = state.clone();
        let h = tokio::spawn(async move {
            let r = update_character(&state, &c.region, &c.realm, &c.name).await;
            (c.region.clone(), c.realm.clone(), c.name.clone(), r)
        });
        handles.push(h);
    }

    for handle in handles {
        match handle.await {
            Ok((_region, _realm, _name, Ok(_))) => ok += 1,
            Ok((region, realm, name, Err(e))) => {
                let msg = e.to_string();
                // Raider.IO 400 = character doesn't exist (deleted, renamed, or
                // bogus NPC entry from guild roster). Auto-prune from DB so it
                // stops polluting future update_all runs.
                if msg.contains("400 Bad Request")
                    && msg.contains("Could not find requested character")
                {
                    warn!(
                        character = %name,
                        realm = %realm,
                        region = %region,
                        "Auto-pruning character not found on Raider.IO (400)"
                    );
                    if let Err(db_err) = state.db.delete_character(&region, &realm, &name).await {
                        error!(error = %db_err, "Failed to prune character from DB");
                    }
                    pruned += 1;
                } else {
                    failed += 1;
                    error!(error = %msg, "update_character failed");
                    errors.push(format!("{name}: {msg}"));
                }
            }
            Err(e) => {
                failed += 1;
                errors.push(format!("Task panic: {e}"));
            }
        }
    }

    info!(total, ok, failed, pruned, "update_all completed");

    Ok(UpdateAllResult {
        total_characters: total,
        updated_ok: ok,
        failed,
        pruned,
        errors,
    })
}
