// SPDX-License-Identifier: MPL-2.0

use loro::{ExportMode, LoroDoc, LoroValue};

use crate::Post;

pub const POST_KIND: &str = "koru-social-post";
pub const SCHEMA_VERSION: i64 = 3;

#[derive(Debug)]
pub enum DocumentError {
    UnsupportedVersion,
    InvalidContent,
}

/// Each top-level authoring field has its own CRDT register. A document is
/// one post, never the entire Socials workspace or a provider credential.
pub struct Document {
    pub loro: LoroDoc,
}

impl Document {
    pub fn new(post: &Post) -> Result<Self, DocumentError> {
        let doc = Self {
            loro: LoroDoc::new(),
        };
        doc.loro
            .get_map("social")
            .insert("schema", SCHEMA_VERSION)
            .map_err(|_| DocumentError::InvalidContent)?;
        doc.write(post)?;
        Ok(doc)
    }

    pub fn from_snapshot(bytes: &[u8]) -> Result<Self, DocumentError> {
        let doc = Self {
            loro: LoroDoc::new(),
        };
        doc.loro
            .import(bytes)
            .map_err(|_| DocumentError::InvalidContent)?;
        doc.read()?;
        Ok(doc)
    }

    pub fn read(&self) -> Result<Post, DocumentError> {
        let value = self.loro.get_map("social").get_deep_value();
        let LoroValue::Map(values) = value else {
            return Err(DocumentError::InvalidContent);
        };
        if !matches!(
            values.get("schema"),
            Some(LoroValue::I64(1 | 2 | SCHEMA_VERSION))
        ) {
            return Err(DocumentError::UnsupportedVersion);
        }
        let mut fields = serde_json::Map::new();
        for (key, value) in values.iter().filter(|(key, _)| key.as_str() != "schema") {
            let LoroValue::String(encoded) = value else {
                return Err(DocumentError::InvalidContent);
            };
            fields.insert(
                key.to_string(),
                serde_json::from_str(encoded.as_str())
                    .map_err(|_| DocumentError::InvalidContent)?,
            );
        }
        serde_json::from_value(serde_json::Value::Object(fields))
            .map_err(|_| DocumentError::InvalidContent)
    }

    pub fn write(&self, post: &Post) -> Result<(), DocumentError> {
        let map = self.loro.get_map("social");
        let LoroValue::Map(current) = map.get_deep_value() else {
            return Err(DocumentError::InvalidContent);
        };
        if !matches!(
            current.get("schema"),
            Some(LoroValue::I64(1 | 2 | SCHEMA_VERSION))
        ) {
            return Err(DocumentError::UnsupportedVersion);
        }
        if current.get("schema") != Some(&LoroValue::I64(SCHEMA_VERSION)) {
            map.insert("schema", SCHEMA_VERSION)
                .map_err(|_| DocumentError::InvalidContent)?;
        }
        let serde_json::Value::Object(fields) =
            serde_json::to_value(post).map_err(|_| DocumentError::InvalidContent)?
        else {
            return Err(DocumentError::InvalidContent);
        };
        for (key, value) in fields {
            let encoded =
                serde_json::to_string(&value).map_err(|_| DocumentError::InvalidContent)?;
            // Avoid assigning unchanged registers, preserving other writers' work.
            if current.get(&key) != Some(&LoroValue::String(encoded.as_str().into())) {
                map.insert(&key, encoded)
                    .map_err(|_| DocumentError::InvalidContent)?;
            }
        }
        self.loro.commit();
        Ok(())
    }

    pub fn snapshot(&self) -> Result<Vec<u8>, DocumentError> {
        self.loro
            .export(ExportMode::Snapshot)
            .map_err(|_| DocumentError::InvalidContent)
    }
}
