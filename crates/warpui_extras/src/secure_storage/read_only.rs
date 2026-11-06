use super::{Error, SecureStorage};

pub(super) struct ReadOnlySecureStorage {
    inner: Box<dyn SecureStorage>,
}

impl ReadOnlySecureStorage {
    pub(super) fn new(inner: Box<dyn SecureStorage>) -> Self {
        Self { inner }
    }

    fn mutation_error() -> Error {
        Error::Unknown(anyhow::anyhow!(
            "secure storage is read-only in a window worker"
        ))
    }
}

impl SecureStorage for ReadOnlySecureStorage {
    fn write_value(&self, _: &str, _: &str) -> Result<(), Error> {
        Err(Self::mutation_error())
    }

    fn write_value_with_owner_only_fallback(&self, _: &str, _: &str) -> Result<(), Error> {
        Err(Self::mutation_error())
    }

    fn read_value(&self, key: &str) -> Result<String, Error> {
        self.inner.read_value(key)
    }

    fn remove_value(&self, _: &str) -> Result<(), Error> {
        Err(Self::mutation_error())
    }
}

#[cfg(test)]
#[path = "read_only_tests.rs"]
mod tests;
