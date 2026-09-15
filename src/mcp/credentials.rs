use crate::automation::error::{BridgeError, Result};
use hbb_common::rand::{rngs::OsRng, RngCore};
use security_framework::passwords::{get_generic_password, set_generic_password};

const SERVICE: &str = "org.rustdesk.mcp-controller";
const ACCOUNT: &str = "local-http";

pub fn load_or_create(reset: bool) -> Result<String> {
    if !reset {
        match get_generic_password(SERVICE, ACCOUNT) {
            Ok(bytes) => {
                return String::from_utf8(bytes).map_err(|_| {
                    BridgeError::new("CREDENTIAL_ERROR", "Stored MCP credential is invalid")
                })
            }
            Err(error) if error.code() == -25300 => {}
            Err(_) => {
                return Err(BridgeError::new(
                    "CREDENTIAL_ERROR",
                    "Keychain access failed",
                ))
            }
        }
    }
    let mut random = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut random)
        .map_err(|_| BridgeError::new("CREDENTIAL_ERROR", "Secure random source failed"))?;
    let token = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    set_generic_password(SERVICE, ACCOUNT, token.as_bytes()).map_err(|_| {
        BridgeError::new(
            "CREDENTIAL_ERROR",
            "Could not save MCP credential to Keychain",
        )
    })?;
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
    #[test]
    fn bearer_requires_the_whole_exact_token() {
        assert!(super::matches("abc123", "abc123"));
        for token in ["", "abc12", "abc1234", "xbc123", "abc12x"] {
            assert!(!super::matches("abc123", token));
        }
    }
}
