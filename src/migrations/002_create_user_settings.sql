-- User-specific electricity contract settings
CREATE TABLE IF NOT EXISTS user_settings (
  user_id TEXT PRIMARY KEY,
  margin_cents_kwh REAL NOT NULL DEFAULT 0.0,
  transfer_day_cents_kwh REAL NOT NULL DEFAULT 0.0,
  transfer_night_cents_kwh REAL NOT NULL DEFAULT 0.0,
  tax_cents_kwh REAL NOT NULL DEFAULT 2.79372,
  vat_percent REAL NOT NULL DEFAULT 25.5,
  night_start_hour INTEGER NOT NULL DEFAULT 22,
  night_end_hour INTEGER NOT NULL DEFAULT 7,
  timezone TEXT NOT NULL DEFAULT 'Europe/Helsinki',
  updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
