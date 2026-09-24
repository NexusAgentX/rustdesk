use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
};
use subtle::ConstantTimeEq;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct Principal {
    pub id: String,
    pub credential: String,
    pub revoked: CancellationToken,
}

struct Entry {
    hash: [u8; 32],
    principal: Principal,
}

/// Tokens belong to individual clients. Client-reported names never identify a principal.
pub struct Credentials {
    entries: Mutex<HashMap<String, Entry>>,
    capacity: usize,
}

impl Credentials {
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(HashMap::new()),
            capacity,
        })
    }

    pub fn issue(&self) -> io::Result<(String, String)> {
        let id = Uuid::new_v4().to_string();
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        self.insert(id.clone(), &token)?;
        Ok((id, token))
    }

    /// Restore a previously generated token from the platform's protected credential storage.
    pub fn insert(&self, id: String, token: &str) -> io::Result<()> {
        if id.is_empty()
            || id.len() > 128
            || token.len() < 32
            || token.len() > 256
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/=".contains(&byte))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid MCP credential",
            ));
        }
        let mut entries = self.entries.lock().unwrap();
        if entries.contains_key(&id) || entries.len() >= self.capacity {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "MCP credential limit or duplicate ID",
            ));
        }
        let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        if entries
            .values()
            .any(|entry| bool::from(entry.hash.ct_eq(&hash)))
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Duplicate MCP credential",
            ));
        }
        entries.insert(
            id.clone(),
            Entry {
                hash,
                principal: Principal {
                    id,
                    credential: Uuid::new_v4().to_string(),
                    revoked: CancellationToken::new(),
                },
            },
        );
        Ok(())
    }

    pub fn revoke(&self, id: &str) -> bool {
        if let Some(entry) = self.entries.lock().unwrap().remove(id) {
            entry.principal.revoked.cancel();
            true
        } else {
            false
        }
    }

    pub(crate) fn authenticate(&self, token: &str) -> Option<Principal> {
        if token.len() > 256 {
            return None;
        }
        let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.entries
            .lock()
            .unwrap()
            .values()
            .find(|entry| bool::from(entry.hash.ct_eq(&hash)))
            .map(|entry| entry.principal.clone())
    }
}
