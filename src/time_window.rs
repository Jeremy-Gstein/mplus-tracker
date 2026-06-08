// src/time_window.rs — scope-to-time-range resolution

use anyhow::{anyhow, Result};
use chrono::{DateTime, Datelike, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::America::New_York;
use serde::Deserialize;

/// NA weekly reset: Tuesday 09:00 America/New_York.
/// Using a named timezone handles EDT (UTC-4) vs EST (UTC-5) automatically.
const NA_RESET_WEEKDAY: Weekday = Weekday::Tue;
const NA_RESET_TIME: NaiveTime = match NaiveTime::from_hms_opt(9, 0, 0) {
    Some(t) => t,
    None => panic!("invalid reset time"),
};

/// EU weekly reset: Wednesday 07:00 UTC (no DST offset needed).
const EU_RESET_WEEKDAY: Weekday = Weekday::Wed;
const EU_RESET_HOUR_UTC: u32 = 7;

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
                let epoch = DateTime::from_timestamp(0, 0)
                    .ok_or_else(|| anyhow!("epoch failed"))?;
                Ok(Self { from: epoch, to: now })
            }
            Scope::Custom => {
                let from = custom_from
                    .ok_or_else(|| anyhow!("scope=custom requires `from` parameter"))?;
                let to = custom_to
                    .ok_or_else(|| anyhow!("scope=custom requires `to` parameter"))?;
                if from > to {
                    return Err(anyhow!("`from` must be before `to`"));
                }
                Ok(Self { from, to })
            }
        }
    }
}

/// Return the UTC instant of the most-recent weekly reset for the given region.
///
/// NA: Tuesday 09:00 America/New_York — chrono-tz handles EDT/EST automatically.
/// EU: Wednesday 07:00 UTC.
fn weekly_reset_start(now: DateTime<Utc>, region: &str) -> Result<DateTime<Utc>> {
    if region.eq_ignore_ascii_case("eu") {
        return weekly_reset_eu(now);
    }
    weekly_reset_na(now)
}

fn weekly_reset_na(now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    // Convert now to Eastern time so we can reason in local wall-clock time.
    let now_et = now.with_timezone(&New_York);

    for days_back in 0i64..8 {
        let candidate_et = now_et - chrono::Duration::days(days_back);

        if candidate_et.weekday() != NA_RESET_WEEKDAY {
            continue;
        }

        // Build the reset instant in ET (handles DST via chrono-tz).
        let candidate_naive = candidate_et
            .date_naive()
            .and_time(NA_RESET_TIME);

        let candidate_et = New_York
            .from_local_datetime(&candidate_naive)
            .earliest()
            .ok_or_else(|| anyhow!("Could not build ET reset datetime (DST gap?)"))?;

        let candidate_utc = candidate_et.with_timezone(&Utc);

        if candidate_utc <= now {
            return Ok(candidate_utc);
        }
    }

    Err(anyhow!("Could not determine NA weekly reset start"))
}

fn weekly_reset_eu(now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    for days_back in 0i64..8 {
        let candidate = now - chrono::Duration::days(days_back);

        if candidate.weekday() != EU_RESET_WEEKDAY {
            continue;
        }

        let reset = Utc
            .with_ymd_and_hms(
                candidate.year(),
                candidate.month(),
                candidate.day(),
                EU_RESET_HOUR_UTC,
                0,
                0,
            )
            .single()
            .ok_or_else(|| anyhow!("Could not build EU reset datetime"))?;

        if reset <= now {
            return Ok(reset);
        }
    }

    Err(anyhow!("Could not determine EU weekly reset start"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tuesday 2024-11-05 was during EST (UTC-5), so 09:00 ET = 14:00 UTC.
    #[test]
    fn na_reset_est() {
        // Wednesday 2024-11-06 12:00 UTC — reset was prior Tuesday in EST
        let now = DateTime::from_timestamp(1730891200, 0).unwrap(); // Wed 2024-11-06 ~12:00 UTC
        let reset = weekly_reset_na(now).unwrap();
        // Tuesday 2024-11-05 09:00 EST = 14:00 UTC
        assert_eq!(reset.hour(), 14);
        assert_eq!(reset.weekday(), Weekday::Tue);
    }

    /// Tuesday 2024-03-12 was during EDT (UTC-4), so 09:00 ET = 13:00 UTC.
    #[test]
    fn na_reset_edt() {
        // Wednesday 2024-03-13 12:00 UTC — reset was prior Tuesday in EDT
        let now = DateTime::from_timestamp(1710331200, 0).unwrap(); // Wed 2024-03-13 ~12:00 UTC
        let reset = weekly_reset_na(now).unwrap();
        // Tuesday 2024-03-12 09:00 EDT = 13:00 UTC
        assert_eq!(reset.hour(), 13);
        assert_eq!(reset.weekday(), Weekday::Tue);
    }
}
