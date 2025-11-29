-- ============================================================================
-- POSTGATE DATABASE SCHEMA
-- ============================================================================
--
-- Multi-tenant PostgreSQL proxy with token-based authentication.
--
-- This schema provides:
-- - Database registration (schema or dedicated backends)
-- - Token management with SHA-256 hashing
-- - PL/pgSQL functions for all administrative operations
--
-- Architecture:
-- - Each tenant gets an isolated PostgreSQL schema
-- - Tokens are hashed with SHA-256 (never stored in plain text)
-- - Permissions are per-token (SELECT, INSERT, UPDATE, DELETE, CREATE, ALTER, DROP)
--
-- ============================================================================

-- ============================================================================
-- EXTENSIONS
-- ============================================================================

-- pgcrypto is required for secure token generation
-- Provides: gen_random_bytes(), digest() for SHA-256 hashing
CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- ============================================================================
-- DATABASES TABLE
-- ============================================================================
--
-- Stores registered databases. Each database represents a tenant.
--
-- Backend Types:
-- - 'schema': Isolated PostgreSQL schema (shared connection pool)
-- - 'dedicated': External PostgreSQL connection string (premium)
--
-- Security: max_rows limits query results to prevent memory exhaustion
--

CREATE TABLE postgate_databases (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name character varying(100) NOT NULL,

    -- Backend type: determines how queries are executed
    backend_type character varying(20) NOT NULL DEFAULT 'schema',

    -- For 'schema' backend: the PostgreSQL schema name
    -- Format: tenant_<uuid>_<sanitized_name>
    schema_name character varying(100),

    -- For 'dedicated' backend: full PostgreSQL connection string
    -- Example: postgres://user:pass@host:5432/database
    connection_string text,

    -- Maximum rows returned per query (prevents memory exhaustion)
    max_rows integer NOT NULL DEFAULT 1000,

    created_at timestamp with time zone DEFAULT NOW() NOT NULL,

    -- Ensure backend configuration is valid
    CONSTRAINT valid_backend CHECK (
        (backend_type = 'schema' AND schema_name IS NOT NULL) OR
        (backend_type = 'dedicated' AND connection_string IS NOT NULL)
    ),

    -- Schema names must be unique
    CONSTRAINT unique_schema UNIQUE (schema_name)
);

-- ============================================================================
-- TOKENS TABLE
-- ============================================================================
--
-- Stores API tokens for database access.
--
-- Token Format: pg_<64_hex_chars> (67 chars total)
-- - Prefix: "pg_" for identification
-- - Body: 32 random bytes, hex-encoded (64 chars)
--
-- Security:
-- - Tokens are NEVER stored in plain text
-- - Only SHA-256 hash is stored (cannot be reversed)
-- - token_prefix stores first 8 chars for UI identification
-- - Full token is returned ONLY at creation time
--
-- Permissions:
-- - SELECT: Read data
-- - INSERT: Create new rows
-- - UPDATE: Modify existing rows
-- - DELETE: Remove rows
-- - CREATE: Create tables, indexes, views (DDL)
-- - ALTER: Modify table structure (DDL)
-- - DROP: Drop tables, truncate data (DDL)
--

CREATE TABLE postgate_tokens (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),

    -- Foreign key to database (cascades on delete)
    database_id uuid NOT NULL REFERENCES postgate_databases(id) ON DELETE CASCADE,

    -- Human-readable name (e.g., "production", "readonly", "migrations")
    name character varying(100) NOT NULL DEFAULT 'default',

    -- SHA-256 hash of the full token (hex encoded, 64 chars)
    token_hash character varying(64) NOT NULL,

    -- First 8 characters for identification (e.g., "pg_a1b2c")
    token_prefix character varying(8) NOT NULL,

    -- Array of allowed SQL operations
    -- Default: DML only (safe for applications)
    -- Tenant: DML + DDL (full control)
    allowed_operations text[] NOT NULL DEFAULT ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE'],

    created_at timestamp with time zone DEFAULT NOW() NOT NULL,

    -- Tracks last usage for auditing and cleanup
    last_used_at timestamp with time zone,

    -- Each database can only have one token with the same name
    UNIQUE (database_id, name)
);

-- Index for fast token validation (most common operation)
CREATE INDEX idx_postgate_tokens_hash ON postgate_tokens(token_hash);

