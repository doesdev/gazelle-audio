//! Localhost-only access: per-start bearer token, Host and Origin checks.

use std::io::Write;
use std::path::{Path, PathBuf};

use axum::http::{header, HeaderMap};

/// 32 random bytes as lowercase hex.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("operating system random source");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `Authorization: Bearer <token>`.
pub fn bearer_ok(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| constant_time_eq(t.as_bytes(), token.as_bytes()))
}

/// Rejects DNS-rebinding requests: the Host must name the loopback listener itself.
pub fn host_allowed(host: Option<&str>, port: u16) -> bool {
    host.is_some_and(|h| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}"))
}

/// Browsers always send Origin on WebSocket handshakes; non-browser clients may omit it.
pub fn origin_allowed(origin: Option<&str>, port: u16) -> bool {
    match origin {
        None => true,
        Some(o) => o == format!("http://127.0.0.1:{port}") || o == format!("http://localhost:{port}"),
    }
}

/// `%LOCALAPPDATA%\gazelle\capture-token`; elsewhere `$HOME/.local/state/gazelle/capture-token`.
pub fn token_path(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(local) = var("LOCALAPPDATA") {
        return Some(PathBuf::from(local).join("gazelle").join("capture-token"));
    }
    var("HOME").map(|home| PathBuf::from(home).join(".local/state/gazelle/capture-token"))
}

/// Writes the token readable by the current user only. On Windows the per-user
/// `%LOCALAPPDATA%` ACL provides that; on Unix the file is created with mode 0600.
pub fn write_token(path: &Path, token: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(token.as_bytes())
}
