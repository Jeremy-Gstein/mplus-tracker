# mplus-tracker

A Rust web service that tracks World of Warcraft Mythic+ activity for characters
and guilds using the public [Raider.IO API](https://raider.io/api).

---

## Prerequisites

| Tool | Minimum version |
|------|----------------|
| Docker | 24+ |
| Docker Compose (plugin) | v2.20+ |
| `curl` or any HTTP client | any |

No local Rust installation is required — everything builds inside Docker.

---

## Quick start

### 1. Edit `config.toml`

Open `config.toml` and replace the example players/guilds with your own:

```toml
[[guilds]]
region = "us"
realm  = "area-52"
name   = "My Guild"

[[players]]
id    = "player_ash"
label = "Ash"

[[players.characters]]
region = "us"
realm  = "area-52"
name   = "Ashmain"

[[players.characters]]
region = "us"
realm  = "area-52"
name   = "Ashalt"
```

**Realm names** must match Raider.IO's slug format (lowercase, hyphens, no spaces).
Examples: `area-52`, `stormrage`, `tarren-mill`, `draenor`.

### 2. Build and start

```bash
docker-compose up --build
```

The first build downloads Rust crates and compiles the binary (~3–5 minutes).
Subsequent builds use Docker's layer cache and are much faster.

You should see output like:

```
mplus-tracker  | 2024-... INFO mplus_tracker: Server listening address=0.0.0.0:8080
```

### 3. Verify the service is up

```bash
curl http://localhost:8080/health
# {"status":"ok","version":"0.1.0"}
```

---

## Configuration reference

`config.toml` is mounted read-only into the container at `/config/config.toml`.
All critical settings can also be overridden via environment variables in
`docker-compose.yml`.

| TOML key | Env var | Default | Description |
|----------|---------|---------|-------------|
| `storage.database_path` | `DATABASE_PATH` | `/data/mplus.sqlite` | SQLite file path inside the container |
| `server.host` | `SERVER_HOST` | `0.0.0.0` | Bind address |
| `server.port` | `SERVER_PORT` | `8080` | HTTP port |
| `concurrency.max_concurrent_raiderio` | `MAX_CONCURRENT_RAIDERIO` | `3` | Max parallel Raider.IO requests |
| `raiderio.max_retries` | — | `5` | Max retry attempts on 429/5xx |
| `raiderio.base_backoff_ms` | — | `1000` | Initial backoff in milliseconds |
| `raiderio.max_backoff_ms` | — | `120000` | Maximum backoff cap (2 minutes) |

Log level is controlled by the `RUST_LOG` env var (e.g. `info`, `debug`,
`mplus_tracker=trace`).

---

## API reference

### Health check

```
GET /health
```

Returns `{"status":"ok","version":"0.1.0"}`.

---

### Update endpoints

> These trigger live calls to Raider.IO and may take a few seconds.
> All parameters are **query string** parameters.

#### `POST /update/guild`

Fetch guild members from Raider.IO and upsert them into the DB.

| Parameter | Required | Example |
|-----------|----------|---------|
| `region` | yes | `us` |
| `realm` | yes | `area-52` |
| `name` | yes | `My+Guild` |

```bash
curl -X POST "http://localhost:8080/update/guild?region=us&realm=area-52&name=My+Guild"
```

```json
{
  "request_id": "...",
  "guild": { "region": "us", "realm": "area-52", "name": "My Guild" },
  "members_added": 24,
  "members_updated": 0
}
```

---

#### `POST /update/character`

Fetch recent Mythic+ runs for a character and store them.

| Parameter | Required | Example |
|-----------|----------|---------|
| `region` | yes | `us` |
| `realm` | yes | `area-52` |
| `name` | yes | `Ashmain` |

```bash
curl -X POST "http://localhost:8080/update/character?region=us&realm=area-52&name=Ashmain"
```

```json
{
  "request_id": "...",
  "character": { "region": "us", "realm": "area-52", "name": "Ashmain" },
  "runs_inserted": 8,
  "runs_ignored": 2,
  "rate_limited": false
}
```

---

#### `POST /update/all`

Update every character currently stored in the DB. Concurrency is bounded
by `max_concurrent_raiderio` in config.

```bash
curl -X POST http://localhost:8080/update/all
```

```json
{
  "request_id": "...",
  "total_characters": 5,
  "updated_ok": 5,
  "failed": 0,
  "errors": []
}
```

---

### Query endpoints

All query endpoints accept these parameters:

| Parameter | Values | Default | Description |
|-----------|--------|---------|-------------|
| `scope` | `today`, `week`, `alltime`, `custom` | `alltime` | Time window |
| `min_level` | integer | `0` | Only count keys at or above this level |
| `from` | ISO 8601 datetime | — | Required when `scope=custom` |
| `to` | ISO 8601 datetime | — | Required when `scope=custom` |

**Weekly reset windows:**
- NA/US: Tuesday 15:00 UTC
- EU: Wednesday 07:00 UTC

---

#### `GET /character/{region}/{realm}/{name}/keys`

Count Mythic+ runs for a single character.

```bash
# All-time keys
curl "http://localhost:8080/character/us/area-52/Ashmain/keys"

# Keys done this week, level 10+
curl "http://localhost:8080/character/us/area-52/Ashmain/keys?scope=week&min_level=10"

# Keys in a custom date range
curl "http://localhost:8080/character/us/area-52/Ashmain/keys?scope=custom&from=2024-10-01T00:00:00Z&to=2024-10-31T23:59:59Z"
```

```json
{
  "character": { "region": "us", "realm": "area-52", "name": "Ashmain" },
  "scope": "week",
  "from": "2024-10-15T15:00:00Z",
  "to": "2024-10-18T20:34:12Z",
  "min_level": 10,
  "count": 14
}
```

---

#### `GET /player/{player_id}/keys`

Count Mythic+ runs across all alts for a logical player (aggregated).
`player_id` must match the `id` field in `config.toml`.

```bash
curl "http://localhost:8080/player/player_ash/keys?scope=week"
```

```json
{
  "player_id": "player_ash",
  "label": "Ash",
  "scope": "week",
  "from": "2024-10-15T15:00:00Z",
  "to": "2024-10-18T20:34:12Z",
  "min_level": 0,
  "count": 21
}
```

---

#### `GET /guild/{region}/{realm}/{name}/roster`

Return the guild roster currently stored in the DB.
Run `POST /update/guild` first to populate it.

```bash
curl "http://localhost:8080/guild/us/area-52/My+Guild/roster"
```

```json
{
  "guild": { "region": "us", "realm": "area-52", "name": "My Guild" },
  "members": [
    { "region": "us", "realm": "area-52", "name": "Ashmain", "guild_name": "My Guild" }
  ]
}
```

---

## Typical usage workflow

```bash
# 1. Start the service
docker-compose up --build -d

# 2. Pull guild roster (populates characters table)
curl -X POST "http://localhost:8080/update/guild?region=us&realm=area-52&name=My+Guild"

# 3. Update a specific character's runs
curl -X POST "http://localhost:8080/update/character?region=us&realm=area-52&name=Ashmain"

# 4. Update ALL known characters at once
curl -X POST http://localhost:8080/update/all

# 5. Query this week's keys for a player (all their alts combined)
curl "http://localhost:8080/player/player_ash/keys?scope=week&min_level=10"
```

---

## Data persistence

The SQLite database is stored in a named Docker volume (`mplus-db`).
It survives `docker-compose down` and restarts.

To inspect the database directly:

```bash
# Install sqlite3 if needed: brew install sqlite  /  apt install sqlite3
docker run --rm -it \
  -v mplus-tracker_mplus-db:/data \
  alpine sh -c "apk add sqlite && sqlite3 /data/mplus.sqlite"

# Inside sqlite3:
.tables
SELECT name, COUNT(*) FROM characters GROUP BY name LIMIT 10;
SELECT dungeon_short, key_level, completed_at FROM runs ORDER BY completed_at DESC LIMIT 20;
.quit
```

To reset all data:

```bash
docker-compose down -v   # removes the volume
docker-compose up -d
```

---

## Running without Docker (local dev)

You need Rust 1.78+ and `sqlite3` development headers.

```bash
# macOS
brew install sqlite

# Ubuntu/Debian
sudo apt-get install libsqlite3-dev pkg-config

# Build and run
DATABASE_PATH=./mplus.sqlite CONFIG_PATH=./config.toml cargo run
```

---

## Stopping and cleaning up

```bash
# Stop, keep data
docker-compose stop

# Stop and remove containers (data volume survives)
docker-compose down

# Stop, remove containers AND all data
docker-compose down -v
```

---

## Troubleshooting

**"character not found in DB"** when querying keys
→ The character hasn't been updated yet. Run `POST /update/character` or
`POST /update/all` first.

**"Failed to read config file"**
→ Check that `config.toml` exists in the same directory as `docker-compose.yml`.

**429 rate limit errors in logs**
→ Normal — the service retries with exponential backoff automatically.
Lower `max_concurrent_raiderio` in config.toml if this is frequent.

**Build fails with linker errors**
→ Ensure Docker has at least 2 GB of memory allocated (Docker Desktop → Settings → Resources).
