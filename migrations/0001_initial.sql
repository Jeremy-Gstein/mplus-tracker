-- Migration: initial schema

CREATE TABLE IF NOT EXISTS characters (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    region      TEXT    NOT NULL,
    realm       TEXT    NOT NULL,
    name        TEXT    NOT NULL,
    guild_name  TEXT,
    guild_realm TEXT,
    last_seen   DATETIME,
    UNIQUE (region, realm, name)
);

CREATE TABLE IF NOT EXISTS players (
    id    TEXT PRIMARY KEY,
    label TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS player_characters (
    player_id    TEXT    NOT NULL REFERENCES players(id) ON DELETE CASCADE,
    character_id INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    PRIMARY KEY (player_id, character_id)
);

CREATE TABLE IF NOT EXISTS runs (
    id           INTEGER  PRIMARY KEY AUTOINCREMENT,
    character_id INTEGER  NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    dungeon_short TEXT    NOT NULL,
    key_level    INTEGER  NOT NULL,
    completed_at DATETIME NOT NULL,
    within_time  INTEGER  NOT NULL DEFAULT 0,  -- 0/1 bool
    season       TEXT,
    url          TEXT,
    source_run_id TEXT,
    hash         TEXT     NOT NULL,
    UNIQUE (hash)
);

CREATE INDEX IF NOT EXISTS idx_runs_character_id      ON runs (character_id);
CREATE INDEX IF NOT EXISTS idx_runs_completed_at      ON runs (completed_at);
CREATE INDEX IF NOT EXISTS idx_runs_character_completed ON runs (character_id, completed_at);
