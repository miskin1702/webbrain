use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

/// Manages authentication token and origin verification.
#[derive(Debug, Clone)]
pub struct AuthManager {
    token: String,
    allowed_extension_id: Option<String>,
}

impl AuthManager {
    /// Create a new AuthManager with an explicit token or generate one.
    pub fn new(token_override: Option<String>, allowed_extension_id: Option<String>) -> Self {
        let token = match token_override {
            Some(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => Self::load_or_generate_token(),
        };

        Self {
            token,
            allowed_extension_id,
        }
    }

    /// Access the active pairing token (e.g. to print in console on startup).
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Constant-time comparison of token.
    pub fn verify_token(&self, candidate: &str) -> bool {
        let a = self.token.as_bytes();
        let b = candidate.as_bytes();

        if a.len() != b.len() {
            return false;
        }

        let mut diff = 0u8;
        for (x, y) in a.iter().zip(b.iter()) {
            diff |= x ^ y;
        }
        diff == 0
    }

    /// Validate the WebSocket Origin header.
    /// - Native clients: no origin header or "null" -> ALLOWED.
    /// - Chrome/Firefox extensions: "chrome-extension://<id>" or "moz-extension://<id>" -> ALLOWED.
    /// - Web pages: "http://..." or "https://..." -> REJECTED.
    pub fn is_allowed_origin(&self, origin: Option<&str>) -> bool {
        let origin = match origin {
            Some(o) if !o.trim().is_empty() && o != "null" => o.trim(),
            _ => return true, // native client or headless test
        };

        if origin.starts_with("chrome-extension://") {
            let ext_id = origin
                .trim_start_matches("chrome-extension://")
                .trim_end_matches('/');
            if let Some(expected) = &self.allowed_extension_id {
                return ext_id == expected;
            }
            return true;
        }

        if origin.starts_with("moz-extension://") {
            let ext_id = origin
                .trim_start_matches("moz-extension://")
                .trim_end_matches('/');
            if let Some(expected) = &self.allowed_extension_id {
                return ext_id == expected;
            }
            return true;
        }

        // Web origins (http://, https://, etc.) are strictly rejected.
        false
    }

    /// Default token file path:
    /// Windows: `%LOCALAPPDATA%\webbrain\workspace-bridge.token`
    /// Unix/Other: `~/.config/webbrain/workspace-bridge.token`
    pub fn default_token_path() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
                let mut path = PathBuf::from(local_app_data);
                path.push("webbrain");
                path.push("workspace-bridge.token");
                return Some(path);
            }
        }

        if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            let mut path = PathBuf::from(home);
            path.push(".config");
            path.push("webbrain");
            path.push("workspace-bridge.token");
            return Some(path);
        }

        None
    }

    /// Generate a 32-character hex pairing token (two v4 UUIDs combined).
    pub fn generate_secure_token() -> String {
        format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
    }

    fn load_or_generate_token() -> String {
        if let Some(token_file) = Self::default_token_path() {
            if token_file.exists() {
                if let Ok(content) = fs::read_to_string(&token_file) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        return trimmed.to_string();
                    }
                }
            }

            // Generate and save token
            let new_token = Self::generate_secure_token();
            if let Some(parent) = token_file.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&token_file, &new_token);
            return new_token;
        }

        Self::generate_secure_token()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_verification() {
        let auth = AuthManager::new(Some("secret123".to_string()), None);
        assert!(auth.verify_token("secret123"));
        assert!(!auth.verify_token("wrong"));
        assert!(!auth.verify_token("secret124"));
        assert!(!auth.verify_token("secret1234"));
    }

    #[test]
    fn test_origin_verification() {
        let auth = AuthManager::new(Some("test".to_string()), None);
        // Native / no origin
        assert!(auth.is_allowed_origin(None));
        assert!(auth.is_allowed_origin(Some("")));
        assert!(auth.is_allowed_origin(Some("null")));

        // Extension origins
        assert!(auth.is_allowed_origin(Some("chrome-extension://abcdefghijklmnop")));
        assert!(
            auth.is_allowed_origin(Some("moz-extension://12345678-1234-1234-1234-123456789abc"))
        );

        // Web origins must be rejected
        assert!(!auth.is_allowed_origin(Some("https://malicious.com")));
        assert!(!auth.is_allowed_origin(Some("http://localhost:3000")));
        assert!(!auth.is_allowed_origin(Some("http://127.0.0.1:8080")));
    }

    #[test]
    fn test_specific_extension_id_restriction() {
        let auth = AuthManager::new(Some("test".to_string()), Some("my_trusted_id".to_string()));
        assert!(auth.is_allowed_origin(Some("chrome-extension://my_trusted_id")));
        assert!(!auth.is_allowed_origin(Some("chrome-extension://rogue_id")));
    }
}
