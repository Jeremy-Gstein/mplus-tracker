// src/hash.rs — deterministic dedupe hash for run records

use sha2::{Digest, Sha256};

/// Produces a hex SHA-256 hash that uniquely identifies a run for a character.
/// This ensures ON CONFLICT (hash) DO NOTHING works correctly across re-fetches.
pub fn run_hash(
    region: &str,
    realm: &str,
    name: &str,
    dungeon_short: &str,
    key_level: i64,
    completed_at: &str,        // ISO8601 string from Raider.IO (before parsing)
    source_run_id: Option<&str>,
) -> String {
    let mut h = Sha256::new();
    h.update(region.to_lowercase().as_bytes());
    h.update(b"|");
    h.update(realm.to_lowercase().as_bytes());
    h.update(b"|");
    h.update(name.to_lowercase().as_bytes());
    h.update(b"|");
    h.update(dungeon_short.to_uppercase().as_bytes());
    h.update(b"|");
    h.update(key_level.to_string().as_bytes());
    h.update(b"|");
    h.update(completed_at.as_bytes());
    h.update(b"|");
    h.update(source_run_id.unwrap_or("").as_bytes());
    hex::encode(h.finalize())
}
