import { randomBytes } from "node:crypto";
import type Database from "better-sqlite3";

const KEY_PREFIX = "sp_";
const KEY_BYTE_LENGTH = 32;
/** Generate a random API key with prefix */
const generateApiKey = (): string => {
  const bytes = randomBytes(KEY_BYTE_LENGTH);
  return `${KEY_PREFIX}${bytes.toString("hex")}`;
};

export interface ApiKeyInfo {
  readonly id: string;
  readonly userId: string;
  readonly key: string;
  readonly createdAt: string;
  readonly lastUsedAt: string | null;
}

interface ApiKeyRow {
  readonly id: string;
  readonly user_id: string;
  readonly key_plaintext: string | null;
  readonly created_at: string;
  readonly last_used_at: string | null;
}

/** Get the current (single) API key for a user, or null if no key exists. */
export const getCurrentApiKey = (
  db: Database.Database,
  userId: string,
): ApiKeyInfo | null => {
  const row = db
    .prepare(
      `SELECT id, user_id, key_plaintext, created_at, last_used_at
       FROM api_keys WHERE user_id = ? ORDER BY created_at DESC LIMIT 1`,
    )
    .get(userId) as ApiKeyRow | undefined;

  if (!row || !row.key_plaintext) {
    return null;
  }

  return {
    id: row.id,
    userId: row.user_id,
    key: row.key_plaintext,
    createdAt: row.created_at,
    lastUsedAt: row.last_used_at,
  };
};

/** Create or regenerate the single API key for a user.
 *  Deletes any existing keys first, then creates a new one. */
export const regenerateApiKey = (
  db: Database.Database,
  userId: string,
): ApiKeyInfo => {
  const rawKey = generateApiKey();
  const id = randomBytes(16).toString("hex");

  const createdAt = new Date().toISOString();

  const regenerate = db.transaction(() => {
    db.prepare(`DELETE FROM api_keys WHERE user_id = ?`).run(userId);
    db.prepare(
      `INSERT INTO api_keys (id, user_id, key_plaintext, created_at)
       VALUES (?, ?, ?, ?)`,
    ).run(id, userId, rawKey, createdAt);
  });

  regenerate();

  return {
    id,
    userId,
    key: rawKey,
    createdAt,
    lastUsedAt: null,
  };
};

/** Resolve a raw API key to a user ID (returns null if invalid) */
export const resolveApiKey = (
  db: Database.Database,
  rawKey: string,
): string | null => {
  const row = db
    .prepare(`SELECT user_id FROM api_keys WHERE key_plaintext = ?`)
    .get(rawKey) as { user_id: string } | undefined;

  if (!row) {
    return null;
  }

  db.prepare(
    `UPDATE api_keys SET last_used_at = ? WHERE key_plaintext = ?`,
  ).run(new Date().toISOString(), rawKey);

  return row.user_id;
};
