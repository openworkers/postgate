# Postgate

Secure HTTP proxy for PostgreSQL with SQL validation, token-based authentication, and multi-tenant support.

## Overview

Postgate provides a secure HTTP interface to PostgreSQL with:
- **Token-based authentication** - API tokens (`pg_xxx`) with SHA-256 hashing
- **SQL validation** - Parses and validates every query before execution
- **Multi-tenant isolation** - Schema-based or dedicated database backends
- **Fine-grained permissions** - Per-token operation control (SELECT, INSERT, UPDATE, DELETE, CREATE, ALTER, DROP)
- **PL/pgSQL administration** - All tenant/token management via SQL functions

## Architecture

```
                                Token: pg_xxx...
┌─────────────────┐                                  ┌──────────────┐
│  Admin Client   │ ────── POST /query ────────────▶ │              │
│                 │   SELECT create_tenant_token()   │   postgate   │
└─────────────────┘                                  │              │
                                                     │   ┌──────┐   │      ┌────────────────┐
                                Token: pg_xxx...     │   │ SQL  │   │      │   PostgreSQL   │
┌─────────────────┐                                  │   │Parser│   │ ───▶ │                │
│  Tenant Client  │ ────── POST /query ────────────▶ │   └──────┘   │      │ ┌────────────┐ │
│                 │   SELECT * FROM my_table         │              │      │ │  Schema A  │ │
└─────────────────┘                                  └──────────────┘      │ ├────────────┤ │
                                                                           │ │  Schema B  │ │
                                                                           │ └────────────┘ │
                                                                           └────────────────┘
```

## Endpoints

Postgate exposes only 2 endpoints:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/query` | POST | Execute SQL query |

All administration (creating databases, tokens) is done via SQL functions through `/query`.

## Quick Start

### 1. Setup Database

```bash
# Create the database
createdb postgate

# Create user
psql postgres -c "CREATE USER postgate WITH PASSWORD 'your-password' SUPERUSER"

# Run postgate (migrations run automatically)
DATABASE_URL="postgres://postgate:your-password@localhost/postgate" cargo run
```

### 2. Create Admin Token

Use the CLI to generate tokens for the seed databases:

```bash
# Generate token for postgate admin (manages tenants)
DATABASE_URL="postgres://postgate:your-password@localhost/postgate" \
  cargo run -- gen-token 00000000-0000-0000-0000-000000000000 admin
# Output: pg_abc123... (SAVE THIS!)
```

Or via SQL directly:

```sql
SELECT * FROM create_tenant_token(
    '00000000-0000-0000-0000-000000000000'::uuid,
    'admin',
    ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE', 'CREATE', 'ALTER', 'DROP']
);
```

### 3. Use the API

```bash
# Health check
curl http://localhost:3000/health

# Create a tenant database (using admin token)
curl -X POST http://localhost:3000/query \
  -H "Authorization: Bearer pg_your_admin_token" \
  -H "Content-Type: application/json" \
  -d '{"sql": "SELECT * FROM create_tenant_database($1, $2::integer)", "params": ["my_app", 5000]}'

# Create a token for the tenant
curl -X POST http://localhost:3000/query \
  -H "Authorization: Bearer pg_your_admin_token" \
  -H "Content-Type: application/json" \
  -d '{"sql": "SELECT * FROM create_tenant_token($1::uuid, $2, $3::text[])", "params": ["<database-id>", "default", ["SELECT", "INSERT", "UPDATE", "DELETE"]]}'
```

## Configuration

| Environment Variable | Default | Description |
|---------------------|---------|-------------|
| `DATABASE_URL` | *required* | PostgreSQL connection string |
| `POSTGATE_HOST` | `127.0.0.1` | HTTP server bind address |
| `POSTGATE_PORT` | `3000` | HTTP server port |

## CLI Commands

```bash
# Start the server (default)
cargo run

# Create a tenant database (schema-based)
cargo run -- create-db <NAME> [-m <MAX_ROWS>]

# Create a dedicated database (external connection)
cargo run -- create-db <NAME> -d <CONNECTION_STRING>

