import { randomBytes, createHash } from "node:crypto";
import type Database from "better-sqlite3";

const KEY_PREFIX = "sp_";
const KEY_BYTE_LENGTH = 32;

/** Generate a random API key with prefix */
export const generateApiKey = (): string => {
  const bytes = randomBytes(KEY_BYTE_LENGTH);
  return `${KEY_PREFIX}${bytes.toString("hex")}`;
};

/** Hash an API key for auth lookup (SHA-256) */
export const hashApiKey = (key: string): string =>
  createHash("sha256").update(key).digest("hex");

export interface ApiKeyInfo {
  readonly id: string;
  readonly userId: string;
  readonly key: string | null;
  readonly createdAt: string;
  readonly lastUsedAt: string | null;
}

interface ApiKeyRow {
  readonly id: string;
  readonly user_id: string;
  readonly key_hash: string;
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
      `SELECT id, user_id, key_hash, key_plaintext, created_at, last_used_at
       FROM api_keys WHERE user_id = ? ORDER BY created_at DESC LIMIT 1`,
    )
    .get(userId) as ApiKeyRow | undefined;

  if (!row) {
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
  const keyHash = hashApiKey(rawKey);
  const id = randomBytes(16).toString("hex");

  const createdAt = new Date().toISOString();

  const regenerate = db.transaction(() => {
    db.prepare(`DELETE FROM api_keys WHERE user_id = ?`).run(userId);
    db.prepare(
      `INSERT INTO api_keys (id, user_id, key_hash, key_plaintext, name, created_at)
       VALUES (?, ?, ?, ?, 'default', ?)`,
    ).run(id, userId, keyHash, rawKey, createdAt);
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
  const keyHash = hashApiKey(rawKey);
  const row = db
    .prepare(`SELECT user_id FROM api_keys WHERE key_hash = ?`)
    .get(keyHash) as { user_id: string } | undefined;

  if (!row) {
    return null;
  }

  db.prepare(`UPDATE api_keys SET last_used_at = ? WHERE key_hash = ?`).run(
    new Date().toISOString(),
    keyHash,
  );

  return row.user_id;
};
