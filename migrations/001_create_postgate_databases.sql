CREATE TABLE IF NOT EXISTS postgate_databases (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL,
    name VARCHAR(100) NOT NULL,

    -- Backend type: 'schema' ou 'dedicated'
    backend_type VARCHAR(20) NOT NULL DEFAULT 'schema',

    -- Pour backend 'schema'
    schema_name VARCHAR(100),

    -- Pour backend 'dedicated'
    connection_string TEXT,

    -- Rules (allowed_operations, allowed_tables, denied_tables, max_rows, timeout_seconds)
    rules JSONB NOT NULL DEFAULT '{}',

    created_at TIMESTAMPTZ DEFAULT NOW(),

    CONSTRAINT valid_backend CHECK (
        (backend_type = 'schema' AND schema_name IS NOT NULL) OR
        (backend_type = 'dedicated' AND connection_string IS NOT NULL)
    ),

    CONSTRAINT unique_schema UNIQUE (schema_name)
);

CREATE INDEX IF NOT EXISTS idx_postgate_databases_user_id ON postgate_databases(user_id);
