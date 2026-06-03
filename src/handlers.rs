// src/handlers.rs — axum route handlers

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use crate::{
    models::ApiError,
    service::{update_all_characters, update_character, update_guild, AppState},
    time_window::{Scope, TimeWindow},
};

// ─── Error helper ─────────────────────────────────────────────────────────────

pub(crate) struct AppErr(StatusCode, String);

impl IntoResponse for AppErr {
    fn into_response(self) -> Response {
        let body = Json(ApiError {
            error: self.1,
            code: self.0.as_u16(),
        });
        (self.0, body).into_response()
    }
}

impl From<anyhow::Error> for AppErr {
    fn from(e: anyhow::Error) -> Self {
        AppErr(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

macro_rules! bad_request {
    ($msg:expr) => {
        return Err(AppErr(StatusCode::BAD_REQUEST, $msg.to_string()))
    };
}

macro_rules! not_found {
    ($msg:expr) => {
        return Err(AppErr(StatusCode::NOT_FOUND, $msg.to_string()))
    };
}

// ─── POST /update/guild ───────────────────────────────────────────────────────

#[derive(Deserialize, Debug)]
pub struct GuildParams {
    pub region: Option<String>,
    pub realm: Option<String>,
    pub name: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct GuildUpdateResponse {
    pub request_id: String,
    pub guild: GuildRef,
    pub members_added: usize,
    pub members_updated: usize,
}

#[derive(Serialize, Debug)]
pub struct GuildRef {
    pub region: String,
    pub realm: String,
    pub name: String,
}

#[instrument(skip(state))]
pub async fn post_update_guild(
    State(state): State<AppState>,
    Query(params): Query<GuildParams>,
) -> Result<impl IntoResponse, AppErr> {
    let region = params.region.as_deref().unwrap_or("");
    let realm = params.realm.as_deref().unwrap_or("");
    let name = params.name.as_deref().unwrap_or("");

    if region.is_empty() { bad_request!("Missing `region` parameter"); }
    if realm.is_empty()  { bad_request!("Missing `realm` parameter");  }
    if name.is_empty()   { bad_request!("Missing `name` parameter");   }

    let result = update_guild(&state, region, realm, name).await?;

    Ok(Json(GuildUpdateResponse {
        request_id: Uuid::new_v4().to_string(),
        guild: GuildRef {
            region: result.region,
            realm: result.realm,
            name: result.name,
        },
        members_added: result.members_added,
        members_updated: result.members_updated,
    }))
}

// ─── POST /update/character ───────────────────────────────────────────────────

#[derive(Deserialize, Debug)]
pub struct CharacterParams {
    pub region: Option<String>,
    pub realm: Option<String>,
    pub name: Option<String>,
}

#[derive(Serialize)]
pub struct CharacterUpdateResponse {
    pub request_id: String,
    pub character: CharacterRef,
    pub runs_inserted: usize,
    pub runs_ignored: usize,
    pub rate_limited: bool,
}

#[derive(Serialize)]
pub struct CharacterRef {
    pub region: String,
    pub realm: String,
    pub name: String,
}

#[instrument(skip(state))]
pub async fn post_update_character(
    State(state): State<AppState>,
    Query(params): Query<CharacterParams>,
) -> Result<impl IntoResponse, AppErr> {
    let region = params.region.as_deref().unwrap_or("");
    let realm = params.realm.as_deref().unwrap_or("");
    let name = params.name.as_deref().unwrap_or("");

    if region.is_empty() { bad_request!("Missing `region` parameter"); }
    if realm.is_empty()  { bad_request!("Missing `realm` parameter");  }
    if name.is_empty()   { bad_request!("Missing `name` parameter");   }

    let result = update_character(&state, region, realm, name).await?;

    Ok(Json(CharacterUpdateResponse {
        request_id: Uuid::new_v4().to_string(),
        character: CharacterRef {
            region: result.character.region,
            realm: result.character.realm,
            name: result.character.name,
        },
        runs_inserted: result.runs_inserted,
        runs_ignored: result.runs_ignored,
        rate_limited: false, // backoff handled internally; request succeeded by the time we're here
    }))
}

// ─── POST /update/all ─────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct UpdateAllResponse {
    pub request_id: String,
    pub total_characters: usize,
    pub updated_ok: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

#[instrument(skip(state))]
pub async fn post_update_all(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppErr> {
    let result = update_all_characters(&state).await?;

    Ok(Json(UpdateAllResponse {
        request_id: Uuid::new_v4().to_string(),
        total_characters: result.total_characters,
        updated_ok: result.updated_ok,
        failed: result.failed,
        errors: result.errors,
    }))
}

// ─── GET /character/{region}/{realm}/{name}/keys ──────────────────────────────

#[derive(Deserialize, Debug)]
pub struct KeysQuery {
    #[serde(default)]
    pub scope: Scope,
    pub min_level: Option<i64>,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Serialize)]
pub struct CharacterKeysResponse {
    pub character: CharacterRef,
    pub scope: String,
    pub from: String,
    pub to: String,
    pub min_level: i64,
    pub count: i64,
}

#[instrument(skip(state))]
pub async fn get_character_keys(
    State(state): State<AppState>,
    Path((region, realm, name)): Path<(String, String, String)>,
    Query(query): Query<KeysQuery>,
) -> Result<impl IntoResponse, AppErr> {
    let char = state
        .db
        .get_character(&region, &realm, &name)
        .await
        .map_err(AppErr::from)?;

    let char = match char {
        Some(c) => c,
        None => not_found!(format!(
            "Character {name} on {realm}-{region} not found in DB"
        )),
    };

    let custom_from = parse_optional_dt(query.from.as_deref())?;
    let custom_to = parse_optional_dt(query.to.as_deref())?;

    let window =
        TimeWindow::resolve(query.scope, Some(&region), custom_from, custom_to)
            .map_err(|e| AppErr(StatusCode::BAD_REQUEST, e.to_string()))?;

    let min_level = query.min_level.unwrap_or(0);

    let count = state
        .db
        .count_runs_for_character(char.id, window.from, window.to, min_level)
        .await
        .map_err(AppErr::from)?;

    Ok(Json(CharacterKeysResponse {
        character: CharacterRef { region, realm, name },
        scope: format!("{:?}", query.scope).to_lowercase(),
        from: window.from.to_rfc3339(),
        to: window.to.to_rfc3339(),
        min_level,
        count,
    }))
}

// ─── GET /player/{player_id}/keys ────────────────────────────────────────────

#[derive(Serialize)]
pub struct PlayerKeysResponse {
    pub player_id: String,
    pub label: String,
    pub scope: String,
    pub from: String,
    pub to: String,
    pub min_level: i64,
    pub count: i64,
}

#[instrument(skip(state))]
pub async fn get_player_keys(
    State(state): State<AppState>,
    Path(player_id): Path<String>,
    Query(query): Query<KeysQuery>,
) -> Result<impl IntoResponse, AppErr> {
    let player = state
        .db
        .get_player(&player_id)
        .await
        .map_err(AppErr::from)?;

    let player = match player {
        Some(p) => p,
        None => not_found!(format!("Player `{player_id}` not found")),
    };

    let char_ids = state
        .db
        .get_player_character_ids(&player_id)
        .await
        .map_err(AppErr::from)?;

    let custom_from = parse_optional_dt(query.from.as_deref())?;
    let custom_to = parse_optional_dt(query.to.as_deref())?;

    let window = TimeWindow::resolve(query.scope, None, custom_from, custom_to)
        .map_err(|e| AppErr(StatusCode::BAD_REQUEST, e.to_string()))?;

    let min_level = query.min_level.unwrap_or(0);

    let count = state
        .db
        .count_runs_for_characters(&char_ids, window.from, window.to, min_level)
        .await
        .map_err(AppErr::from)?;

    Ok(Json(PlayerKeysResponse {
        player_id,
        label: player.label,
        scope: format!("{:?}", query.scope).to_lowercase(),
        from: window.from.to_rfc3339(),
        to: window.to.to_rfc3339(),
        min_level,
        count,
    }))
}

// ─── GET /guild/{region}/{realm}/{name}/roster ────────────────────────────────

#[derive(Serialize)]
pub struct RosterResponse {
    pub guild: GuildRef,
    pub members: Vec<crate::models::CharacterSummary>,
}

#[instrument(skip(state))]
pub async fn get_guild_roster(
    State(state): State<AppState>,
    Path((region, realm, name)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, AppErr> {
    let members = state
        .db
        .get_guild_members(&name, &realm)
        .await
        .map_err(AppErr::from)?;

    let summaries = members
        .iter()
        .map(|c| crate::models::CharacterSummary::from(c))
        .collect();

    Ok(Json(RosterResponse {
        guild: GuildRef { region, realm, name },
        members: summaries,
    }))
}

// ─── GET /health ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
}

pub async fn get_health() -> impl IntoResponse {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn parse_optional_dt(s: Option<&str>) -> Result<Option<DateTime<chrono::Utc>>, AppErr> {
    match s {
        None => Ok(None),
        Some(raw) => {
            let dt = DateTime::parse_from_rfc3339(raw)
                .map_err(|e| AppErr(StatusCode::BAD_REQUEST, format!("Invalid datetime `{raw}`: {e}")))?
                .with_timezone(&chrono::Utc);
            Ok(Some(dt))
        }
    }
}
