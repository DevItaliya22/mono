// SPDX-License-Identifier: MPL-2.0

//! Field-only streaming validation before the existing PeerFrame decoder.
//! Does not materialize byte arrays as JSON Values. Device decoding is untouched.
use super::Error;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::fmt;

#[derive(Clone, Copy)]
enum Shape {
    Peer,
    Profile,
    Identity,
    Unit,
    Workspace,
    Replica,
    Document,
    Blob,
    Attribute,
    Open,
    Scalar,
}
struct Fields(Shape);
impl<'de> DeserializeSeed<'de> for Fields {
    type Value = Option<String>;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Fields {
    type Value = Option<String>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a defined PeerFrame field")
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        // Only short discriminators are kept; content strings are never copied.
        Ok((value.len() <= 64).then(|| value.to_owned()))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element_seed(Fields(self.0))?.is_some() {}
        Ok(None)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut keys = std::collections::BTreeSet::<String>::new();
        let mut kind = None;
        while let Some(key) = map.next_key::<String>()? {
            if keys.contains(&key) {
                return Err(de::Error::custom("duplicate field"));
            }
            // Structural maps have a small fixed vocabulary; open blob attribute
            // maps retain the existing extensible metadata contract.
            if !matches!(self.0, Shape::Open) && (key.len() > 64 || keys.len() >= 32) {
                return Err(de::Error::custom("unknown field"));
            }
            let child = match (self.0, key.as_str()) {
                (Shape::Peer, "profiles") => Shape::Profile,
                (Shape::Peer, "workspaces") => Shape::Workspace,
                (Shape::Peer, "unit") | (Shape::Profile, "unit") => Shape::Unit,
                (Shape::Profile, "identity") => Shape::Identity,
                (Shape::Peer, "replica") => Shape::Replica,
                (Shape::Peer, "documents") => Shape::Document,
                (Shape::Peer, "blobs") => Shape::Blob,
                (Shape::Blob, "attributes") => Shape::Open,
                (Shape::Open, _) => Shape::Attribute,
                _ => Shape::Scalar,
            };
            let value = map.next_value_seed(Fields(child))?;
            if key == "kind" || matches!(self.0, Shape::Unit) && key == "type" {
                kind = value;
            }
            keys.insert(key);
        }
        let allowed: &[&str] = match self.0 {
            Shape::Peer => match kind.as_deref() {
                Some("sync_request" | "sync_ack" | "catalog_request") => &["kind"],
                Some("catalog") => &["kind", "digest", "profiles", "workspaces"],
                Some("profile_request") => &["kind", "unit", "version"],
                Some("profile_updates") => &["kind", "unit", "updates"],
                Some("inventory_request") => &["kind", "workspace_id"],
                Some("cloud_compare") => &["kind", "workspace_id", "documents"],
                Some("cloud_compared") => &["kind", "workspace_id", "matching"],
                Some("inventory") => &["kind", "workspace_id", "replica", "documents", "blobs"],
                Some("loro") => &["kind", "workspace_id", "message"],
                Some("loro_batch") => &["kind", "workspace_id", "messages"],
                Some("error") => &["kind", "code"],
                _ => return Err(de::Error::custom("non-document Cloud frame")),
            },
            Shape::Profile => &["unit", "version", "identity"],
            Shape::Identity => &[
                "name",
                "purpose",
                "avatar",
                "icon",
                "color",
                "account",
                "locale",
                "created_at_ms",
                "kind",
            ],
            Shape::Unit => match kind.as_deref() {
                Some("identity" | "private" | "legacy_private") => &["type", "profile"],
                Some("personal") => &["type", "owner", "workspace"],
                _ => return Err(de::Error::custom("unknown Profile unit")),
            },
            Shape::Workspace => &["workspace_id", "profile_id", "digest"],
            Shape::Replica => &[
                "profile_id",
                "profile_name",
                "profile_locale",
                "profile_created_at_ms",
                "profile_icon",
                "profile_color",
                "profile_kind",
                "workspace_id",
                "workspace_name",
                "workspace_created_at_ms",
                "workspace_icon",
                "workspace_color",
                "workspace_pinned",
                "workspace_archived",
                "workspace_deleted_at_ms",
                "manifest_document",
            ],
            Shape::Document => &["document_id", "digest", "room_id", "version", "is_manifest"],
            Shape::Blob => &[
                "hash",
                "size",
                "mime",
                "filename",
                "created_at",
                "attributes",
            ],
            Shape::Attribute => &["type", "value"],
            // Arbitrary attribute names, not arbitrary attribute value fields.
            Shape::Open => return Ok(None),
            Shape::Scalar => return Err(de::Error::custom("unexpected object")),
        };
        if keys.iter().any(|key| !allowed.contains(&key.as_str())) {
            return Err(de::Error::custom("unknown field"));
        }
        Ok(None)
    }
}
pub(super) fn validate(bytes: &[u8]) -> Result<(), Error> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    Fields(Shape::Peer)
        .deserialize(&mut decoder)
        .map_err(|_| Error::Invalid)?;
    decoder.end().map_err(|_| Error::Invalid)
}
