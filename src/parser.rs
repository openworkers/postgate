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
    let tables = extract_tables(&statement);

    // Validate operation
    if !rules.allowed_operations.is_empty() && !rules.allowed_operations.contains(&operation) {
        return Err(ParseError::OperationNotAllowed(operation));
    }

    // Validate tables
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

fn extract_tables(statement: &Statement) -> HashSet<String> {
    let mut tables = HashSet::new();

    let _ = visit_relations(statement, |relation| {
        tables.insert(
            relation
                .0
                .iter()
                .map(|i| i.value.clone())
                .collect::<Vec<_>>()
                .join("."),
        );
        std::ops::ControlFlow::<()>::Continue(())
    });

    tables
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
}
