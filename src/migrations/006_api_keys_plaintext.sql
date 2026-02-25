-- Add plaintext column for single-key-per-user model (key always visible in UI)
ALTER TABLE api_keys ADD COLUMN key_plaintext TEXT;
