//! Redacted, zeroized secret strings.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroize;

/// Token or password; never printed.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Raw value, for sending only.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_the_value() {
        let secret = Secret::new("eyJhbGciOi.refresh");
        assert!(!format!("{secret:?}").contains("eyJ"));
        assert!(!format!("{:?}", Some(&secret)).contains("refresh"));
    }

    #[test]
    fn serde_round_trips_as_plain_string() {
        let json = serde_json::to_string(&Some(Secret::new("tok"))).unwrap();
        assert_eq!(json, "\"tok\"");
        let back: Option<Secret> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.unwrap().expose(), "tok");
    }
}
