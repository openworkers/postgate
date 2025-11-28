use crate::config::{QueryRules, SqlOperation};
use sqlparser::ast::{Statement, visit_relations};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("Failed to parse SQL: {0}")]
    SqlParser(#[from] sqlparser::parser::ParserError),

    #[error("Empty query")]
    EmptyQuery,

    #[error("Multiple statements not allowed")]
    MultipleStatements,

    #[error("Operation {0} is not allowed")]
    OperationNotAllowed(SqlOperation),

    #[error("Table '{0}' is not allowed")]
    TableNotAllowed(String),

    #[error("Table '{0}' is denied")]
    TableDenied(String),

    #[error("Qualified table names are not allowed: '{0}'")]
    QualifiedTableName(String),

    #[error("System table access is not allowed: '{0}'")]
    SystemTableAccess(String),

    #[error("Unsupported statement type")]
    UnsupportedStatement,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct ParsedQuery {
    pub operation: SqlOperation,
    pub tables: HashSet<String>,
    pub statement: Statement,
}

pub fn parse_and_validate(sql: &str, rules: &QueryRules) -> Result<ParsedQuery, ParseError> {
    let dialect = PostgreSqlDialect {};
    let statements = Parser::parse_sql(&dialect, sql)?;

    if statements.is_empty() {
        return Err(ParseError::EmptyQuery);
    }

    if statements.len() > 1 {
        return Err(ParseError::MultipleStatements);
    }

    let statement = statements.into_iter().next().unwrap();
    let operation = extract_operation(&statement)?;

    // Extract and validate table references (blocks qualified names, pg_*, etc.)
    let table_refs = extract_table_refs(&statement);
    let tables = validate_table_refs(&table_refs)?;

    // Validate operation
    if !rules.allowed_operations.is_empty() && !rules.allowed_operations.contains(&operation) {
        return Err(ParseError::OperationNotAllowed(operation));
    }

    // Validate tables against rules
    for table in &tables {
        let table_lower = table.to_lowercase();

        // Check denied tables
        if rules.denied_tables.contains(&table_lower) {
            return Err(ParseError::TableDenied(table.clone()));
        }

        // Check allowed tables (if whitelist is specified)
        if let Some(allowed) = &rules.allowed_tables {
            if !allowed.contains(&table_lower) {
                return Err(ParseError::TableNotAllowed(table.clone()));
            }
        }
    }

    Ok(ParsedQuery {
        operation,
        tables,
        statement,
    })
}

fn extract_operation(statement: &Statement) -> Result<SqlOperation, ParseError> {
    match statement {
        Statement::Query(_) => Ok(SqlOperation::Select),
        Statement::Insert(_) => Ok(SqlOperation::Insert),
        Statement::Update { .. } => Ok(SqlOperation::Update),
        Statement::Delete(_) => Ok(SqlOperation::Delete),
        _ => Err(ParseError::UnsupportedStatement),
    }
}

/// Table reference with schema info
#[derive(Debug)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
}

fn extract_table_refs(statement: &Statement) -> Vec<TableRef> {
    let mut tables = Vec::new();

    let _ = visit_relations(statement, |relation| {
        let parts: Vec<_> = relation.0.iter().map(|i| i.value.clone()).collect();

        let table_ref = match parts.len() {
            1 => TableRef {
                schema: None,
                name: parts[0].clone(),
            },
            2 => TableRef {
                schema: Some(parts[0].clone()),
                name: parts[1].clone(),
            },
            _ => TableRef {
                schema: Some(parts[..parts.len() - 1].join(".")),
                name: parts[parts.len() - 1].clone(),
            },
        };

        tables.push(table_ref);
        std::ops::ControlFlow::<()>::Continue(())
    });

    tables
}

