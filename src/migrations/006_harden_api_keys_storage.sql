CREATE UNIQUE INDEX IF NOT EXISTS idx_api_keys_single_per_user ON api_keys(user_id);
