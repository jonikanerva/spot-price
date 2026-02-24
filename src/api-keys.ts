import { randomBytes, createHash } from "node:crypto";
import type Database from "better-sqlite3";

const KEY_PREFIX = "sp_";
const KEY_BYTE_LENGTH = 32;

/** Generate a random API key with prefix */
export const generateApiKey = (): string => {
  const bytes = randomBytes(KEY_BYTE_LENGTH);
  return `${KEY_PREFIX}${bytes.toString("hex")}`;
};

/** Hash an API key for storage (SHA-256) */
export const hashApiKey = (key: string): string =>
  createHash("sha256").update(key).digest("hex");

interface ApiKeyRow {
  id: string;
  user_id: string;
  key_hash: string;
  name: string;
  created_at: string;
  last_used_at: string | null;
}

export interface ApiKeyInfo {
  readonly id: string;
  readonly userId: string;
  readonly name: string;
  readonly createdAt: string;
  readonly lastUsedAt: string | null;
}

/** Create a new API key — returns the raw key (shown once) and stored info */
export const createApiKey = (
  db: Database.Database,
  userId: string,
  name: string,
): { readonly rawKey: string; readonly keyInfo: ApiKeyInfo } => {
  const rawKey = generateApiKey();
  const keyHash = hashApiKey(rawKey);
  const id = randomBytes(16).toString("hex");

  db.prepare(
    `INSERT INTO api_keys (id, user_id, key_hash, name) VALUES (?, ?, ?, ?)`,
  ).run(id, userId, keyHash, name);

  return {
    rawKey,
    keyInfo: {
      id,
      userId,
      name,
      createdAt: new Date().toISOString(),
      lastUsedAt: null,
    },
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

  // Update last_used_at
  db.prepare(
    `UPDATE api_keys SET last_used_at = datetime('now') WHERE key_hash = ?`,
  ).run(keyHash);

  return row.user_id;
};

/** List all API keys for a user (without hashes) */
export const listApiKeys = (
  db: Database.Database,
  userId: string,
): readonly ApiKeyInfo[] => {
  const rows = db
    .prepare(
      `SELECT id, user_id, name, created_at, last_used_at
       FROM api_keys WHERE user_id = ? ORDER BY created_at DESC`,
    )
    .all(userId) as readonly ApiKeyRow[];

  return rows.map(
    (r): ApiKeyInfo => ({
      id: r.id,
      userId: r.user_id,
      name: r.name,
      createdAt: r.created_at,
      lastUsedAt: r.last_used_at,
    }),
  );
};

/** Delete an API key (returns true if deleted) */
export const deleteApiKey = (
  db: Database.Database,
  keyId: string,
  userId: string,
): boolean => {
  const result = db
    .prepare(`DELETE FROM api_keys WHERE id = ? AND user_id = ?`)
    .run(keyId, userId);
  return result.changes > 0;
};