-- Index for listing tokens by database
CREATE INDEX idx_postgate_tokens_database_id ON postgate_tokens(database_id);

-- ============================================================================
-- TENANT MANAGEMENT FUNCTIONS
-- ============================================================================

-- ----------------------------------------------------------------------------
-- create_tenant_database(name, max_rows)
-- ----------------------------------------------------------------------------
-- Creates a new tenant with an isolated PostgreSQL schema.
--
-- Parameters:
--   p_name: Human-readable database name (used in schema name)
--   p_max_rows: Maximum rows per query (default: 1000)
--
-- Returns:
--   id: UUID of the new database
--   schema_name: Generated schema name (tenant_<uuid>_<name>)
--
-- Side Effects:
--   - Creates a new PostgreSQL schema
--   - Inserts record into postgate_databases
--
-- Example:
--   SELECT * FROM create_tenant_database('my_app', 5000);
--   -- Returns: { id: "abc-123...", schema_name: "tenant_def456_my_app" }
--

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
    -- Generate unique schema name: tenant_<random_uuid>_<sanitized_name>
    -- UUID ensures uniqueness, name provides human readability
    v_schema_name := 'tenant_' || REPLACE(gen_random_uuid()::text, '-', '') || '_' || REPLACE(p_name, '-', '_');

    -- Create the PostgreSQL schema for isolation
    EXECUTE format('CREATE SCHEMA IF NOT EXISTS %I', v_schema_name);

    -- Insert database record
    INSERT INTO postgate_databases (name, backend_type, schema_name, max_rows)
    VALUES (p_name, 'schema', v_schema_name, p_max_rows)
    RETURNING postgate_databases.id INTO v_id;

    RETURN QUERY SELECT v_id, v_schema_name;
END;
$$ LANGUAGE plpgsql;

-- ----------------------------------------------------------------------------
-- delete_tenant_database(database_id)
-- ----------------------------------------------------------------------------
-- Deletes a tenant database and drops its schema.
--
-- Parameters:
--   p_database_id: UUID of the database to delete
--
-- Returns:
--   boolean: true if deleted, false if not found
--
-- Side Effects:
--   - Drops the PostgreSQL schema (CASCADE - all tables!)
--   - Deletes record from postgate_databases
--   - Cascades to delete all tokens for this database
--
-- Example:
--   SELECT delete_tenant_database('abc-123...'::uuid);
--   -- Returns: true
--

CREATE OR REPLACE FUNCTION delete_tenant_database(
    p_database_id uuid
) RETURNS boolean AS $$
DECLARE
    v_schema_name character varying(100);
    v_backend_type character varying(20);
BEGIN
    -- Get current schema info before deletion
    SELECT schema_name, backend_type INTO v_schema_name, v_backend_type
    FROM postgate_databases
    WHERE id = p_database_id;

    IF NOT FOUND THEN
        RETURN FALSE;
    END IF;

    -- Only drop schema for schema-based backends
    -- Dedicated backends use external databases
    IF v_backend_type = 'schema' AND v_schema_name IS NOT NULL THEN
        EXECUTE format('DROP SCHEMA IF EXISTS %I CASCADE', v_schema_name);
    END IF;

    -- Delete database record (tokens cascade automatically)
    DELETE FROM postgate_databases WHERE id = p_database_id;

    RETURN TRUE;
END;
$$ LANGUAGE plpgsql;

-- ============================================================================
-- TOKEN MANAGEMENT FUNCTIONS
-- ============================================================================

-- ----------------------------------------------------------------------------
-- create_tenant_token(database_id, name, permissions)
-- ----------------------------------------------------------------------------
-- Creates an API token for database access.
--
-- IMPORTANT: The full token is returned ONLY at creation time!
-- It cannot be retrieved later (only the hash is stored).
--
-- Parameters:
--   p_database_id: UUID of the database
--   p_name: Human-readable token name (default: 'default')
--   p_permissions: Array of allowed operations (default: DML only)
--
-- Returns:
--   id: UUID of the new token
--   token: Full token string (pg_xxx...) - SAVE THIS!
--
-- Token Format:
--   pg_<64_hex_chars> (67 characters total)
--   - "pg_" prefix for identification
--   - 32 random bytes (64 hex characters)
--
-- Example (DML only - safe for applications):
--   SELECT * FROM create_tenant_token(
--       'abc-123...'::uuid,
--       'production',
--       ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']
--   );
--
-- Example (Full tenant access - includes DDL):
--   SELECT * FROM create_tenant_token(
--       'abc-123...'::uuid,
--       'migrations',
--       ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE', 'CREATE', 'ALTER', 'DROP']
--   );
--

