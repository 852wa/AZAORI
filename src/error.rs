use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AzaError {
    pub message: String,
    pub kind: String,
    pub line: Option<usize>,
    pub col: Option<usize>,
    pub end: Option<i64>,
}

impl AzaError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: "runtime".into(),
            line: None,
            col: None,
            end: None,
        }
    }
    pub fn at(mut self, line: usize) -> Self {
        if self.line.is_none() {
            self.line = Some(line);
        }
        self
    }
    pub fn place(mut self, col: usize, end: usize) -> Self {
        if self.col.is_none() {
            self.col = Some(col);
            self.end = Some(end as i64);
        }
        self
    }
    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = kind.into();
        self
    }
}

impl fmt::Display for AzaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(line) = self.line {
            write!(f, "{}行目: {}", line, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}
impl std::error::Error for AzaError {}
pub type AzaResult<T> = Result<T, AzaError>;
