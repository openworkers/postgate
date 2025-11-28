-- Function to create a new tenant database (schema)
CREATE OR REPLACE FUNCTION create_tenant_database(
    p_user_id UUID,
    p_name VARCHAR(100),
    p_allowed_operations TEXT[] DEFAULT ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE'],
    p_max_rows INTEGER DEFAULT 1000,
    p_timeout_seconds INTEGER DEFAULT 30
) RETURNS TABLE (
    id UUID,
    schema_name VARCHAR(100)
) AS $$
DECLARE
    v_id UUID;
    v_schema_name VARCHAR(100);
    v_rules JSONB;
BEGIN
    -- Generate unique schema name
    v_schema_name := 'tenant_' || REPLACE(p_user_id::TEXT, '-', '') || '_' || REPLACE(p_name, '-', '_');

    -- Build rules JSON
    v_rules := jsonb_build_object(
        'allowed_operations', to_jsonb(p_allowed_operations),
        'max_rows', p_max_rows,
        'timeout_seconds', p_timeout_seconds
    );

    -- Create the schema
    EXECUTE format('CREATE SCHEMA IF NOT EXISTS %I', v_schema_name);

    -- Insert into postgate_databases
    INSERT INTO postgate_databases (user_id, name, backend_type, schema_name, rules)
    VALUES (p_user_id, p_name, 'schema', v_schema_name, v_rules)
    RETURNING postgate_databases.id INTO v_id;

    RETURN QUERY SELECT v_id, v_schema_name;
END;
$$ LANGUAGE plpgsql;

-- Function to delete a tenant database (schema)
CREATE OR REPLACE FUNCTION delete_tenant_database(
    p_database_id UUID
) RETURNS BOOLEAN AS $$
DECLARE
    v_schema_name VARCHAR(100);
    v_backend_type VARCHAR(20);
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

    -- Delete from postgate_databases
    DELETE FROM postgate_databases WHERE id = p_database_id;

    RETURN TRUE;
END;
$$ LANGUAGE plpgsql;

-- Function to list tenant databases for a user
CREATE OR REPLACE FUNCTION list_tenant_databases(
    p_user_id UUID
) RETURNS TABLE (
    id UUID,
    name VARCHAR(100),
    schema_name VARCHAR(100),
    created_at TIMESTAMPTZ
) AS $$
BEGIN
    RETURN QUERY
    SELECT d.id, d.name, d.schema_name, d.created_at
    FROM postgate_databases d
    WHERE d.user_id = p_user_id
    ORDER BY d.created_at DESC;
END;
$$ LANGUAGE plpgsql;

-- Function to get a specific tenant database
CREATE OR REPLACE FUNCTION get_tenant_database(
    p_database_id UUID
) RETURNS TABLE (
    id UUID,
    user_id UUID,
    name VARCHAR(100),
    schema_name VARCHAR(100),
    rules JSONB,
    created_at TIMESTAMPTZ
) AS $$
BEGIN
    RETURN QUERY
    SELECT d.id, d.user_id, d.name, d.schema_name, d.rules, d.created_at
    FROM postgate_databases d
    WHERE d.id = p_database_id;
END;
$$ LANGUAGE plpgsql;
