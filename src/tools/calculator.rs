use async_trait::async_trait;
use std::collections::HashMap;

use crate::tools::Tool;
use crate::types::{AgentError, ToolDefinition};

/// Calculator tool that evaluates mathematical expressions
pub struct Calculator;

impl Calculator {
    pub fn new() -> Self {
        Self
    }
}

impl Default for Calculator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for Calculator {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "calculator".to_string(),
            description: "Evaluates mathematical expressions. Supports +, -, *, /, parentheses, decimals, and negative numbers.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "The mathematical expression to evaluate, e.g., '2 + 3 * 4' or '(10 - 5) / 2.5'"
                    }
                },
                "required": ["expression"]
            }),
        }
    }

    async fn execute(
        &self,
        input: HashMap<String, serde_json::Value>,
    ) -> Result<String, AgentError> {
        let expression = input
            .get("expression")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::ToolError("Missing 'expression' parameter".to_string()))?;

        let result = evaluate(expression)
            .map_err(|e| AgentError::ToolError(format!("Calculation error: {}", e)))?;

        Ok(format!("{}", result))
    }
}

/// Recursive descent parser for mathematical expressions
/// Grammar:
///   expression = term (('+' | '-') term)*
///   term = factor (('*' | '/') factor)*
///   factor = '-'? atom
///   atom = number | '(' expression ')'

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn consume(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.consume();
            } else {
                break;
            }
        }
    }

    fn parse_expression(&mut self) -> Result<f64, String> {
        let mut left = self.parse_term()?;

        loop {
            self.skip_whitespace();
            match self.peek() {
                Some('+') => {
                    self.consume();
                    let right = self.parse_term()?;
                    left += right;
                }
                Some('-') => {
                    self.consume();
                    let right = self.parse_term()?;
                    left -= right;
                }
                _ => break,
            }
        }

        Ok(left)
    }

    fn parse_term(&mut self) -> Result<f64, String> {
        let mut left = self.parse_factor()?;

        loop {
            self.skip_whitespace();
            match self.peek() {
                Some('*') => {
                    self.consume();
                    let right = self.parse_factor()?;
                    left *= right;
                }
                Some('/') => {
                    self.consume();
                    let right = self.parse_factor()?;
                    if right == 0.0 {
                        return Err("Division by zero".to_string());
                    }
                    left /= right;
                }
                _ => break,
            }
        }

        Ok(left)
    }

    fn parse_factor(&mut self) -> Result<f64, String> {
        self.skip_whitespace();

        // Handle unary minus
        if self.peek() == Some('-') {
            self.consume();
            let value = self.parse_atom()?;
            return Ok(-value);
        }

        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<f64, String> {
        self.skip_whitespace();

        match self.peek() {
            Some('(') => {
                self.consume();
                let result = self.parse_expression()?;
                self.skip_whitespace();
                if self.peek() != Some(')') {
                    return Err("Expected closing parenthesis".to_string());
                }
                self.consume();
                Ok(result)
            }
            Some(c) if c.is_ascii_digit() || c == '.' => self.parse_number(),
            Some(c) => Err(format!("Unexpected character: {}", c)),
            None => Err("Unexpected end of expression".to_string()),
        }
    }

    fn parse_number(&mut self) -> Result<f64, String> {
        self.skip_whitespace();

        let mut num_str = String::new();

        // Integer part
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                num_str.push(c);
                self.consume();
            } else {
                break;
            }
        }

        // Decimal part
        if self.peek() == Some('.') {
            num_str.push('.');
            self.consume();

            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    num_str.push(c);
                    self.consume();
                } else {
                    break;
                }
            }
        }

        if num_str.is_empty() || num_str == "." {
            return Err("Invalid number".to_string());
        }

        num_str
            .parse::<f64>()
            .map_err(|_| format!("Invalid number: {}", num_str))
    }
}

/// Evaluate a mathematical expression
pub fn evaluate(expression: &str) -> Result<f64, String> {
    let mut parser = Parser::new(expression);
    let result = parser.parse_expression()?;

    parser.skip_whitespace();
    if parser.peek().is_some() {
        return Err(format!(
            "Unexpected character at position {}",
            parser.pos
        ));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_addition() {
        assert_eq!(evaluate("2 + 3").unwrap(), 5.0);
    }

    #[test]
    fn test_operator_precedence() {
        assert_eq!(evaluate("2 + 3 * 4").unwrap(), 14.0);
    }

    #[test]
    fn test_parentheses() {
        assert_eq!(evaluate("(2 + 3) * 4").unwrap(), 20.0);
    }

    #[test]
    fn test_decimals() {
        assert_eq!(evaluate("3.14 * 2").unwrap(), 6.28);
    }

    #[test]
    fn test_negative_numbers() {
        assert_eq!(evaluate("-5 + 3").unwrap(), -2.0);
    }

    #[test]
    fn test_complex_expression() {
        assert_eq!(evaluate("42 * 17 + (100 / 4)").unwrap(), 739.0);
    }

    #[test]
    fn test_division_by_zero() {
        assert!(evaluate("1 / 0").is_err());
    }
}
