// src/db.rs — SQLite persistence layer via sqlx

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use sqlx::{sqlite::SqlitePool, sqlite::SqlitePoolOptions, Row};
use tracing::info;

use crate::models::{Character, Player, Run};

#[derive(Clone)]
pub struct Database {
    pub pool: SqlitePool,
}

impl Database {
    pub async fn connect(database_url: &str) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = std::path::Path::new(database_url)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Cannot create DB directory: {}", parent.display()))?;
        }

        let url = if database_url.starts_with("sqlite:") {
            database_url.to_string()
        } else {
            format!("sqlite:{database_url}?mode=rwc")
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .with_context(|| format!("Cannot open SQLite at {database_url}"))?;

        // Enable WAL mode for better concurrency
        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&pool)
            .await?;
        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&pool)
            .await?;

        info!("Connected to SQLite: {database_url}");
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .context("Database migration failed")?;
        info!("Database migrations applied");
        Ok(())
    }

    // ─── Characters ──────────────────────────────────────────────────────────

    /// Upsert a character; returns its DB id.
    pub async fn upsert_character(
        &self,
        region: &str,
        realm: &str,
        name: &str,
        guild_name: Option<&str>,
        guild_realm: Option<&str>,
    ) -> Result<i64> {
        let now = Utc::now();
        let row = sqlx::query(
            r#"
            INSERT INTO characters (region, realm, name, guild_name, guild_realm, last_seen)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT (region, realm, name) DO UPDATE SET
                guild_name  = COALESCE(excluded.guild_name,  guild_name),
                guild_realm = COALESCE(excluded.guild_realm, guild_realm),
                last_seen   = excluded.last_seen
            RETURNING id
            "#,
        )
        .bind(region)
        .bind(realm)
        .bind(name)
        .bind(guild_name)
        .bind(guild_realm)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .context("upsert_character failed")?;

        Ok(row.get(0))
    }

    pub async fn find_character_id(
        &self,
        region: &str,
        realm: &str,
        name: &str,
    ) -> Result<Option<i64>> {
        let row = sqlx::query("SELECT id FROM characters WHERE region=? AND realm=? AND name=?")
            .bind(region)
            .bind(realm)
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    pub async fn get_character(
        &self,
        region: &str,
        realm: &str,
        name: &str,
    ) -> Result<Option<Character>> {
        let row = sqlx::query_as::<_, Character>(
            "SELECT id, region, realm, name, guild_name, guild_realm, last_seen \
             FROM characters WHERE region=? AND realm=? AND name=?",
        )
        .bind(region)
        .bind(realm)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn list_all_characters(&self) -> Result<Vec<Character>> {
        let rows = sqlx::query_as::<_, Character>(
            "SELECT id, region, realm, name, guild_name, guild_realm, last_seen FROM characters",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    // ─── Players ─────────────────────────────────────────────────────────────

    pub async fn upsert_player(&self, id: &str, label: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO players (id, label) VALUES (?, ?) \
             ON CONFLICT (id) DO UPDATE SET label = excluded.label",
        )
        .bind(id)
        .bind(label)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn link_player_character(&self, player_id: &str, character_id: i64) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO player_characters (player_id, character_id) VALUES (?, ?)",
        )
        .bind(player_id)
        .bind(character_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Upsert a character row (creating it if absent) and immediately link it
    /// to `player_id`.  Safe to call repeatedly — both the character upsert and
    /// the link are idempotent.  This is the canonical way to associate a
    /// config-declared character with its player, because it works regardless of
    /// whether the character row was already created by a guild roster pull.
    pub async fn upsert_and_link_character(
        &self,
        player_id: &str,
        region: &str,
        realm: &str,
        name: &str,
    ) -> Result<()> {
        let char_id = self.upsert_character(region, realm, name, None, None).await?;
        self.link_player_character(player_id, char_id).await?;
        Ok(())
    }

    /// Scan every existing character row whose (region, realm, name) matches an
    /// entry in `chars` and ensure it is linked to `player_id`.  Used during
    /// guild roster pulls to retroactively link characters that were inserted
    /// before the player mapping existed (or before the config was updated).
    ///
    /// Returns the number of newly-created links (0 means all were already
    /// present or no matches were found).
    pub async fn backfill_player_links(
        &self,
        player_id: &str,
        chars: &[(String, String, String)], // (region, realm, name)
    ) -> Result<usize> {
        let mut linked = 0usize;
        for (region, realm, name) in chars {
            if let Some(char_id) = self.find_character_id(region, realm, name).await? {
                // INSERT OR IGNORE: only counts when a row is actually inserted.
                let result = sqlx::query(
                    "INSERT OR IGNORE INTO player_characters (player_id, character_id) VALUES (?, ?)",
                )
                .bind(player_id)
                .bind(char_id)
                .execute(&self.pool)
                .await?;
                if result.rows_affected() > 0 {
                    linked += 1;
                }
            }
        }
        Ok(linked)
    }

    pub async fn get_player(&self, player_id: &str) -> Result<Option<Player>> {
        let row = sqlx::query_as::<_, Player>("SELECT id, label FROM players WHERE id=?")
            .bind(player_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    pub async fn get_player_character_ids(&self, player_id: &str) -> Result<Vec<i64>> {
        let rows = sqlx::query(
            "SELECT character_id FROM player_characters WHERE player_id=?",
        )
        .bind(player_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.get(0)).collect())
    }

    // ─── Runs ─────────────────────────────────────────────────────────────────

    /// Insert a run; returns (inserted: bool).
    pub async fn insert_run(&self, run: &Run) -> Result<bool> {
        let result = sqlx::query(
            r#"
            INSERT INTO runs
                (character_id, dungeon_short, key_level, completed_at, within_time,
                 season, url, source_run_id, hash)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (hash) DO NOTHING
            "#,
        )
        .bind(run.character_id)
        .bind(&run.dungeon_short)
        .bind(run.key_level)
        .bind(run.completed_at)
        .bind(run.within_time as i64)
        .bind(&run.season)
        .bind(&run.url)
        .bind(&run.source_run_id)
        .bind(&run.hash)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Count timed runs for a single character in a time window.
    pub async fn count_runs_for_character(
        &self,
        character_id: i64,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        min_level: i64,
    ) -> Result<i64> {
        let row = sqlx::query(
            "SELECT COUNT(*) FROM runs \
             WHERE character_id=? AND completed_at BETWEEN ? AND ? \
             AND key_level >= ? AND within_time = 1",
        )
        .bind(character_id)
        .bind(from)
        .bind(to)
        .bind(min_level)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get(0))
    }

    /// Count timed runs for a set of character ids in a time window.
    pub async fn count_runs_for_characters(
        &self,
        character_ids: &[i64],
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        min_level: i64,
    ) -> Result<i64> {
        if character_ids.is_empty() {
            return Ok(0);
        }
        let placeholders = character_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT COUNT(*) FROM runs \
             WHERE character_id IN ({placeholders}) \
             AND completed_at BETWEEN ? AND ? \
             AND key_level >= ? AND within_time = 1"
        );
        let mut q = sqlx::query(&sql);
        for id in character_ids {
            q = q.bind(id);
        }
        q = q.bind(from).bind(to).bind(min_level);
        let row = q.fetch_one(&self.pool).await?;
        Ok(row.get(0))
    }

    // ─── Guild helpers ───────────────────────────────────────────────────────

    /// Find characters by guild name + realm.
    pub async fn get_guild_members(
        &self,
        guild_name: &str,
        guild_realm: &str,
    ) -> Result<Vec<Character>> {
        let rows = sqlx::query_as::<_, Character>(
            "SELECT id, region, realm, name, guild_name, guild_realm, last_seen \
             FROM characters WHERE guild_name=? AND guild_realm=?",
        )
        .bind(guild_name)
        .bind(guild_realm)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    // ─── Debug / dump helpers ────────────────────────────────────────────────

    /// List recent runs with character info joined in, for the debug UI.
    pub async fn list_runs(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        min_level: i64,
        limit: i64,
    ) -> Result<Vec<crate::handlers::DebugRun>> {
        let rows = sqlx::query(
            r#"
            SELECT r.id, c.name, c.realm, c.region,
                   r.dungeon_short, r.key_level, r.completed_at,
                   r.within_time, r.url, r.hash
            FROM runs r
            JOIN characters c ON c.id = r.character_id
            WHERE r.completed_at BETWEEN ? AND ?
              AND r.key_level >= ?
              AND r.within_time = 1
            ORDER BY r.completed_at DESC
            LIMIT ?
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(min_level)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|r| crate::handlers::DebugRun {
                id: r.get(0),
                character_name: r.get(1),
                realm: r.get(2),
                region: r.get(3),
                dungeon_short: r.get(4),
                key_level: r.get(5),
                completed_at: r.get::<DateTime<Utc>, _>(6).to_rfc3339(),
                within_time: r.get::<i64, _>(7) != 0,
                url: r.get(8),
                hash: r.get(9),
            })
            .collect())
    }

    /// List runs filtered to guild members (by guild_name + guild_realm).
    pub async fn list_runs_for_guild(
        &self,
        guild_name: &str,
        guild_realm: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        min_level: i64,
        limit: i64,
    ) -> Result<Vec<crate::handlers::DebugRun>> {
        let rows = sqlx::query(
            r#"
            SELECT r.id, c.name, c.realm, c.region,
                   r.dungeon_short, r.key_level, r.completed_at,
                   r.within_time, r.url, r.hash
            FROM runs r
            JOIN characters c ON c.id = r.character_id
            WHERE c.guild_name = ? AND c.guild_realm = ?
              AND r.completed_at BETWEEN ? AND ?
              AND r.key_level >= ?
              AND r.within_time = 1
            ORDER BY r.completed_at DESC
            LIMIT ?
            "#,
        )
        .bind(guild_name)
        .bind(guild_realm)
        .bind(from)
        .bind(to)
        .bind(min_level)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|r| crate::handlers::DebugRun {
                id: r.get(0),
                character_name: r.get(1),
                realm: r.get(2),
                region: r.get(3),
                dungeon_short: r.get(4),
                key_level: r.get(5),
                completed_at: r.get::<DateTime<Utc>, _>(6).to_rfc3339(),
                within_time: r.get::<i64, _>(7) != 0,
                url: r.get(8),
                hash: r.get(9),
            })
            .collect())
    }

    /// Full character list with run counts, for the debug characters view.
    pub async fn list_all_characters_debug(&self) -> Result<Vec<serde_json::Value>> {
        let rows = sqlx::query(
            r#"
            SELECT c.id, c.region, c.realm, c.name,
                   c.guild_name, c.guild_realm,
                   c.last_seen,
                   COUNT(r.id) AS run_count
            FROM characters c
            LEFT JOIN runs r ON r.character_id = c.id AND r.within_time = 1
            GROUP BY c.id
            ORDER BY run_count DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|r| {
                let last_seen: Option<DateTime<Utc>> = r.get(6);
                serde_json::json!({
                    "id": r.get::<i64, _>(0),
                    "region": r.get::<String, _>(1),
                    "realm": r.get::<String, _>(2),
                    "name": r.get::<String, _>(3),
                    "guild_name": r.get::<Option<String>, _>(4),
                    "guild_realm": r.get::<Option<String>, _>(5),
                    "last_seen": last_seen.map(|t| t.to_rfc3339()),
                    "run_count": r.get::<i64, _>(7),
                })
            })
            .collect())
    }

    /// Aggregate stats for the debug dashboard.
    pub async fn get_debug_stats(&self) -> Result<crate::handlers::DebugStats> {
        use crate::handlers::{CharacterRunCount, DebugStats, DungeonCount, KeyLevelCount};

        let total_characters: i64 = sqlx::query("SELECT COUNT(*) FROM characters")
            .fetch_one(&self.pool)
            .await?
            .get(0);

        let total_runs: i64 = sqlx::query("SELECT COUNT(*) FROM runs WHERE within_time = 1")
            .fetch_one(&self.pool)
            .await?
            .get(0);

        let today_start = {
            let now = Utc::now();
            Utc.with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
                .single()
                .unwrap_or(now)
        };
        let runs_today: i64 = sqlx::query(
            "SELECT COUNT(*) FROM runs WHERE completed_at >= ? AND within_time = 1",
        )
        .bind(today_start)
        .fetch_one(&self.pool)
        .await?
        .get(0);

        let week_start = Utc::now() - chrono::Duration::days(7);
        let runs_this_week: i64 = sqlx::query(
            "SELECT COUNT(*) FROM runs WHERE completed_at >= ? AND within_time = 1",
        )
        .bind(week_start)
        .fetch_one(&self.pool)
        .await?
        .get(0);

        let dungeon_rows = sqlx::query(
            "SELECT dungeon_short, COUNT(*) AS cnt FROM runs \
             WHERE within_time = 1 GROUP BY dungeon_short ORDER BY cnt DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        let runs_by_dungeon = dungeon_rows
            .iter()
            .map(|r| DungeonCount { dungeon: r.get(0), count: r.get(1) })
            .collect();

        let kl_rows = sqlx::query(
            "SELECT key_level, COUNT(*) AS cnt FROM runs \
             WHERE within_time = 1 GROUP BY key_level ORDER BY key_level DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        let runs_by_keylevel = kl_rows
            .iter()
            .map(|r| KeyLevelCount { key_level: r.get(0), count: r.get(1) })
            .collect();

        let top_rows = sqlx::query(
            r#"
            SELECT c.name, c.realm, c.region, COUNT(r.id) AS cnt
            FROM runs r
            JOIN characters c ON c.id = r.character_id
            WHERE r.within_time = 1
            GROUP BY c.id
            ORDER BY cnt DESC
            LIMIT 20
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        let top_characters = top_rows
            .iter()
            .map(|r| CharacterRunCount {
                name: r.get(0),
                realm: r.get(1),
                region: r.get(2),
                run_count: r.get(3),
            })
            .collect();

        Ok(DebugStats {
            total_characters,
            total_runs,
            runs_today,
            runs_this_week,
            runs_by_dungeon,
            runs_by_keylevel,
            top_characters,
        })
    }

    /// Check whether a specific run hash already exists in the DB.
    pub async fn run_hash_exists(&self, hash: &str) -> Result<bool> {
        let row = sqlx::query("SELECT 1 FROM runs WHERE hash=? LIMIT 1")
            .bind(hash)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }

    // ─── Players list ────────────────────────────────────────────────────────

    /// Return all players with their linked character ids.
    pub async fn get_all_players(&self) -> Result<Vec<(Player, Vec<i64>)>> {
        let players = sqlx::query_as::<_, Player>("SELECT id, label FROM players ORDER BY label")
            .fetch_all(&self.pool)
            .await?;

        let mut out = Vec::with_capacity(players.len());
        for player in players {
            let char_ids = self.get_player_character_ids(&player.id).await?;
            out.push((player, char_ids));
        }
        Ok(out)
    }


    // ─── Depletions leaderboard ───────────────────────────────────────────────

    /// Top characters by depleted run count (within_time = 0).
    pub async fn get_depletions(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<crate::handlers::DepletionEntry>> {
        let rows = sqlx::query(
            r#"
            SELECT c.name, c.realm, c.region,
                   COUNT(r.id) AS depleted_count
            FROM runs r
            JOIN characters c ON c.id = r.character_id
            WHERE r.within_time = 0
              AND r.completed_at BETWEEN ? AND ?
            GROUP BY c.id
            ORDER BY depleted_count DESC
            LIMIT ?
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|r| crate::handlers::DepletionEntry {
                name:           r.get(0),
                realm:          r.get(1),
                region:         r.get(2),
                depleted_count: r.get(3),
            })
            .collect())
    }

    // ─── Unified leaderboard (players + untracked characters) ────────────────

    /// Returns a leaderboard entry per "person":
    ///   - Logical players: all their alts summed under player.label
    ///   - Characters NOT linked to any player: each appears individually by name
    pub async fn get_leaderboard_all(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        min_level: i64,
    ) -> Result<Vec<crate::handlers::LeaderboardEntry>> {
        // ── 1. Player entries (alts aggregated) ──────────────────────────────
        let players = sqlx::query_as::<_, crate::models::Player>(
            "SELECT id, label FROM players ORDER BY label",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut entries: Vec<crate::handlers::LeaderboardEntry> = Vec::new();

        for player in &players {
            let char_ids = self.get_player_character_ids(&player.id).await?;
            if char_ids.is_empty() { continue; }

            let count = self
                .count_runs_for_characters(&char_ids, from, to, min_level)
                .await?;

            // Skip players who have no runs in this window — they would
            // otherwise clutter the bottom of the leaderboard with 0-key rows.
            if count == 0 { continue; }

            entries.push(crate::handlers::LeaderboardEntry {
                display_name: player.label.clone(),
                player_id:    Some(player.id.clone()),
                count,
                is_player:    true,
            });
        }

        // ── 2. Untracked characters (not linked to any player) ───────────────
        // Collect all character_ids that ARE linked to a player
        let untracked_rows = sqlx::query(
            r#"
            SELECT c.id, c.name, c.realm, c.region,
                   COUNT(r.id) AS run_count
            FROM characters c
            LEFT JOIN runs r
              ON r.character_id = c.id
             AND r.within_time = 1
             AND r.completed_at BETWEEN ? AND ?
             AND r.key_level >= ?
            WHERE c.id NOT IN (
                SELECT character_id FROM player_characters
            )
            GROUP BY c.id
            HAVING run_count > 0
            ORDER BY run_count DESC
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(min_level)
        .fetch_all(&self.pool)
        .await?;

        for row in &untracked_rows {
            entries.push(crate::handlers::LeaderboardEntry {
                display_name: row.get::<String, _>(1),
                player_id:    None,
                count:        row.get::<i64, _>(4),
                is_player:    false,
            });
        }

        // ── 3. Sort combined list by count desc ──────────────────────────────
        entries.sort_by(|a, b| b.count.cmp(&a.count));

        Ok(entries)
    }

        // ─── Character deletion ───────────────────────────────────────────────────

    /// Delete a character and all their runs (cascade). Returns true if a row
    /// was actually deleted, false if the character wasn't in the DB.
    pub async fn delete_character(
        &self,
        region: &str,
        realm: &str,
        name: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "DELETE FROM characters WHERE region=? AND realm=? AND name=?",
        )
        .bind(region)
        .bind(realm)
        .bind(name)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }
}
