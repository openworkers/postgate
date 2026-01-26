-- ============================================================================
-- POSTGATE HELPER FUNCTIONS
-- ============================================================================
--
-- Utility functions accessible to all tenants via qualified names:
--   SELECT * FROM postgate_helpers.list_tables();
--   SELECT * FROM postgate_helpers.describe_table('users');
--

-- ============================================================================
-- SCHEMA
-- ============================================================================

CREATE SCHEMA IF NOT EXISTS postgate_helpers;

COMMENT ON SCHEMA postgate_helpers IS 'Utility functions for tenants (accessible via PostGate)';

-- ============================================================================
-- FUNCTION: list_tables()
-- ============================================================================
-- Lists all tables in the current tenant's schema with row counts.
--
-- Example:
--   SELECT * FROM postgate_helpers.list_tables();

CREATE OR REPLACE FUNCTION postgate_helpers.list_tables()
RETURNS TABLE(table_name text, row_count bigint)
LANGUAGE plpgsql
SECURITY DEFINER
AS $$
DECLARE
    v_schema text;
    tbl record;
    cnt bigint;
BEGIN
    v_schema := current_schema();

    -- Prevent access to system schemas
    IF v_schema IN ('public', 'postgate_helpers') THEN
        RAISE EXCEPTION 'Cannot list tables in system schemas';
    END IF;

    FOR tbl IN
        SELECT tablename
        FROM pg_tables
        WHERE schemaname = v_schema
        ORDER BY tablename
    LOOP
        EXECUTE format('SELECT count(*) FROM %I.%I', v_schema, tbl.tablename) INTO cnt;
        table_name := tbl.tablename;
        row_count := cnt;
        RETURN NEXT;
    END LOOP;
END;
$$;

COMMENT ON FUNCTION postgate_helpers.list_tables() IS 'List all tables in the current tenant schema with row counts';

-- ============================================================================
-- FUNCTION: describe_table(table_name)
-- ============================================================================
-- Describes columns of a table in the current tenant's schema.
--
-- Example:
--   SELECT * FROM postgate_helpers.describe_table('users');

CREATE OR REPLACE FUNCTION postgate_helpers.describe_table(p_table_name text)
RETURNS TABLE(
    column_name text,
    data_type text,
    is_nullable boolean,
    column_default text,
    is_primary_key boolean
)
LANGUAGE plpgsql
SECURITY DEFINER
AS $$
DECLARE
    v_schema text;
BEGIN
    v_schema := current_schema();

    -- Prevent access to system schemas
    IF v_schema IN ('public', 'postgate_helpers') THEN
        RAISE EXCEPTION 'Cannot describe tables in system schemas';
    END IF;

    RETURN QUERY
    SELECT
        c.column_name::text,
        c.data_type::text,
        (c.is_nullable = 'YES')::boolean,
        c.column_default::text,
        COALESCE(
            (SELECT true
             FROM information_schema.table_constraints tc
             JOIN information_schema.key_column_usage kcu
                ON tc.constraint_name = kcu.constraint_name
                AND tc.table_schema = kcu.table_schema
             WHERE tc.constraint_type = 'PRIMARY KEY'
                AND tc.table_schema = v_schema
                AND tc.table_name = p_table_name
                AND kcu.column_name = c.column_name
             LIMIT 1),
            false
        )::boolean
    FROM information_schema.columns c
    WHERE c.table_schema = v_schema
        AND c.table_name = p_table_name
    ORDER BY c.ordinal_position;
END;
$$;

COMMENT ON FUNCTION postgate_helpers.describe_table(text) IS 'Describe columns of a table in the current tenant schema';

-- ============================================================================
-- PERMISSIONS
-- ============================================================================

GRANT USAGE ON SCHEMA postgate_helpers TO PUBLIC;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA postgate_helpers TO PUBLIC;