# Generate a token for a database
cargo run -- gen-token <DATABASE_ID> [NAME] [-p <PERMISSIONS>]

# Show help
cargo run -- --help
cargo run -- create-db --help
cargo run -- gen-token --help
```

**Examples:**

```bash
# Create a tenant database with schema isolation
cargo run -- create-db my-app
# Output: <database-uuid>
# Schema: tenant_xxx_my_app

# Create with custom max rows
cargo run -- create-db my-app -m 5000

# Create a dedicated database (external PostgreSQL)
cargo run -- create-db premium-client -d "postgres://user:pass@host/db"

# Generate token with default DML permissions
cargo run -- gen-token <database-uuid> default

# Generate token with full permissions (DML + DDL)
cargo run -- gen-token <database-uuid> admin \
    -p SELECT,INSERT,UPDATE,DELETE,CREATE,ALTER,DROP

# Generate read-only token
cargo run -- gen-token <database-uuid> readonly -p SELECT
```

## API Reference

### POST /query

Execute a SQL query against a tenant database.

**Headers:**
- `Authorization: Bearer <token>` - API token (format: `pg_<64_hex_chars>`)
- `Content-Type: application/json`

**Request Body:**
```json
{
  "sql": "SELECT * FROM users WHERE id = $1",
  "params": [1]
}
```

**Response (success):**
```json
{
  "rows": [{"id": 1, "name": "Alice", "email": "alice@example.com"}],
  "row_count": 1
}
```

**Response (error):**
```json
{
  "error": "Operation DELETE is not allowed",
  "code": "PARSE_ERROR"
}
```

**Error Codes:**
| Code | HTTP Status | Description |
|------|-------------|-------------|
| `PARSE_ERROR` | 400 | SQL parsing or validation failed |
| `ROW_LIMIT_EXCEEDED` | 400 | Query returned more rows than allowed |
| `UNAUTHORIZED` | 401 | Missing or invalid token |
| `DATABASE_NOT_FOUND` | 404 | Token's database doesn't exist |
| `TIMEOUT` | 504 | Query timed out (default: 30s) |
| `DATABASE_ERROR` | 500 | PostgreSQL execution error |
| `INTERNAL_ERROR` | 500 | Unexpected server error |

### GET /health

Health check endpoint.

**Response:**
```json
{"status": "ok"}
```

## Token System

### Token Format

Tokens follow a specific format for security and identification:

```
pg_<64_hex_characters>
│   └─────────────────────── 32 random bytes (hex encoded)
└─────────────────────────── Prefix for identification
```

Example: `pg_a1b2c3d4e5f6...` (67 characters total)

### Token Storage

- Tokens are **hashed with SHA-256** before storage
- Only the hash is stored in `postgate_tokens` table
- A `token_prefix` (first 8 chars) is stored for identification
- **The full token is only returned once** at creation time

### Token Permissions

Each token has an array of allowed SQL operations:

| Permission | Description |
|------------|-------------|
| `SELECT` | Read data |
| `INSERT` | Create new rows |
| `UPDATE` | Modify existing rows |
| `DELETE` | Remove rows |
| `CREATE` | Create tables, indexes, views |
| `ALTER` | Modify table structure |
| `DROP` | Drop tables, truncate |

**Permission Sets:**
- **Default** (`SELECT`, `INSERT`, `UPDATE`, `DELETE`) - Safe for most applications
- **Tenant** (all 7 permissions) - Full control over schema

## SQL Validation

Every query is parsed and validated before execution:

### Blocked Patterns
- Multiple statements (prevents SQL injection via `;`)
- Schema-qualified table names (`public.users`, `other_schema.data`)
- System tables (`pg_*`, `information_schema`)
- Operations not allowed by token permissions

### Examples

```sql
-- ✅ Allowed (with SELECT permission)
SELECT * FROM users WHERE id = $1

-- ✅ Allowed (with CREATE permission)
CREATE TABLE orders (id SERIAL PRIMARY KEY, user_id INT)

-- ❌ Blocked: Multiple statements
SELECT 1; DROP TABLE users

-- ❌ Blocked: Schema-qualified name
SELECT * FROM public.users

-- ❌ Blocked: System table access
SELECT * FROM pg_tables

