// src/raiderio.rs — Raider.IO API client with retry / 429 backoff

use anyhow::{anyhow, Result};
use rand::Rng;
use reqwest::{Client, Response, StatusCode};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, warn};

use crate::config::RaiderIoConfig;
use crate::models::{RioCharacterProfile, RioGuildProfile};

#[derive(Clone)]
pub struct RaiderIoClient {
    client: Client,
    cfg: RaiderIoConfig,
}

impl RaiderIoClient {
    pub fn new(cfg: RaiderIoConfig) -> Result<Self> {
        let client = Client::builder()
            .user_agent(&cfg.user_agent)
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { client, cfg })
    }

    // ─── Public API ──────────────────────────────────────────────────────────

    pub async fn get_character_profile(
        &self,
        region: &str,
        realm: &str,
        name: &str,
    ) -> Result<RioCharacterProfile> {
        let url = format!("{}/characters/profile", self.cfg.base_url);
        let params = [
            ("region", region),
            ("realm", realm),
            ("name", name),
            ("fields", "mythic_plus_recent_runs"),
        ];
        let resp = self.get_with_retry(&url, &params).await?;
        let profile: RioCharacterProfile = resp.json().await?;
        Ok(profile)
    }

    pub async fn get_guild_profile(
        &self,
        region: &str,
        realm: &str,
        name: &str,
    ) -> Result<RioGuildProfile> {
        let url = format!("{}/guilds/profile", self.cfg.base_url);
        let params = [
            ("region", region),
            ("realm", realm),
            ("name", name),
            ("fields", "members"),
        ];
        let resp = self.get_with_retry(&url, &params).await?;
        let profile: RioGuildProfile = resp.json().await?;
        Ok(profile)
    }

    // ─── Retry logic ─────────────────────────────────────────────────────────

    async fn get_with_retry(
        &self,
        url: &str,
        params: &[(&str, &str)],
    ) -> Result<Response> {
        let mut attempt = 0u32;

        loop {
            debug!(url, attempt, "Raider.IO request");

            let resp = self
                .client
                .get(url)
                .query(params)
                .send()
                .await;

            match resp {
                Err(e) => {
                    // Network-level error
                    attempt += 1;
                    if attempt > self.cfg.max_retries {
                        return Err(anyhow!("Network error after {attempt} attempts: {e}"));
                    }
                    let delay = self.backoff_ms(attempt, None);
                    warn!(attempt, delay_ms = delay, error = %e, "Network error, retrying");
                    sleep(Duration::from_millis(delay)).await;
                }
                Ok(r) => {
                    let status = r.status();

                    if status == StatusCode::TOO_MANY_REQUESTS {
                        attempt += 1;
                        if attempt > self.cfg.max_retries {
                            return Err(anyhow!("Rate limited after {attempt} attempts"));
                        }
                        // Parse Retry-After header
                        let retry_after_ms = r
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                            .map(|secs| secs * 1_000);

                        let delay = self.backoff_ms(attempt, retry_after_ms);
                        warn!(
                            attempt,
                            delay_ms = delay,
                            retry_after_header = ?retry_after_ms,
                            "Rate limited (429), retrying"
                        );
                        sleep(Duration::from_millis(delay)).await;
                        continue;
                    }

                    if status.is_server_error() {
                        attempt += 1;
                        if attempt > self.cfg.max_retries {
                            return Err(anyhow!(
                                "Server error {status} after {attempt} attempts"
                            ));
                        }
                        let delay = self.backoff_ms(attempt, None);
                        warn!(attempt, delay_ms = delay, %status, "Server error, retrying");
                        sleep(Duration::from_millis(delay)).await;
                        continue;
                    }

                    if !status.is_success() {
                        let body = r.text().await.unwrap_or_default();
                        return Err(anyhow!("Raider.IO error {status}: {body}"));
                    }

                    return Ok(r);
                }
            }
        }
    }

    /// Exponential backoff with jitter.
    /// If `header_ms` is provided (from Retry-After), use max(header, computed).
    fn backoff_ms(&self, attempt: u32, header_ms: Option<u64>) -> u64 {
        let exp: u64 = self
            .cfg
            .base_backoff_ms
            .saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1)));
        let capped = exp.min(self.cfg.max_backoff_ms);
        // jitter: ±25 %
        let jitter_range = capped / 4;
        let jitter = rand::thread_rng().gen_range(0..=jitter_range);
        let computed = capped.saturating_add(jitter);
        header_ms.map_or(computed, |h| h.max(computed))
    }
}
