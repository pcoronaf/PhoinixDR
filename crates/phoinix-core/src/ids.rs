//! Opaque domain identifiers.
//!
//! Identifiers are UUIDs wrapped in newtypes so that a source cannot be
//! confused with a volume or a candidate at compile time. Integer database
//! IDs must never be exposed as domain identifiers.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! define_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Creates a fresh random identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wraps an existing UUID.
            #[must_use]
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// Returns the underlying UUID.
            #[must_use]
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }

            /// The all-zero identifier, useful for tests and placeholders.
            #[must_use]
            pub const fn nil() -> Self {
                Self(Uuid::nil())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl From<Uuid> for $name {
            fn from(uuid: Uuid) -> Self {
                Self(uuid)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

define_id! {
    /// Identifies a storage source: a physical device, an image file, or a
    /// derived view such as a partition.
    SourceId
}

define_id! {
    /// Identifies a volume (a filesystem instance) discovered on a source.
    VolumeId
}

define_id! {
    /// Identifies a recovery candidate produced by a scan.
    CandidateId
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn ids_are_distinct_and_round_trip() {
        let a = SourceId::new();
        let b = SourceId::new();
        assert_ne!(a, b);
        let text = a.to_string();
        let parsed: SourceId = text.parse().unwrap();
        assert_eq!(a, parsed);
    }

    #[test]
    fn serde_is_transparent() {
        let id = CandidateId::nil();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"00000000-0000-0000-0000-000000000000\"");
        let back: CandidateId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }
}
