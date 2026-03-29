import { randomBytes } from "node:crypto";
import type { Pool } from "pg";

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
export const getCurrentApiKey = async (
  pool: Pool,
  userId: string,
): Promise<ApiKeyInfo | null> => {
  const { rows } = await pool.query<ApiKeyRow>(
    `SELECT id, user_id, key_plaintext, created_at, last_used_at
     FROM api_keys WHERE user_id = $1 ORDER BY created_at DESC LIMIT 1`,
    [userId],
  );

  const row = rows[0];
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
export const regenerateApiKey = async (
  pool: Pool,
  userId: string,
): Promise<ApiKeyInfo> => {
  const rawKey = generateApiKey();
  const id = randomBytes(16).toString("hex");
  const createdAt = new Date().toISOString();

  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    await client.query(`DELETE FROM api_keys WHERE user_id = $1`, [userId]);
    await client.query(
      `INSERT INTO api_keys (id, user_id, key_plaintext, created_at)
       VALUES ($1, $2, $3, $4)`,
      [id, userId, rawKey, createdAt],
    );
    await client.query("COMMIT");
  } catch (err) {
    await client.query("ROLLBACK");
    throw err;
  } finally {
    client.release();
  }

  return {
    id,
    userId,
    key: rawKey,
    createdAt,
    lastUsedAt: null,
  };
};

/** Resolve a raw API key to a user ID (returns null if invalid) */
export const resolveApiKey = async (
  pool: Pool,
  rawKey: string,
): Promise<string | null> => {
  const { rows } = await pool.query<{ user_id: string }>(
    `SELECT user_id FROM api_keys WHERE key_plaintext = $1`,
    [rawKey],
  );

  const row = rows[0];
  if (!row) {
    return null;
  }

  await pool.query(
    `UPDATE api_keys SET last_used_at = $1 WHERE key_plaintext = $2`,
    [new Date().toISOString(), rawKey],
  );

  return row.user_id;
};
