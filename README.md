# Postgate

Secure HTTP proxy for PostgreSQL with SQL validation and multi-tenant support.

## Architecture

```
┌─────────────────┐      JWT        ┌──────────────┐      ┌────────────┐
│ postgate-admin  │ ──────────────▶ │   postgate   │ ───▶ │ PostgreSQL │
│   (worker)      │                 │              │      │            │
└─────────────────┘                 └──────────────┘      └────────────┘
        │                                  ▲
        │  SELECT create_tenant_database() │
        └──────────────────────────────────┘

┌─────────────────┐      JWT        ┌──────────────┐      ┌────────────┐
│  tenant worker  │ ──────────────▶ │   postgate   │ ───▶ │ PostgreSQL │
│                 │                 │              │      │ (schema X) │
└─────────────────┘                 └──────────────┘      └────────────┘
```

## Features

- **SQL Validation**: Parses and validates SQL using sqlparser
  - Restrict operations (SELECT, INSERT, UPDATE, DELETE)
  - Allow/deny specific tables
  - Single statement only (no SQL injection via `;`)

- **Multi-tenant Isolation**:
  - **Schema mode**: Each tenant gets a PostgreSQL schema (shared pool)
  - **Dedicated mode**: External connection string for premium tenants

- **JWT Authentication**: Tokens contain database_id, validated with shared secret

- **Tenant Management via PL/pgSQL**:
  - `create_tenant_database()` - Creates schema + database entry
  - `delete_tenant_database()` - Drops schema + removes entry
  - `list_tenant_databases()` - List databases for a user
  - `get_tenant_database()` - Get database details

## Request Flow

1. Client sends `POST /query` with JWT Bearer token
2. Postgate validates JWT, extracts `database_id`
3. Loads database config from `postgate_databases` table
4. Parses and validates SQL against rules
5. Executes in transaction with `SET LOCAL search_path TO "schema"`
6. Returns `{rows: [...], row_count: N}`

## Configuration

```bash
DATABASE_URL=postgres://user:pass@localhost/postgate
JWT_SECRET=your-secret-key
POSTGATE_HOST=127.0.0.1  # optional, default: 127.0.0.1
POSTGATE_PORT=3000       # optional, default: 3000
```

## API

### POST /query

Execute a SQL query.

**Headers:**
- `Authorization: Bearer <jwt>` - JWT containing `{sub: "database-uuid", exp: ...}`
- `Content-Type: application/json`

**Body:**
```json
{
  "sql": "SELECT * FROM users WHERE id = $1",
  "params": [1]
}
```

**Response:**
```json
{
  "rows": [{"id": 1, "name": "Alice"}],
  "row_count": 1
}
```

### GET /health

Health check endpoint.

**Response:**
```json
{"status": "ok"}
```

## Admin Functions (via postgate)

Admin worker uses these PL/pgSQL functions through postgate itself:

```sql
-- Create a tenant
SELECT * FROM create_tenant_database('user-uuid'::uuid, 'my_database');
-- Returns: {id: "db-uuid", schema_name: "tenant_..."}

-- List tenants for a user
SELECT * FROM list_tenant_databases('user-uuid'::uuid);

-- Get tenant details
SELECT * FROM get_tenant_database('db-uuid'::uuid);

-- Delete a tenant
SELECT delete_tenant_database('db-uuid'::uuid);
-- Returns: true/false
```

## Development

```bash
# Setup test database
createdb postgate_test
psql postgate_test -c "CREATE USER postgate WITH PASSWORD 'password' SUPERUSER"

# Run migrations
DATABASE_URL="postgres://postgate:password@localhost/postgate_test" cargo run

# Run tests
DATABASE_URL="postgres://postgate:password@localhost/postgate_test" cargo test

# Format code
cargo fmt
```

## Project Structure

```
src/
├── auth.rs      # JWT validation
├── config.rs    # Types (Config, DatabaseBackend, QueryRules)
├── error.rs     # Error types → HTTP responses
├── executor.rs  # ExecutorPool, SQL execution
├── parser.rs    # SQL validation (sqlparser)
├── server.rs    # HTTP handlers (actix-web)
├── store.rs     # CRUD for postgate_databases
├── lib.rs       # Module exports
└── main.rs      # Entry point, migrations

migrations/
├── 001_create_postgate_databases.sql
└── 002_tenant_functions.sql

tests/
└── integration.rs  # Integration tests
```

## License

MIT
