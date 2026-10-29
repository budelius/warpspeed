use super::{Error, Model, UserPreferences};

pub struct ReadOnlyPreferences {
    inner: Model,
}

impl ReadOnlyPreferences {
    pub fn new(inner: Model) -> Self {
        Self { inner }
    }

    fn mutation_error() -> Error {
        Error::Unknown(anyhow::anyhow!(
            "preferences are read-only in a window worker"
        ))
    }
}

impl UserPreferences for ReadOnlyPreferences {
    fn write_value(&self, _: &str, _: String) -> Result<(), Error> {
        Err(Self::mutation_error())
    }

    fn read_value(&self, key: &str) -> Result<Option<String>, Error> {
        self.inner.read_value(key)
    }

    fn remove_value(&self, _: &str) -> Result<(), Error> {
        Err(Self::mutation_error())
    }

    fn write_value_with_hierarchy(
        &self,
        _: &str,
        _: String,
        _: Option<&str>,
        _: Option<u32>,
    ) -> Result<(), Error> {
        Err(Self::mutation_error())
    }

    fn read_value_with_hierarchy(
        &self,
        key: &str,
        hierarchy: Option<&str>,
    ) -> Result<Option<String>, Error> {
        self.inner.read_value_with_hierarchy(key, hierarchy)
    }

    fn remove_value_with_hierarchy(&self, _: &str, _: Option<&str>) -> Result<(), Error> {
        Err(Self::mutation_error())
    }

    fn is_settings_file(&self) -> bool {
        self.inner.is_settings_file()
    }

    fn reload_from_disk(&self) -> Result<(), Error> {
        self.inner.reload_from_disk()
    }

    fn inhibit_writes_for_key(&self, key: &str, hierarchy: Option<&str>) {
        self.inner.inhibit_writes_for_key(key, hierarchy);
    }

    fn clear_all_write_inhibitions(&self) {
        self.inner.clear_all_write_inhibitions();
    }
}

#[cfg(test)]
#[path = "read_only_tests.rs"]
mod tests;
