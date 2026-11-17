-- Read-only release checks of the production database. One SELECT; it
-- changes nothing. Run it like `scripts/schema-snapshot.sql`, in a read-only
-- psql session (STACK.md §15 shows the command).
--
-- Expected output (STACK.md §15):
--
--   typescript_migrations|1,2,3,4,5,6,7
--   server_version|<the PostgreSQL version>
--   user_settings_invalid_timezone|0
--   user_settings_invalid_area|0
--   user_settings_invalid_night_hours|0
--   contract_terms_invalid_night_hours|0
--   user_settings_without_contract_terms|0
--
-- Before migration 4 creates the table `contract_terms`, the last two lines
-- end with `not migrated` in place of the count. After migration 5 drops the
-- night hours of `user_settings`, that line ends with `dropped`.
--
-- The statement fails when the table `_migrations` does not exist.

-- A count is the number of rows that the Rust service rejects; each such
-- user gets 500 from the price routes. A `user_settings` row with an invalid
-- night hour makes migration 4 fail. A `user_settings` row without a
-- `contract_terms` row is a user without a terms version. Print counts only,
-- never a user id or a stored value.

-- `query_to_xml` runs a count only when its table and columns exist: a
-- direct reference fails the whole statement before migration 4 or after
-- migration 5.

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
  SELECT 5, 'user_settings_invalid_night_hours|' || CASE
    WHEN NOT EXISTS (
      SELECT 1 FROM information_schema.columns
      WHERE table_schema = current_schema()
        AND table_name = 'user_settings'
        AND column_name = 'night_start_hour'
    ) THEN 'dropped'
    ELSE coalesce(substring(
      query_to_xml(
        'SELECT count(*) AS n FROM user_settings
         WHERE night_start_hour NOT BETWEEN 0 AND 23
            OR night_end_hour NOT BETWEEN 0 AND 23',
        false, true, ''
      )::text
      FROM '<n>([0-9]+)</n>'
    ), 'unreadable')
  END
  UNION ALL
  SELECT 6, 'contract_terms_invalid_night_hours|' || CASE
    WHEN to_regclass('contract_terms') IS NULL THEN 'not migrated'
    ELSE coalesce(substring(
      query_to_xml(
        'SELECT count(*) AS n FROM contract_terms
         WHERE night_start_hour NOT BETWEEN 0 AND 23
            OR night_end_hour NOT BETWEEN 0 AND 23',
        false, true, ''
      )::text
      FROM '<n>([0-9]+)</n>'
    ), 'unreadable')
  END
  UNION ALL
  SELECT 7, 'user_settings_without_contract_terms|' || CASE
    WHEN to_regclass('contract_terms') IS NULL THEN 'not migrated'
    ELSE coalesce(substring(
      query_to_xml(
        'SELECT count(*) AS n FROM user_settings s
         WHERE NOT EXISTS (SELECT 1 FROM contract_terms t WHERE t.user_id = s.user_id)',
        false, true, ''
      )::text
      FROM '<n>([0-9]+)</n>'
    ), 'unreadable')
  END
)
SELECT line FROM lines ORDER BY position;
