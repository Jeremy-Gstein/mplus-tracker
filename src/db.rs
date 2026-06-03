// src/db.rs — SQLite persistence layer via sqlx

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
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

    /// Count runs for a single character in a time window.
    pub async fn count_runs_for_character(
        &self,
        character_id: i64,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        min_level: i64,
    ) -> Result<i64> {
        let row = sqlx::query(
            "SELECT COUNT(*) FROM runs \
             WHERE character_id=? AND completed_at BETWEEN ? AND ? AND key_level >= ?",
        )
        .bind(character_id)
        .bind(from)
        .bind(to)
        .bind(min_level)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get(0))
    }

    /// Count runs for a set of character ids in a time window.
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
        // Build a dynamic IN clause
        let placeholders = character_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT COUNT(*) FROM runs \
             WHERE character_id IN ({placeholders}) \
             AND completed_at BETWEEN ? AND ? AND key_level >= ?"
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
}