-- ✅ Allowed: postgate_helpers functions
SELECT * FROM postgate_helpers.list_tables()
```

## Helper Functions

The `postgate_helpers` schema provides utility functions accessible to all tenants:

### postgate_helpers.list_tables()

List all tables in the current tenant's schema with row counts.

```sql
SELECT * FROM postgate_helpers.list_tables();
-- Returns: { table_name: "users", row_count: 42 }, ...
```

### postgate_helpers.describe_table(name)

Describe columns of a table in the current tenant's schema.

```sql
SELECT * FROM postgate_helpers.describe_table('users');
-- Returns: { column_name, data_type, is_nullable, column_default, is_primary_key }
```

## Multi-Tenant Isolation

### Schema Backend (Default)

Each tenant gets an isolated PostgreSQL schema:

```
PostgreSQL Database
├── public/              ← postgate metadata tables
│   ├── postgate_databases
│   └── postgate_tokens
├── tenant_abc123_myapp/ ← Tenant A's schema
│   ├── users
│   └── orders
└── tenant_def456_other/ ← Tenant B's schema
    ├── products
    └── inventory
```

**How it works:**
1. Query arrives with token
2. Postgate validates token, gets `database_id`
3. Looks up `schema_name` from `postgate_databases`
4. Executes in transaction with `SET LOCAL search_path TO "tenant_xxx"`
5. Tenant can only see their own tables

### Dedicated Backend

For premium tenants, use a separate PostgreSQL connection:

```sql
INSERT INTO postgate_databases (name, backend_type, connection_string, max_rows)
VALUES ('premium_tenant', 'dedicated', 'postgres://user:pass@host/db', 10000);
```

## PL/pgSQL Functions

All administration is done via SQL functions executed through `/query`:

### create_tenant_database

Create a new tenant with isolated schema.

```sql
SELECT * FROM create_tenant_database(
    'my_app_name',    -- Database name
    5000              -- Max rows per query (optional, default: 1000)
);
-- Returns: { id: "uuid", schema_name: "tenant_xxx_my_app_name" }
```

### delete_tenant_database

Delete a tenant and drop their schema.

```sql
SELECT delete_tenant_database('database-uuid'::uuid);
-- Returns: true/false
```

### create_tenant_token

Create an API token for a database.

```sql
SELECT * FROM create_tenant_token(
    'database-uuid'::uuid,                              -- Database ID
    'my_token_name',                                    -- Token name (optional)
    ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']       -- Permissions (optional)
);
-- Returns: { id: "token-uuid", token: "pg_xxx..." }
-- ⚠️ SAVE THE TOKEN! It's only shown once.
```

### delete_tenant_token

Delete a token by ID.

```sql
SELECT delete_tenant_token('token-uuid'::uuid);
-- Returns: true/false
```

### Querying Tokens (via SQL)

```sql
-- List all tokens for a database (admin only, through /query)
SELECT id, name, token_prefix, created_at, last_used_at
FROM postgate_tokens
WHERE database_id = 'your-database-id'::uuid;
```

## Database Schema

### postgate_databases

| Column | Type | Description |
|--------|------|-------------|
| `id` | UUID | Primary key |
| `name` | VARCHAR(100) | Display name |
| `backend_type` | VARCHAR(20) | `'schema'` or `'dedicated'` |
| `schema_name` | VARCHAR(100) | For schema backend |
| `connection_string` | TEXT | For dedicated backend |
| `max_rows` | INTEGER | Max rows per query (default: 1000) |
| `created_at` | TIMESTAMPTZ | Creation timestamp |

### postgate_tokens

| Column | Type | Description |
|--------|------|-------------|
| `id` | UUID | Primary key |
| `database_id` | UUID | FK to postgate_databases |
| `name` | VARCHAR(100) | Token name |
| `token_hash` | VARCHAR(64) | SHA-256 hash (hex) |
| `token_prefix` | VARCHAR(8) | First 8 chars for identification |
| `allowed_operations` | TEXT[] | Array of permissions |
| `created_at` | TIMESTAMPTZ | Creation timestamp |
| `last_used_at` | TIMESTAMPTZ | Last usage timestamp |

## Seed Data

The migration creates a default admin database:

| ID | Name | Backend | Purpose |
|----|------|---------|---------|
| `00000000-0000-0000-0000-000000000000` | postgate_admin | schema (public) | Admin operations |

## Client Integration

### TypeScript/JavaScript

```typescript
interface PostgateQueryRequest {
  sql: string;
  params?: unknown[];
}

