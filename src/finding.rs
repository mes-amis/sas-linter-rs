use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Warning => f.write_str("warning"),
            Severity::Error => f.write_str("error"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub rule: &'static str,
    pub message: String,
    pub severity: Severity,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: [{}] {}",
            self.path, self.line, self.column, self.rule, self.message
        )
    }
}
