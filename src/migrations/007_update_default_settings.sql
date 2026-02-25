-- Update default values for new user settings to reflect typical Finnish contract
-- This only changes the table defaults; existing users keep their current values.
-- SQLite doesn't support ALTER COLUMN DEFAULT, so we recreate the table.

CREATE TABLE IF NOT EXISTS user_settings_new (
  user_id TEXT PRIMARY KEY,
  margin_cents_kwh REAL NOT NULL DEFAULT 0.49,
  transfer_day_cents_kwh REAL NOT NULL DEFAULT 2.92,
  transfer_night_cents_kwh REAL NOT NULL DEFAULT 1.37,
  tax_cents_kwh REAL NOT NULL DEFAULT 2.82752,
  vat_percent REAL NOT NULL DEFAULT 25.5,
  night_start_hour INTEGER NOT NULL DEFAULT 22,
  night_end_hour INTEGER NOT NULL DEFAULT 7,
  timezone TEXT NOT NULL DEFAULT 'Europe/Helsinki',
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT OR IGNORE INTO user_settings_new
  SELECT * FROM user_settings;

DROP TABLE user_settings;

ALTER TABLE user_settings_new RENAME TO user_settings;
