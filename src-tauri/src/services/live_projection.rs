//! Shared error aggregation for multi-target live configuration projection.

use crate::error::AppError;

#[derive(Default)]
pub(crate) struct LiveProjectionFailures {
    failures: Vec<String>,
}

impl LiveProjectionFailures {
    pub(crate) fn record(&mut self, target: impl Into<String>, result: Result<(), AppError>) {
        if let Err(error) = result {
            self.push(target, error);
        }
    }

    pub(crate) fn push(&mut self, target: impl Into<String>, error: impl std::fmt::Display) {
        let failure = format!("{}: {error}", target.into());
        log::warn!("{failure}");
        self.failures.push(failure);
    }

    pub(crate) fn finish(self, operation: &str) -> Result<(), AppError> {
        if self.failures.is_empty() {
            Ok(())
        } else {
            Err(AppError::Message(format!(
                "{operation} failed for {}",
                self.failures.join("; ")
            )))
        }
    }
}
