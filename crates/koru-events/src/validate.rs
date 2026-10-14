// SPDX-License-Identifier: MPL-2.0

//! The one validation, called from both sides of the wire.
//!
//! The SDK validates before a send so a mistake fails at the call site with a
//! nameable reason; the events worker validates again on ingest because the
//! wire is a public boundary and not every client is ours. Both call this —
//! two enforcement points, one implementation, zero drift.

use std::fmt;

use crate::envelope::{AttrValue, Envelope};
use crate::registry::{self, AttrKind};

/// Why an envelope was refused. `code()` is the stable ASCII code an API
/// carries; `Display` is untranslated developer text a client must not show
/// to a person (PRINCIPLES: APIs return stable codes, not prose).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Violation {
    UnknownName {
        name: String,
    },
    MalformedName {
        name: String,
    },
    WrongVersion {
        name: String,
        declared: u16,
        given: u16,
    },
    UndeclaredAttr {
        name: String,
        key: String,
    },
    MissingAttr {
        name: String,
        key: String,
    },
    WrongAttrKind {
        name: String,
        key: String,
        expected: AttrKind,
    },
    TokenOutsideVocabulary {
        name: String,
        key: String,
        value: String,
    },
    MalformedVersion {
        name: String,
        key: String,
        value: String,
    },
    BadActorToken {
        token: String,
    },
    BadTimestamp {
        given: i64,
    },
}

impl Violation {
    pub fn code(&self) -> &'static str {
        match self {
            Violation::UnknownName { .. } => "unknown_event",
            Violation::MalformedName { .. } => "malformed_name",
            Violation::WrongVersion { .. } => "unknown_version",
            Violation::UndeclaredAttr { .. } => "undeclared_attr",
            Violation::MissingAttr { .. } => "missing_attr",
            Violation::WrongAttrKind { .. } => "wrong_attr_kind",
            Violation::TokenOutsideVocabulary { .. } => "token_outside_vocabulary",
            Violation::MalformedVersion { .. } => "malformed_version",
            Violation::BadActorToken { .. } => "bad_actor_token",
            Violation::BadTimestamp { .. } => "bad_timestamp",
        }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Violation::UnknownName { name } => {
                write!(f, "`{name}` is not in the event registry")
            }
            Violation::MalformedName { name } => {
                write!(f, "`{name}` does not match the event-name grammar")
            }
            Violation::WrongVersion {
                name,
                declared,
                given,
            } => {
                write!(f, "`{name}` is declared at version {declared}, not {given}")
            }
            Violation::UndeclaredAttr { name, key } => {
                write!(f, "`{name}` declares no attr `{key}`")
            }
            Violation::MissingAttr { name, key } => {
                write!(f, "`{name}` requires attr `{key}`")
            }
            Violation::WrongAttrKind {
                name,
                key,
                expected,
            } => {
                write!(f, "`{name}`.`{key}` takes a {} value", expected.as_str())
            }
            Violation::TokenOutsideVocabulary { name, key, value } => {
                write!(f, "`{value}` is not in `{name}`.`{key}`'s vocabulary")
            }
            Violation::MalformedVersion { name, key, value } => {
                write!(
                    f,
                    "`{name}`.`{key}` takes digits and dots; `{value}` is not one"
                )
            }
            Violation::BadActorToken { token } => {
                write!(f, "actor token `{token}` is not a lowercase name token")
            }
            Violation::BadTimestamp { given } => {
                write!(
                    f,
                    "occurred_at_ms {given} is not a positive epoch-ms timestamp"
                )
            }
        }
    }
}

impl std::error::Error for Violation {}

/// Refuse anything the registry does not declare. First violation wins — a
/// boundary rejecting an event names one precise reason, not a list.
pub fn validate(envelope: &Envelope) -> Result<(), Violation> {
    let name = envelope.name.as_str();
    if !registry::name_is_well_formed(name) {
        return Err(Violation::MalformedName {
            name: name.to_string(),
        });
    }
    let Some(spec) = registry::spec(name) else {
        return Err(Violation::UnknownName {
            name: name.to_string(),
        });
    };
    if envelope.version != spec.version {
        return Err(Violation::WrongVersion {
            name: name.to_string(),
            declared: spec.version,
            given: envelope.version,
        });
    }
    if envelope.occurred_at_ms <= 0 {
        return Err(Violation::BadTimestamp {
            given: envelope.occurred_at_ms,
        });
    }
    if let Some(token) = &envelope.actor.token
        && !token_is_well_formed(token)
    {
        return Err(Violation::BadActorToken {
            token: token.clone(),
        });
    }
    for (key, value) in &envelope.attrs {
        let Some(attr) = spec.attr(key) else {
            return Err(Violation::UndeclaredAttr {
                name: name.to_string(),
                key: key.clone(),
            });
        };
        let matches = matches!(
            (attr.kind, value),
            (AttrKind::Id, AttrValue::Id(_))
                | (AttrKind::Time, AttrValue::Time(_))
                | (AttrKind::Token, AttrValue::Token(_))
                | (AttrKind::Count, AttrValue::Count(_))
                | (AttrKind::Int, AttrValue::Int(_))
                | (AttrKind::Version, AttrValue::Version(_))
                | (AttrKind::Bucket, AttrValue::Bucket(_))
                | (AttrKind::Flag, AttrValue::Flag(_))
        );
        if !matches {
            return Err(Violation::WrongAttrKind {
                name: name.to_string(),
                key: key.clone(),
                expected: attr.kind,
            });
        }
        if let AttrValue::Token(token) = value
            && !attr.vocabulary.contains(&token.as_str())
        {
            return Err(Violation::TokenOutsideVocabulary {
                name: name.to_string(),
                key: key.clone(),
                value: token.clone(),
            });
        }
        // Digits and dots, nothing else, and short — what makes a version
        // the one unbounded-vocabulary value that still cannot carry content.
        if let AttrValue::Version(version) = value
            && !version_is_well_formed(version)
        {
            return Err(Violation::MalformedVersion {
                name: name.to_string(),
                key: key.clone(),
                value: version.clone(),
            });
        }
    }
    for attr in spec.attrs.iter().filter(|attr| attr.required) {
        if !envelope.attrs.contains_key(attr.key) {
            return Err(Violation::MissingAttr {
                name: name.to_string(),
                key: attr.key.to_string(),
            });
        }
    }
    Ok(())
}

/// A version is dotted digits, at most 32 characters: `10.0.26200`.
///
/// `pub(crate)` for [`crate::telemetry::validate_event`]: the same shape is
/// what a version attr arriving in an exported batch is held to — one rule,
/// enforced at both boundaries, so the two cannot drift.
pub(crate) fn version_is_well_formed(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 32
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// An actor token is a name token, never prose: `[a-z0-9_-]`, 1–64 chars.
fn token_is_well_formed(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 64
        && token
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}
