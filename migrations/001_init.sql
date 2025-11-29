--
-- Postgate Database Schema
-- Multi-tenant PostgreSQL proxy with token-based authentication
--

-- ============================================================================
-- DATABASES TABLE
-- ============================================================================

CREATE TABLE postgate_databases (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name character varying(100) NOT NULL,

    -- Backend type: 'schema' or 'dedicated'
    backend_type character varying(20) NOT NULL DEFAULT 'schema',

    -- For 'schema' backend
    schema_name character varying(100),

    -- For 'dedicated' backend
    connection_string text,

    -- Limits (max_rows only, operations are on tokens)
    max_rows integer NOT NULL DEFAULT 1000,

    created_at timestamp with time zone DEFAULT NOW() NOT NULL,

    CONSTRAINT valid_backend CHECK (
        (backend_type = 'schema' AND schema_name IS NOT NULL) OR
        (backend_type = 'dedicated' AND connection_string IS NOT NULL)
    ),

    CONSTRAINT unique_schema UNIQUE (schema_name)
);

-- ============================================================================
-- TOKENS TABLE
-- ============================================================================

CREATE TABLE postgate_tokens (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    database_id uuid NOT NULL REFERENCES postgate_databases(id) ON DELETE CASCADE,
    name character varying(100) NOT NULL DEFAULT 'default',
    token_hash character varying(64) NOT NULL,  -- SHA-256 hash (hex encoded)
    token_prefix character varying(8) NOT NULL,  -- First 8 chars for identification (pg_xxxx)

    -- Allowed operations for this token
    allowed_operations text[] NOT NULL DEFAULT ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE'],

    created_at timestamp with time zone DEFAULT NOW() NOT NULL,
    last_used_at timestamp with time zone,
    UNIQUE (database_id, name)
);

CREATE INDEX idx_postgate_tokens_hash ON postgate_tokens(token_hash);
CREATE INDEX idx_postgate_tokens_database_id ON postgate_tokens(database_id);

-- ============================================================================
-- TENANT MANAGEMENT FUNCTIONS
-- ============================================================================

-- Function to create a new tenant database (schema)
CREATE OR REPLACE FUNCTION create_tenant_database(
    p_name character varying(100),
    p_max_rows integer DEFAULT 1000
) RETURNS TABLE (
    id uuid,
    schema_name character varying(100)
) AS $$
DECLARE
    v_id uuid;
    v_schema_name character varying(100);
BEGIN
    -- Generate unique schema name
    v_schema_name := 'tenant_' || REPLACE(gen_random_uuid()::text, '-', '') || '_' || REPLACE(p_name, '-', '_');

    -- Create the schema
    EXECUTE format('CREATE SCHEMA IF NOT EXISTS %I', v_schema_name);

    -- Insert into postgate_databases
    INSERT INTO postgate_databases (name, backend_type, schema_name, max_rows)
    VALUES (p_name, 'schema', v_schema_name, p_max_rows)
    RETURNING postgate_databases.id INTO v_id;

    RETURN QUERY SELECT v_id, v_schema_name;
END;
$$ LANGUAGE plpgsql;

-- Function to delete a tenant database (schema)
CREATE OR REPLACE FUNCTION delete_tenant_database(
    p_database_id uuid
) RETURNS boolean AS $$
DECLARE
    v_schema_name character varying(100);
    v_backend_type character varying(20);
BEGIN
    -- Get schema info
    SELECT schema_name, backend_type INTO v_schema_name, v_backend_type
    FROM postgate_databases
    WHERE id = p_database_id;

    IF NOT FOUND THEN
        RETURN FALSE;
    END IF;

    -- Only drop schema for schema-based backends
    IF v_backend_type = 'schema' AND v_schema_name IS NOT NULL THEN
        EXECUTE format('DROP SCHEMA IF EXISTS %I CASCADE', v_schema_name);
    END IF;

    -- Delete from postgate_databases (tokens will cascade delete)
    DELETE FROM postgate_databases WHERE id = p_database_id;

    RETURN TRUE;
END;
$$ LANGUAGE plpgsql;

-- ============================================================================
-- SEED DATA: ADMIN AND OPENWORKERS DATABASES
-- ============================================================================

-- Admin database: access to public schema for tenant management
INSERT INTO postgate_databases (id, name, backend_type, schema_name, max_rows)
VALUES (
    '00000000-0000-0000-0000-000000000000',
    'postgate_admin',
    'schema',
    'public',
    1000
);

-- OpenWorkers database: dedicated connection to external openworkers database
INSERT INTO postgate_databases (id, name, backend_type, connection_string, max_rows)
VALUES (
    '00000000-0000-0000-0000-000000000001',
    'openworkers',
    'dedicated',
    'postgres://openworkers:password@localhost/openworkers',
    10000
);
