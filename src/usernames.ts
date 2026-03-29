import type { Pool } from "pg";

const USERNAME_REGEX = /^[a-z0-9_-]{3,32}$/;

export const normalizeUsername = (value: string): string =>
  value.trim().toLowerCase();

export const validateUsername = (value: string): boolean =>
  USERNAME_REGEX.test(normalizeUsername(value));

export const toInternalEmail = (username: string): string =>
  `${normalizeUsername(username)}@local.spot`;

export const getUserIdByUsername = async (
  pool: Pool,
  username: string,
): Promise<string | null> => {
  const normalized = normalizeUsername(username);
  const { rows } = await pool.query<{ user_id: string }>(
    "SELECT user_id FROM usernames WHERE username = $1",
    [normalized],
  );
  return rows[0]?.user_id ?? null;
};

export const assignUsername = async (
  pool: Pool,
  userId: string,
  username: string,
): Promise<void> => {
  const normalized = normalizeUsername(username);
  await pool.query(
    `INSERT INTO usernames (user_id, username) VALUES ($1, $2)
     ON CONFLICT (user_id) DO UPDATE SET username = EXCLUDED.username`,
    [userId, normalized],
  );
};

export const getUsernameByUserId = async (
  pool: Pool,
  userId: string,
): Promise<string | null> => {
  const { rows } = await pool.query<{ username: string }>(
    "SELECT username FROM usernames WHERE user_id = $1",
    [userId],
  );
  return rows[0]?.username ?? null;
};
