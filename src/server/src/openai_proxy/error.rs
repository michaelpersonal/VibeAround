use std::fmt;

pub type Result<T> = std::result::Result<T, ProxyTransformError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyTransformError {
    ExpectedObject(&'static str),
    MissingField(&'static str),
    Unsupported(String),
}

impl fmt::Display for ProxyTransformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExpectedObject(name) => write!(f, "expected {name} to be a JSON object"),
            Self::MissingField(name) => write!(f, "missing required field `{name}`"),
            Self::Unsupported(message) => {
                write!(f, "unsupported OpenAI proxy transform: {message}")
            }
        }
    }
}

impl std::error::Error for ProxyTransformError {}
