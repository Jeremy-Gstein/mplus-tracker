// src/time_window.rs — scope-to-time-range resolution

use anyhow::{anyhow, Result};
use chrono::{DateTime, Datelike, TimeZone, Utc, Weekday};
use serde::Deserialize;

/// The "weekly reset" for NA/US is Tuesday 15:00 UTC.
/// For EU it is Wednesday 07:00 UTC.
/// We default to NA (Tuesday) for unrecognised regions.
const NA_RESET_WEEKDAY: Weekday = Weekday::Tue;
const NA_RESET_HOUR: u32 = 15; // 15:00 UTC
const EU_RESET_WEEKDAY: Weekday = Weekday::Wed;
const EU_RESET_HOUR: u32 = 7; // 07:00 UTC

#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Today,
    Week,
    Alltime,
    Custom,
}

impl Default for Scope {
    fn default() -> Self {
        Scope::Alltime
    }
}

pub struct TimeWindow {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

impl TimeWindow {
    pub fn resolve(
        scope: Scope,
        region: Option<&str>,
        custom_from: Option<DateTime<Utc>>,
        custom_to: Option<DateTime<Utc>>,
    ) -> Result<Self> {
        let now = Utc::now();
        match scope {
            Scope::Today => {
                let midnight = Utc
                    .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
                    .single()
                    .ok_or_else(|| anyhow!("Could not compute midnight"))?;
                Ok(Self { from: midnight, to: now })
            }
            Scope::Week => {
                let start = weekly_reset_start(now, region.unwrap_or("us"))?;
                Ok(Self { from: start, to: now })
            }
            Scope::Alltime => {
                // Unix epoch as "no lower bound"
                let epoch = DateTime::from_timestamp(0, 0)
                    .ok_or_else(|| anyhow!("epoch failed"))?;
                Ok(Self { from: epoch, to: now })
            }
            Scope::Custom => {
                let from = custom_from.ok_or_else(|| anyhow!("scope=custom requires `from` parameter"))?;
                let to = custom_to.ok_or_else(|| anyhow!("scope=custom requires `to` parameter"))?;
                if from > to {
                    return Err(anyhow!("`from` must be before `to`"));
                }
                Ok(Self { from, to })
            }
        }
    }
}

/// Return the start of the most-recent weekly reset for the given region.
fn weekly_reset_start(now: DateTime<Utc>, region: &str) -> Result<DateTime<Utc>> {
    let (reset_day, reset_hour) = if region.eq_ignore_ascii_case("eu") {
        (EU_RESET_WEEKDAY, EU_RESET_HOUR)
    } else {
        (NA_RESET_WEEKDAY, NA_RESET_HOUR)
    };

    // Walk back from now until we find the most recent reset point
    // We look up to 8 days back to be safe
    for days_back in 0i64..8 {
        let candidate_date = now - chrono::Duration::days(days_back);
        if candidate_date.weekday() == reset_day {
            let candidate = Utc
                .with_ymd_and_hms(
                    candidate_date.year(),
                    candidate_date.month(),
                    candidate_date.day(),
                    reset_hour,
                    0,
                    0,
                )
                .single()
                .ok_or_else(|| anyhow!("Could not build reset datetime"))?;

            if candidate <= now {
                return Ok(candidate);
            }
        }
    }

    Err(anyhow!("Could not determine weekly reset start"))
}