interface PostgateQueryResponse<T = Record<string, unknown>> {
  rows: T[];
  row_count: number;
}

class PostgateClient {
  constructor(private baseUrl: string, private token: string) {}

  async query<T>(sql: string, params?: unknown[]): Promise<PostgateQueryResponse<T>> {
    const response = await fetch(`${this.baseUrl}/query`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Authorization': `Bearer ${this.token}`
      },
      body: JSON.stringify({ sql, params })
    });

    if (!response.ok) {
      const error = await response.json();
      throw new Error(error.error || `HTTP ${response.status}`);
    }

    return response.json();
  }
}

// Usage
const client = new PostgateClient('http://localhost:3000', 'pg_your_token');
const { rows } = await client.query<{ id: number; name: string }>(
  'SELECT * FROM users WHERE id = $1',
  [1]
);
```

### Admin Operations

```typescript
class PostgateAdminClient extends PostgateClient {
  async createDatabase(name: string, maxRows = 1000) {
    return this.query(
      'SELECT * FROM create_tenant_database($1, $2::integer)',
      [name, maxRows]
    );
  }

  async deleteDatabase(databaseId: string) {
    return this.query(
      'SELECT delete_tenant_database($1::uuid)',
      [databaseId]
    );
  }

  async createToken(
    databaseId: string,
    name = 'default',
    permissions = ['SELECT', 'INSERT', 'UPDATE', 'DELETE']
  ) {
    return this.query(
      'SELECT * FROM create_tenant_token($1::uuid, $2, $3::text[])',
      [databaseId, name, permissions]
    );
  }

  async deleteToken(tokenId: string) {
    return this.query(
      'SELECT delete_tenant_token($1::uuid)',
      [tokenId]
    );
  }
}
```

## Security Considerations

### Token Security
- Tokens are generated with 32 bytes of cryptographic randomness
- Only SHA-256 hashes are stored (tokens cannot be recovered)
- Tokens should be transmitted over HTTPS only
- Rotate tokens periodically

### SQL Injection Prevention
- All queries are parsed and validated before execution
- Multiple statements are blocked
- Parameterized queries prevent injection in values
- Schema-qualified names are blocked to prevent escaping tenant isolation

### Schema Isolation
- Each tenant operates in their own PostgreSQL schema
- `SET LOCAL search_path` ensures queries only see tenant tables
- System tables (`pg_*`) access is blocked
- Cross-schema references are blocked

## Development

```bash
# Clone and setup
git clone <repo>
cd postgate

# Setup test database
createdb postgate_test
psql postgres -c "CREATE USER postgate WITH PASSWORD 'password' SUPERUSER"

# Run tests
DATABASE_URL="postgres://postgate:password@localhost/postgate_test" cargo test

# Run server
DATABASE_URL="postgres://postgate:password@localhost/postgate_test" cargo run

# Format code
cargo fmt
```

## Project Structure

```
postgate/
├── src/
│   ├── main.rs       # Entry point, migrations, server startup
│   ├── lib.rs        # Module exports
│   ├── auth.rs       # Token extraction and validation
│   ├── config.rs     # Configuration types (DatabaseBackend, SqlOperation, etc.)
│   ├── error.rs      # Error types with HTTP response mapping
│   ├── executor.rs   # SQL execution (schema/dedicated backends)
│   ├── parser.rs     # SQL validation (sqlparser)
│   ├── server.rs     # HTTP handlers (actix-web)
│   ├── store.rs      # Database CRUD operations
│   └── token.rs      # Token generation and hashing
├── migrations/
│   └── 001_init.sql  # Schema + PL/pgSQL functions
├── tests/
│   └── integration.rs # Integration tests
├── Cargo.toml
└── README.md
```

## License

MIT
