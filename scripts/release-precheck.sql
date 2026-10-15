-- Read-only release checks of the production database. One SELECT; it
-- changes nothing. Run it like `scripts/schema-snapshot.sql`:
--
--   PGOPTIONS='-c default_transaction_read_only=on' \
--     psql "$DATABASE_PUBLIC_URL" -X -q -A -t -f scripts/release-precheck.sql
--
-- Expected output (STACK.md §15):
--
--   typescript_migrations|1,2,3,4,5,6,7
--   server_version|<the PostgreSQL version>
--   user_settings_invalid_timezone|0
--   user_settings_invalid_area|0
--   user_settings_invalid_night_hours|0
--
-- The statement fails when the table `_migrations` does not exist.

-- A count is the number of `user_settings` rows that the Rust service
-- rejects; each such user gets 500 from the price routes. Print counts only,
-- never a user id or a stored value.

-- The lists must equal the time zones and area codes of
-- crates/domain/src/areas.rs.
WITH supported_timezones(name) AS (
  VALUES
    ('Europe/Amsterdam'), ('Europe/Berlin'), ('Europe/Brussels'),
    ('Europe/Copenhagen'), ('Europe/Helsinki'), ('Europe/Oslo'),
    ('Europe/Paris'), ('Europe/Riga'), ('Europe/Stockholm'),
    ('Europe/Tallinn'), ('Europe/Vienna'), ('Europe/Vilnius'),
    ('Europe/Warsaw')
),
supported_areas(code) AS (
  VALUES
    ('FI'), ('SE1'), ('SE2'), ('SE3'), ('SE4'),
    ('NO1'), ('NO2'), ('NO3'), ('NO4'), ('NO5'),
    ('DK1'), ('DK2'), ('EE'), ('LT'), ('LV'),
    ('AT'), ('BE'), ('FR'), ('GER'), ('NL'), ('PL')
),
lines(position, line) AS (
  SELECT 1, 'typescript_migrations|'
    || coalesce((SELECT string_agg(version::text, ',' ORDER BY version) FROM _migrations), '')
  UNION ALL
  SELECT 2, 'server_version|' || current_setting('server_version')
  UNION ALL
  SELECT 3, 'user_settings_invalid_timezone|' || count(*)
  FROM user_settings
  WHERE timezone NOT IN (SELECT name FROM supported_timezones)
  UNION ALL
  SELECT 4, 'user_settings_invalid_area|' || count(*)
  FROM user_settings
  WHERE area NOT IN (SELECT code FROM supported_areas)
  UNION ALL
  SELECT 5, 'user_settings_invalid_night_hours|' || count(*)
  FROM user_settings
  WHERE night_start_hour NOT BETWEEN 0 AND 23
     OR night_end_hour NOT BETWEEN 0 AND 23
)
SELECT line FROM lines ORDER BY position;
