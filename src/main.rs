// src/main.rs — entry point: load config, init DB, seed players, start server

use anyhow::{Context, Result};
use axum::{
    Router, http::HeaderValue, routing::{delete, get, post}
};
use std::net::SocketAddr;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{info, Level};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use axum::http::Method;
use tower_http::cors::{Any, CorsLayer};

mod auth;
mod config;
mod db;
mod handlers;
mod hash;
mod models;
mod raiderio;
mod service;
mod time_window;

use auth::BearerAuthLayer;
use config::{config_path, Config};
use db::Database;
use raiderio::RaiderIoClient;
use service::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    // ── Logging ────────────────────────────────────────────────────────────
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_target(true)
                .with_thread_ids(false)
                .compact(),
        )
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("mplus-tracker starting up");

    // ── Auth token ─────────────────────────────────────────────────────────
    // Must be set via environment — fail loudly at startup if missing.
    // Generate with:  openssl rand -hex 32
    let api_token = std::env::var("API_TOKEN")
        .context("API_TOKEN env var is required. Generate with: openssl rand -hex 32")?;
    if api_token.len() < 32 {
        anyhow::bail!("API_TOKEN is too short — use at least 32 characters");
    }
    info!("Bearer auth enabled (token length: {} chars)", api_token.len());

    // ── Config ─────────────────────────────────────────────────────────────
    let cfg_path = config_path();
    info!(path = %cfg_path, "Loading config");
    let config = Config::load(&cfg_path)
        .with_context(|| format!("Failed to load config from {cfg_path}"))?;

    // ── Database ───────────────────────────────────────────────────────────
    let db = Database::connect(&config.storage.database_path).await?;
    db.migrate().await?;

    // ── Seed players and characters from config ────────────────────────────
    seed_from_config(&db, &config).await?;

    // ── Raider.IO client ───────────────────────────────────────────────────
    let rio = RaiderIoClient::new(config.raiderio.clone())?;

    // ── App state ──────────────────────────────────────────────────────────
    let state = AppState::new(db, rio, config.clone());

    // ── CORS/HEADERS ───────────────────────────────────────────────────────
    let cors = CorsLayer::new()
        .allow_origin("https://mplus.seemsgood.org".parse::<HeaderValue>().unwrap())
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers(Any);


    // ── Router ─────────────────────────────────────────────────────────────
    let app = Router::new()
        // Health — also exempted inside BearerAuthLayer, belt-and-suspenders
        .route("/health", get(handlers::get_health))
        // Update endpoints
        .route("/update/guild",     post(handlers::post_update_guild))
        .route("/update/character", post(handlers::post_update_character))
        .route("/update/all",       post(handlers::post_update_all))
        // Character management
        .route(
            "/character/:region/:realm/:name",
            delete(handlers::delete_character),
        )
        // Query endpoints
        .route("/players", get(handlers::get_players))
        .route("/leaderboard", get(handlers::get_leaderboard))
        .route(
            "/character/:region/:realm/:name/keys",
            get(handlers::get_character_keys),
        )
        .route("/player/:player_id/keys", get(handlers::get_player_keys))
        .route(
            "/guild/:region/:realm/:name/roster",
            get(handlers::get_guild_roster),
        )
        // Debug / dump endpoints
        .route("/debug/runs",          get(handlers::get_debug_runs))
        .route("/debug/runs/guild",    get(handlers::get_debug_guild_runs))
        .route("/debug/characters",    get(handlers::get_debug_characters))
        .route("/debug/stats",         get(handlers::get_debug_stats))
        .route("/debug/hash-check",    get(handlers::get_debug_hash_check))
        .route("/debug/depletions",     get(handlers::get_debug_depletions))
        // Auth layer wraps everything
        .layer(BearerAuthLayer::new(api_token))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(cors)
        .with_state(state);

    // ── Listen ─────────────────────────────────────────────────────────────
    let addr: SocketAddr = format!(
        "{}:{}",
        config.server.host, config.server.port
    )
    .parse()
    .context("Invalid server address")?;

    info!(address = %addr, "Server listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// On startup, upsert every player and character declared in config.toml.
async fn seed_from_config(db: &Database, config: &Config) -> Result<()> {
    for player in &config.players {
        db.upsert_player(&player.id, &player.label).await?;
        for char_ref in &player.characters {
            let char_id = db
                .upsert_character(
                    &char_ref.region,
                    &char_ref.realm,
                    &char_ref.name,
                    None,
                    None,
                )
                .await?;
            db.link_player_character(&player.id, char_id).await?;
        }
        info!(
            player_id = %player.id,
            label     = %player.label,
            chars     = player.characters.len(),
            "Seeded player"
        );
    }
    Ok(())
}
