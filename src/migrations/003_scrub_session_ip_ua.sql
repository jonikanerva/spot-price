-- Scrub historical session IP addresses and user agents to honour
-- VISION.md → Persistence and Privacy Posture ("Not stored: ... IPs beyond
-- what in-memory rate limiting needs"). The src/auth.ts
-- databaseHooks.session.create.before hook keeps new rows clean; this clears
-- any rows written before that hook existed.
--
-- The "ipAddress" / "userAgent" columns are intentionally KEPT (not dropped):
-- better-auth 1.6.11 always emits both fields in its session INSERT, so a
-- DROP COLUMN would break session creation. Nulling the values is sufficient.
UPDATE "session" SET "ipAddress" = NULL, "userAgent" = NULL;
