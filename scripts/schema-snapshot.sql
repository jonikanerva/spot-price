-- A normalised, read-only snapshot of the application schema: one text line
-- for each relation, column, constraint, index, sequence, trigger, routine,
-- and type in the current schema. The ledger tables `_migrations` and
-- `_sqlx_migrations` are not part of the snapshot.
--
-- This file holds one SELECT statement and changes nothing. Compare a
-- database with the expected snapshot of the TypeScript migrations:
--
--   PGOPTIONS='-c default_transaction_read_only=on' \
--     psql "$URL" -X -q -A -t -f scripts/schema-snapshot.sql \
--     | diff crates/server/tests/fixtures/schema-snapshot.txt -
--
-- NOT NULL constraint rows are left out because PostgreSQL 18 names them
-- after the table name at creation time. The `column` rows show nullability.
WITH ledger(name) AS (
  VALUES ('_migrations'), ('_sqlx_migrations')
),
schema_rels AS (
  SELECT c.oid, c.relname, c.relkind, c.relpersistence
  FROM pg_class c
  JOIN pg_namespace n ON n.oid = c.relnamespace
  WHERE n.nspname = current_schema()
),
tables AS (
  SELECT oid, relname
  FROM schema_rels
  WHERE relkind IN ('r', 'p', 'v', 'm', 'f')
    AND relname NOT IN (SELECT name FROM ledger)
),
ledger_oids AS (
  SELECT oid FROM schema_rels WHERE relname IN (SELECT name FROM ledger)
),
lines(line) AS (
  SELECT concat_ws('|', 'relation', r.relkind, r.relpersistence, r.relname)
  FROM schema_rels r
  WHERE r.relname NOT IN (SELECT name FROM ledger)
    AND r.relkind <> 'c'
    AND NOT EXISTS (
      SELECT 1 FROM pg_index i
      WHERE i.indexrelid = r.oid AND i.indrelid IN (SELECT oid FROM ledger_oids)
    )
    AND NOT EXISTS (
      SELECT 1 FROM pg_depend d
      WHERE d.objid = r.oid AND d.classid = 'pg_class'::regclass
        AND d.refobjid IN (SELECT oid FROM ledger_oids) AND r.relkind = 'S'
    )

  UNION ALL
  SELECT concat_ws('|', 'column', t.relname,
    lpad(row_number() OVER (PARTITION BY t.oid ORDER BY a.attnum)::text, 3, '0'),
    a.attname,
    format_type(a.atttypid, a.atttypmod),
    CASE WHEN a.attnotnull THEN 'not null' ELSE 'null' END,
    coalesce(pg_get_expr(ad.adbin, ad.adrelid), ''),
    a.attidentity,
    a.attgenerated,
    coalesce(co.collname, ''))
  FROM tables t
  JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum > 0 AND NOT a.attisdropped
  LEFT JOIN pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum
  LEFT JOIN pg_collation co ON co.oid = a.attcollation AND co.collname <> 'default'

  UNION ALL
  SELECT concat_ws('|', 'constraint', t.relname, c.conname, c.contype,
    pg_get_constraintdef(c.oid, true))
  FROM tables t
  JOIN pg_constraint c ON c.conrelid = t.oid
  WHERE c.contype <> 'n'

  UNION ALL
  SELECT concat_ws('|', 'index', t.relname, ic.relname, pg_get_indexdef(i.indexrelid))
  FROM tables t
  JOIN pg_index i ON i.indrelid = t.oid
  JOIN pg_class ic ON ic.oid = i.indexrelid

  UNION ALL
  SELECT concat_ws('|', 'sequence', r.relname,
    format_type(s.seqtypid, NULL),
    s.seqstart::text, s.seqincrement::text, s.seqmin::text, s.seqmax::text,
    s.seqcache::text, s.seqcycle::text,
    coalesce((
      SELECT ot.relname || '.' || oa.attname
      FROM pg_depend d
      JOIN pg_class ot ON ot.oid = d.refobjid
      JOIN pg_attribute oa ON oa.attrelid = d.refobjid AND oa.attnum = d.refobjsubid
      WHERE d.classid = 'pg_class'::regclass AND d.objid = r.oid
        AND d.refclassid = 'pg_class'::regclass AND d.deptype IN ('a', 'i')
    ), ''))
  FROM schema_rels r
  JOIN pg_sequence s ON s.seqrelid = r.oid
  WHERE r.relkind = 'S'
    AND NOT EXISTS (
      SELECT 1 FROM pg_depend d
      WHERE d.objid = r.oid AND d.classid = 'pg_class'::regclass
        AND d.refobjid IN (SELECT oid FROM ledger_oids)
    )

  UNION ALL
  SELECT concat_ws('|', 'trigger', t.relname, tg.tgname, pg_get_triggerdef(tg.oid, true))
  FROM tables t
  JOIN pg_trigger tg ON tg.tgrelid = t.oid
  WHERE NOT tg.tgisinternal

  UNION ALL
  SELECT concat_ws('|', 'routine', p.proname, pg_get_function_identity_arguments(p.oid), p.prokind)
  FROM pg_proc p
  JOIN pg_namespace n ON n.oid = p.pronamespace
  WHERE n.nspname = current_schema()

  UNION ALL
  SELECT concat_ws('|', 'type', ty.typname, ty.typtype)
  FROM pg_type ty
  JOIN pg_namespace n ON n.oid = ty.typnamespace
  WHERE n.nspname = current_schema()
    AND ty.typtype IN ('d', 'e', 'r', 'm')
)
SELECT line FROM lines ORDER BY line COLLATE "C";
