-- Hourly spot prices from Nord Pool Data Portal API
CREATE TABLE IF NOT EXISTS prices (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  delivery_start TEXT NOT NULL,
  delivery_end TEXT NOT NULL,
  price_eur_mwh REAL NOT NULL,
  area TEXT NOT NULL DEFAULT 'FI',
  fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE(delivery_start, area)
);

CREATE INDEX IF NOT EXISTS idx_prices_delivery_start ON prices(delivery_start);
CREATE INDEX IF NOT EXISTS idx_prices_area_delivery ON prices(area, delivery_start);
