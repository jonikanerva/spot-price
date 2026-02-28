CREATE TABLE api_keys_new (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL,
  key_plaintext TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  last_used_at TEXT
);

INSERT INTO api_keys_new (id, user_id, key_plaintext, created_at, last_used_at)
WITH ranked AS (
  SELECT
    id,
    user_id,
    key_plaintext,
    created_at,
    last_used_at,
    ROW_NUMBER() OVER (
      PARTITION BY user_id
      ORDER BY datetime(created_at) DESC, id DESC
    ) AS row_number
  FROM api_keys
  WHERE key_plaintext IS NOT NULL
)
SELECT id, user_id, key_plaintext, created_at, last_used_at
FROM ranked
WHERE row_number = 1;

DROP TABLE api_keys;
ALTER TABLE api_keys_new RENAME TO api_keys;

CREATE INDEX IF NOT EXISTS idx_api_keys_user_id ON api_keys(user_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_api_keys_single_per_user ON api_keys(user_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_api_keys_key_plaintext ON api_keys(key_plaintext);
