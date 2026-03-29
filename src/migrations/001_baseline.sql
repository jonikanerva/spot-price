-- PostgreSQL baseline: all tables from the SQLite era, with proper types.

-- Spot electricity prices from Nord Pool
CREATE TABLE IF NOT EXISTS prices (
  id SERIAL PRIMARY KEY,
  delivery_start TIMESTAMPTZ NOT NULL,
  delivery_end TIMESTAMPTZ NOT NULL,
  price_eur_mwh DOUBLE PRECISION NOT NULL,
  area TEXT NOT NULL DEFAULT 'FI',
  fetched_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  UNIQUE(delivery_start, area)
);

CREATE INDEX IF NOT EXISTS idx_prices_delivery_start ON prices(delivery_start);
CREATE INDEX IF NOT EXISTS idx_prices_area_delivery ON prices(area, delivery_start);

-- User-specific electricity contract settings
CREATE TABLE IF NOT EXISTS user_settings (
  user_id TEXT PRIMARY KEY,
  margin_cents_kwh DOUBLE PRECISION NOT NULL DEFAULT 0.49,
  transfer_day_cents_kwh DOUBLE PRECISION NOT NULL DEFAULT 2.92,
  transfer_night_cents_kwh DOUBLE PRECISION NOT NULL DEFAULT 1.37,
  tax_cents_kwh DOUBLE PRECISION NOT NULL DEFAULT 2.82752,
  vat_percent DOUBLE PRECISION NOT NULL DEFAULT 25.5,
  night_start_hour INTEGER NOT NULL DEFAULT 22,
  night_end_hour INTEGER NOT NULL DEFAULT 7,
  timezone TEXT NOT NULL DEFAULT 'Europe/Helsinki',
  area TEXT NOT NULL DEFAULT 'FI',
  updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- API keys for Home Assistant and other integrations (one per user)
CREATE TABLE IF NOT EXISTS api_keys (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL,
  key_plaintext TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  last_used_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_api_keys_user_id ON api_keys(user_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_api_keys_single_per_user ON api_keys(user_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_api_keys_key_plaintext ON api_keys(key_plaintext);

-- Better Auth core tables
CREATE TABLE IF NOT EXISTS "user" (
  "id" TEXT NOT NULL PRIMARY KEY,
  "name" TEXT NOT NULL,
  "email" TEXT NOT NULL UNIQUE,
  "emailVerified" BOOLEAN NOT NULL,
  "image" TEXT,
  "createdAt" TIMESTAMPTZ NOT NULL,
  "updatedAt" TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS "session" (
  "id" TEXT NOT NULL PRIMARY KEY,
  "expiresAt" TIMESTAMPTZ NOT NULL,
  "token" TEXT NOT NULL UNIQUE,
  "createdAt" TIMESTAMPTZ NOT NULL,
  "updatedAt" TIMESTAMPTZ NOT NULL,
  "ipAddress" TEXT,
  "userAgent" TEXT,
  "userId" TEXT NOT NULL REFERENCES "user" ("id") ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS "account" (
  "id" TEXT NOT NULL PRIMARY KEY,
  "accountId" TEXT NOT NULL,
  "providerId" TEXT NOT NULL,
  "userId" TEXT NOT NULL REFERENCES "user" ("id") ON DELETE CASCADE,
  "accessToken" TEXT,
  "refreshToken" TEXT,
  "idToken" TEXT,
  "accessTokenExpiresAt" TIMESTAMPTZ,
  "refreshTokenExpiresAt" TIMESTAMPTZ,
  "scope" TEXT,
  "password" TEXT,
  "createdAt" TIMESTAMPTZ NOT NULL,
  "updatedAt" TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS "verification" (
  "id" TEXT NOT NULL PRIMARY KEY,
  "identifier" TEXT NOT NULL,
  "value" TEXT NOT NULL,
  "expiresAt" TIMESTAMPTZ NOT NULL,
  "createdAt" TIMESTAMPTZ NOT NULL,
  "updatedAt" TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS "account_userId_idx" ON "account" ("userId");
CREATE INDEX IF NOT EXISTS "session_userId_idx" ON "session" ("userId");
CREATE INDEX IF NOT EXISTS "verification_identifier_idx" ON "verification" ("identifier");

-- Username mapping for username-first login UX
CREATE TABLE IF NOT EXISTS usernames (
  user_id TEXT NOT NULL PRIMARY KEY,
  username TEXT NOT NULL UNIQUE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  FOREIGN KEY (user_id) REFERENCES "user" (id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_usernames_username ON usernames(username);
