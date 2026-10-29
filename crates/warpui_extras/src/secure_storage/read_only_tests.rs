use std::collections::HashMap;
use std::sync::Mutex;

use super::ReadOnlySecureStorage;
use crate::secure_storage::{Error, SecureStorage};

#[derive(Default)]
struct MemoryStorage {
    values: Mutex<HashMap<String, String>>,
}

impl SecureStorage for MemoryStorage {
    fn write_value(&self, key: &str, value: &str) -> Result<(), Error> {
        self.values
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    fn read_value(&self, key: &str) -> Result<String, Error> {
        self.values
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or(Error::NotFound)
    }

    fn remove_value(&self, key: &str) -> Result<(), Error> {
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}

#[test]
fn read_only_secure_storage_reads_without_mutating_secrets() {
    let inner = MemoryStorage::default();
    inner.write_value("token", "original").unwrap();
    let storage = ReadOnlySecureStorage::new(Box::new(inner));

    let error = storage.write_value("token", "changed").unwrap_err();

    let Error::Unknown(error) = error else {
        panic!("unexpected secure storage error");
    };
    assert_eq!(
        error.to_string(),
        "secure storage is read-only in a window worker"
    );
    assert_eq!(storage.read_value("token").unwrap(), "original");
}

#[test]
fn read_only_secure_storage_rejects_removals() {
    let storage = ReadOnlySecureStorage::new(Box::new(MemoryStorage::default()));

    let error = storage.remove_value("token").unwrap_err();

    let Error::Unknown(error) = error else {
        panic!("unexpected secure storage error");
    };
    assert_eq!(
        error.to_string(),
        "secure storage is read-only in a window worker"
    );
}