fn validate_table_refs(table_refs: &[TableRef]) -> Result<HashSet<String>, ParseError> {
    let mut table_names = HashSet::new();

    for table_ref in table_refs {
        // Block qualified names (schema.table)
        if let Some(schema) = &table_ref.schema {
            let full_name = format!("{}.{}", schema, table_ref.name);
            return Err(ParseError::QualifiedTableName(full_name));
        }

        let name_lower = table_ref.name.to_lowercase();

        // Block system tables (pg_*)
        if name_lower.starts_with("pg_") {
            return Err(ParseError::SystemTableAccess(table_ref.name.clone()));
        }

        // Block information_schema access
        if name_lower == "information_schema" {
            return Err(ParseError::SystemTableAccess(table_ref.name.clone()));
        }

        table_names.insert(table_ref.name.clone());
    }

    Ok(table_names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_rules() -> QueryRules {
        QueryRules {
            allowed_operations: HashSet::from([
                SqlOperation::Select,
                SqlOperation::Insert,
                SqlOperation::Update,
                SqlOperation::Delete,
            ]),
            allowed_tables: None,
            denied_tables: HashSet::new(),
            max_rows: 1000,
            timeout_seconds: 30,
        }
    }

    #[test]
    fn test_parse_select() {
        let rules = default_rules();
        let result = parse_and_validate("SELECT * FROM users WHERE id = $1", &rules);
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert_eq!(parsed.operation, SqlOperation::Select);
        assert!(parsed.tables.contains("users"));
    }

    #[test]
    fn test_parse_insert() {
        let rules = default_rules();
        let result = parse_and_validate("INSERT INTO users (name, email) VALUES ($1, $2)", &rules);
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert_eq!(parsed.operation, SqlOperation::Insert);
    }

    #[test]
    fn test_operation_not_allowed() {
        let rules = QueryRules {
            allowed_operations: HashSet::from([SqlOperation::Select]),
            ..default_rules()
        };
        let result = parse_and_validate("DELETE FROM users WHERE id = $1", &rules);
        assert!(matches!(result, Err(ParseError::OperationNotAllowed(_))));
    }

    #[test]
    fn test_table_denied() {
        let rules = QueryRules {
            denied_tables: HashSet::from(["secrets".to_string()]),
            ..default_rules()
        };
        let result = parse_and_validate("SELECT * FROM secrets", &rules);
        assert!(matches!(result, Err(ParseError::TableDenied(_))));
    }

    #[test]
    fn test_table_not_in_whitelist() {
        let rules = QueryRules {
            allowed_tables: Some(HashSet::from(["users".to_string(), "posts".to_string()])),
            ..default_rules()
        };
        let result = parse_and_validate("SELECT * FROM admin_logs", &rules);
        assert!(matches!(result, Err(ParseError::TableNotAllowed(_))));
    }

    #[test]
    fn test_multiple_statements_rejected() {
        let rules = default_rules();
        let result = parse_and_validate("SELECT 1; SELECT 2", &rules);
        assert!(matches!(result, Err(ParseError::MultipleStatements)));
    }

    #[test]
    fn test_qualified_table_name_rejected() {
        let rules = default_rules();
        let result = parse_and_validate("SELECT * FROM public.users", &rules);
        assert!(matches!(result, Err(ParseError::QualifiedTableName(_))));
    }

    #[test]
    fn test_schema_qualified_rejected() {
        let rules = default_rules();
        let result = parse_and_validate("SELECT * FROM other_schema.secrets", &rules);
        assert!(matches!(result, Err(ParseError::QualifiedTableName(_))));
    }

    #[test]
    fn test_pg_catalog_rejected() {
        let rules = default_rules();
        let result = parse_and_validate("SELECT * FROM pg_catalog.pg_tables", &rules);
        assert!(matches!(result, Err(ParseError::QualifiedTableName(_))));
    }

    #[test]
    fn test_pg_tables_rejected() {
        let rules = default_rules();
        let result = parse_and_validate("SELECT * FROM pg_tables", &rules);
        assert!(matches!(result, Err(ParseError::SystemTableAccess(_))));
    }

    #[test]
    fn test_pg_namespace_rejected() {
        let rules = default_rules();
        let result = parse_and_validate("SELECT * FROM pg_namespace", &rules);
        assert!(matches!(result, Err(ParseError::SystemTableAccess(_))));
    }

    #[test]
    fn test_information_schema_rejected() {
        let rules = default_rules();
        let result = parse_and_validate("SELECT * FROM information_schema.tables", &rules);
        assert!(matches!(result, Err(ParseError::QualifiedTableName(_))));
    }
}
