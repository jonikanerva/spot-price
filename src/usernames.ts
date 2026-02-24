import type Database from "better-sqlite3";

const USERNAME_REGEX = /^[a-z0-9_-]{3,32}$/;

export const normalizeUsername = (value: string): string =>
  value.trim().toLowerCase();

export const validateUsername = (value: string): boolean =>
  USERNAME_REGEX.test(normalizeUsername(value));

export const toInternalEmail = (username: string): string =>
  `${normalizeUsername(username)}@local.spot`;

export const getUserIdByUsername = (
  db: Database.Database,
  username: string,
): string | null => {
  const normalized = normalizeUsername(username);
  const row = db
    .prepare("SELECT user_id FROM usernames WHERE username = ?")
    .get(normalized) as { user_id: string } | undefined;
  return row?.user_id ?? null;
};

export const assignUsername = (
  db: Database.Database,
  userId: string,
  username: string,
): void => {
  const normalized = normalizeUsername(username);
  db.prepare(
    "INSERT OR REPLACE INTO usernames (user_id, username) VALUES (?, ?)",
  ).run(userId, normalized);
};

export const getUsernameByUserId = (
  db: Database.Database,
  userId: string,
): string | null => {
  const row = db
    .prepare("SELECT username FROM usernames WHERE user_id = ?")
    .get(userId) as { username: string } | undefined;
  return row?.username ?? null;
};
