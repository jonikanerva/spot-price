-- Username mapping for username-first login UX
CREATE TABLE IF NOT EXISTS usernames (
  user_id TEXT NOT NULL PRIMARY KEY,
  username TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  FOREIGN KEY (user_id) REFERENCES "user" (id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_usernames_username ON usernames(username);
