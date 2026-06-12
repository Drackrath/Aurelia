pub mod cli;
pub mod session;
pub mod event_log;
pub mod wine_capture;
pub mod debug_utils;

pub use cli::*;
pub use session::*;
pub use event_log::*;
pub use wine_capture::*;
pub use debug_utils::*;

use std::collections::HashMap;

/// Substrings (matched case-insensitively) that mark a map key as carrying a
/// secret. Any value whose key contains one of these is replaced before it is
/// written to a log on disk. Shared by env and metadata redaction so the two
/// can't drift apart.
pub const SENSITIVE_KEY_FRAGMENTS: &[&str] = &[
    "STEAM_TOKEN",
    "STEAM_PASSWORD",
    "TOKEN",
    "PASSWORD",
    "REFRESH_TOKEN",
    "SESSION_TOKEN",
    "SECRET",
];

/// Replace the values of any secret-bearing keys in `map` with `[REDACTED]`.
pub fn redact_sensitive(mut map: HashMap<String, String>) -> HashMap<String, String> {
    for (key, value) in map.iter_mut() {
        let upper_key = key.to_uppercase();
        if SENSITIVE_KEY_FRAGMENTS.iter().any(|frag| upper_key.contains(frag)) {
            *value = "[REDACTED]".to_string();
        }
    }
    map
}

#[cfg(test)]
mod tests;
