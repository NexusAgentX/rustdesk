use crate::automation::error::{BridgeError, Result};
use hbb_common::{
    config::Config,
    libc,
    rand::{rngs::OsRng, RngCore},
};
use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    sync::Mutex,
};

pub fn load_or_create(reset: bool) -> Result<String> {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap();
    let directory = Config::path("mcp");
    if directory.as_os_str().is_empty() {
        return Err(BridgeError::new(
            "CREDENTIAL_ERROR",
            "Configuration directory is unavailable",
        ));
    }
    load_file(&directory, reset).map_err(|error| {
        BridgeError::new(
            "CREDENTIAL_ERROR",
            format!("Could not access MCP credential file: {error}"),
        )
    })
}

fn load_file(directory: &Path, reset: bool) -> std::io::Result<String> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(std::io::Error::other(
            "Credential directory must be owned by the current user and not be a symlink",
        ));
    }
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    let path = directory.join("token");
    if !reset {
        match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => {
                if !file.metadata()?.is_file()
                    || file.metadata()?.uid() != unsafe { libc::geteuid() }
                {
                    return Err(std::io::Error::other(
                        "Invalid credential file owner or type",
                    ));
                }
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
                let mut token = String::new();
                file.take(65).read_to_string(&mut token)?;
                if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(std::io::Error::other("Stored MCP credential is invalid"));
                }
                return Ok(token);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let mut random = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut random)
        .map_err(|_| std::io::Error::other("Secure random source failed"))?;
    let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = directory.join(format!(".token-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(token.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &path)
    })();
    if let Err(error) = result {
        if let Err(cleanup) = fs::remove_file(&temporary) {
            if cleanup.kind() != std::io::ErrorKind::NotFound {
                hbb_common::log::warn!("Could not remove temporary MCP credential file: {cleanup}");
            }
        }
        return Err(error);
    }
    Ok(token)
}

pub fn matches(expected: &str, supplied: &str) -> bool {
    let mut difference = expected.len() ^ supplied.len();
    for (index, expected) in expected.bytes().enumerate() {
        difference |= usize::from(expected ^ supplied.as_bytes().get(index).copied().unwrap_or(0));
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_file_persists_rotates_and_restricts_permissions() {
        let dir = std::env::temp_dir().join(format!("rustdesk-mcp-{}", uuid::Uuid::new_v4()));
        let first = load_file(&dir, false).unwrap();
        assert_eq!(load_file(&dir, false).unwrap(), first);
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(dir.join("token"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_ne!(load_file(&dir, true).unwrap(), first);
        fs::write(dir.join("token"), "invalid").unwrap();
        assert!(load_file(&dir, false).is_err());
        fs::remove_file(dir.join("token")).unwrap();
        std::os::unix::fs::symlink(dir.join("missing"), dir.join("token")).unwrap();
        assert!(load_file(&dir, false).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn bearer_requires_the_whole_exact_token() {
        assert!(super::matches("abc123", "abc123"));
        for token in ["", "abc12", "abc1234", "xbc123", "abc12x"] {
            assert!(!super::matches("abc123", token));
        }
    }
}
