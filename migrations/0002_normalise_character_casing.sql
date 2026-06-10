-- Migration: normalise character (region, realm, name) to lowercase
--
-- Root cause: Raider.IO returns realm/name casing that varies between API
-- endpoints (guild profile vs character profile), and config.toml may use
-- different casing again.  Because the UNIQUE constraint on `characters` is
-- case-sensitive in SQLite, this produced duplicate rows for the same character
-- — one from a guild roster pull, one from seed_from_config — and only one of
-- them would be linked to its player entry.
--
-- This migration collapses any such duplicates: for each group of rows that
-- share the same lowercase (region, realm, name), we keep the one with the
-- lowest id (the oldest insert) and re-point all runs and player_characters
-- links that reference the newer duplicate rows onto the survivor, then delete
-- the duplicates.  Afterwards every row is updated to lowercase in place so
-- the new upsert_character code (which normalises before insert) can always
-- find the canonical row via ON CONFLICT.

-- ── Step 1: re-point runs from duplicate character rows to the survivor ───────
UPDATE runs
SET character_id = (
    SELECT MIN(id)
    FROM characters c2
    WHERE LOWER(c2.region) = LOWER((SELECT region FROM characters WHERE id = runs.character_id))
      AND LOWER(c2.realm)  = LOWER((SELECT realm  FROM characters WHERE id = runs.character_id))
      AND LOWER(c2.name)   = LOWER((SELECT name   FROM characters WHERE id = runs.character_id))
)
WHERE character_id NOT IN (
    SELECT MIN(id)
    FROM characters
    GROUP BY LOWER(region), LOWER(realm), LOWER(name)
);

-- ── Step 2: re-point player_characters links from duplicates to the survivor ──
-- INSERT OR IGNORE so we don't violate the PK if the survivor already has a link.
INSERT OR IGNORE INTO player_characters (player_id, character_id)
SELECT pc.player_id,
       (
           SELECT MIN(id)
           FROM characters c2
           WHERE LOWER(c2.region) = LOWER(c.region)
             AND LOWER(c2.realm)  = LOWER(c.realm)
             AND LOWER(c2.name)   = LOWER(c.name)
       ) AS canonical_id
FROM player_characters pc
JOIN characters c ON c.id = pc.character_id
WHERE pc.character_id NOT IN (
    SELECT MIN(id)
    FROM characters
    GROUP BY LOWER(region), LOWER(realm), LOWER(name)
);

-- ── Step 3: delete the duplicate (non-canonical) character rows ───────────────
DELETE FROM characters
WHERE id NOT IN (
    SELECT MIN(id)
    FROM characters
    GROUP BY LOWER(region), LOWER(realm), LOWER(name)
);

-- ── Step 4: lowercase every surviving row in place ────────────────────────────
UPDATE characters
SET
    region     = LOWER(region),
    realm      = LOWER(realm),
    name       = LOWER(name),
    guild_realm = LOWER(guild_realm);
