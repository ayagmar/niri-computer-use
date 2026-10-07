use std::fmt;

/// A harness failure: what was being done, and the upstream error text.
#[derive(Debug)]
pub(crate) struct Failure(String);

impl Failure {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub(crate) type Result<T> = std::result::Result<T, Failure>;

/// Adds what the harness was doing to an upstream error.
pub(crate) trait Context<T> {
    fn context(self, doing: impl fmt::Display) -> Result<T>;
}

impl<T, E: fmt::Display> Context<T> for std::result::Result<T, E> {
    fn context(self, doing: impl fmt::Display) -> Result<T> {
        self.map_err(|err| Failure(format!("{doing}: {err}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_keeps_the_upstream_error() {
        let result: std::result::Result<(), &str> = Err("permission denied");
        let failure = result.context("create /x").unwrap_err();
        assert_eq!(failure.to_string(), "create /x: permission denied");
    }
}