CREATE OR REPLACE FUNCTION create_tenant_token(
    p_database_id uuid,
    p_name character varying(100) DEFAULT 'default',
    p_permissions text[] DEFAULT ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']
) RETURNS TABLE (
    id uuid,
    token text
) AS $$
DECLARE
    v_id uuid;
    v_token_bytes bytea;
    v_token_hex text;
    v_full_token text;
    v_token_hash text;
    v_token_prefix text;
BEGIN
    -- Verify database exists
    IF NOT EXISTS (SELECT 1 FROM postgate_databases WHERE postgate_databases.id = p_database_id) THEN
        RAISE EXCEPTION 'Database not found: %', p_database_id;
    END IF;

    -- Generate 32 cryptographically secure random bytes
    v_token_bytes := gen_random_bytes(32);
    v_token_hex := encode(v_token_bytes, 'hex');

    -- Build token: pg_ prefix + 64 hex chars = 67 chars total
    v_token_prefix := 'pg_' || substring(v_token_hex from 1 for 5);
    v_full_token := 'pg_' || v_token_hex;

    -- Hash with SHA-256 (this is what gets stored)
    v_token_hash := encode(digest(v_full_token, 'sha256'), 'hex');

    -- Insert token record (only hash is stored)
    INSERT INTO postgate_tokens (database_id, name, token_hash, token_prefix, allowed_operations)
    VALUES (p_database_id, p_name, v_token_hash, v_token_prefix, p_permissions)
    RETURNING postgate_tokens.id INTO v_id;

    -- Return the full token - THIS IS THE ONLY TIME IT'S AVAILABLE!
    RETURN QUERY SELECT v_id, v_full_token;
END;
$$ LANGUAGE plpgsql;

-- ----------------------------------------------------------------------------
-- delete_tenant_token(token_id)
-- ----------------------------------------------------------------------------
-- Deletes an API token by its ID.
--
-- Parameters:
--   p_token_id: UUID of the token to delete
--
-- Returns:
--   boolean: true if deleted, false if not found
--
-- Example:
--   SELECT delete_tenant_token('xyz-789...'::uuid);
--   -- Returns: true
--

CREATE OR REPLACE FUNCTION delete_tenant_token(
    p_token_id uuid
) RETURNS boolean AS $$
BEGIN
    DELETE FROM postgate_tokens WHERE id = p_token_id;
    RETURN FOUND;
END;
$$ LANGUAGE plpgsql;

-- ============================================================================
-- SEED DATA
-- ============================================================================
--
-- Default databases for system operation.
-- You must create tokens for these manually after first run.
--

-- ----------------------------------------------------------------------------
-- Admin Database (UUID: 00000000-0000-0000-0000-000000000000)
-- ----------------------------------------------------------------------------
-- Purpose: Administrative operations (create tenants, tokens)
-- Schema: public (access to postgate_* tables and functions)
--
-- Create admin token after setup:
--   SELECT * FROM create_tenant_token(
--       '00000000-0000-0000-0000-000000000000'::uuid,
--       'admin',
--       ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE', 'CREATE', 'ALTER', 'DROP']
--   );
--

INSERT INTO postgate_databases (id, name, backend_type, schema_name, max_rows)
VALUES (
    '00000000-0000-0000-0000-000000000000',
    'postgate_admin',
    'schema',
    'public',
    1000
);

-- ----------------------------------------------------------------------------
-- OpenWorkers Database (UUID: 00000000-0000-0000-0000-000000000001)
-- ----------------------------------------------------------------------------
-- Purpose: Dedicated connection to OpenWorkers API database
-- Type: Dedicated (external connection string)
--
-- Note: Update the connection string for your environment!
--

INSERT INTO postgate_databases (id, name, backend_type, connection_string, max_rows)
VALUES (
    '00000000-0000-0000-0000-000000000001',
    'openworkers',
    'dedicated',
    'postgres://openworkers:password@localhost/openworkers',
    10000
);
