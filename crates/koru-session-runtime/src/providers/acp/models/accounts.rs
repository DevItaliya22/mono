// SPDX-License-Identifier: MPL-2.0

//! Explicit connection metadata only. Credential strings are consumed and discarded.
use super::*;
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use std::{
    collections::BTreeSet,
    fmt,
    io::{BufReader, Read},
};

const MAX_ACCOUNT_BYTES: u64 = 1024 * 1024;
const MAX_CONNECTIONS: usize = 4096;

type Connections = BTreeSet<ModelName>;

pub(super) fn environment_content(route: &AcpRoute) -> Result<Option<String>, ProviderFailure> {
    let value = route
        .environment
        .iter()
        .rev()
        .find(|(key, _)| {
            if cfg!(windows) {
                key.to_string_lossy()
                    .eq_ignore_ascii_case("OPENCODE_AUTH_CONTENT")
            } else {
                key == "OPENCODE_AUTH_CONTENT"
            }
        })
        .map(|(_, value)| value.clone())
        .or_else(|| std::env::var_os("OPENCODE_AUTH_CONTENT"));
    value
        .filter(|value| !value.is_empty())
        .map(|value| {
            let content = value.into_string().map_err(|_| invalid_catalog())?;
            if content.len() as u64 > MAX_ACCOUNT_BYTES {
                return Err(invalid_catalog());
            }
            Ok(content)
        })
        .transpose()
}

pub(super) fn data_path(bytes: &[u8]) -> Result<PathBuf, ProviderFailure> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid_catalog())?;
    let mut found = None;
    for line in text.lines() {
        let Some((key, path)) = line.split_once(' ') else {
            continue;
        };
        if key != "data" {
            continue;
        }
        let path = path.trim_start();
        if found.is_some() || path.chars().any(char::is_control) {
            return Err(invalid_catalog());
        }
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(invalid_catalog());
        }
        found = Some(path);
    }
    found.ok_or_else(invalid_catalog)
}

pub(super) fn read(data: PathBuf) -> Result<Connections, ProviderFailure> {
    let path = data.join("auth.json");
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Connections::new()),
        Err(_) => return Err(invalid_catalog()),
    };
    let metadata = file.metadata().map_err(|_| invalid_catalog())?;
    if !metadata.is_file() || metadata.len() > MAX_ACCOUNT_BYTES {
        return Err(invalid_catalog());
    }
    parse_reader(file)
}

pub(super) fn parse(bytes: &[u8]) -> Result<Connections, ProviderFailure> {
    parse_reader(bytes)
}

fn parse_reader(reader: impl Read) -> Result<Connections, ProviderFailure> {
    let mut reader = reader.take(MAX_ACCOUNT_BYTES + 1);
    let connections = {
        let mut decoder = serde_json::Deserializer::from_reader(BufReader::new(&mut reader));
        let connections =
            ConnectionKeys::deserialize(&mut decoder).map_err(|_| invalid_catalog())?;
        decoder.end().map_err(|_| invalid_catalog())?;
        connections.0
    };
    if reader.limit() == 0 {
        return Err(invalid_catalog());
    }
    Ok(connections)
}

struct ConnectionKeys(Connections);
impl<'de> Deserialize<'de> for ConnectionKeys {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeysVisitor;
        impl<'de> Visitor<'de> for KeysVisitor {
            type Value = ConnectionKeys;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("bounded account metadata")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut connections = Connections::new();
                let mut count = 0;
                while let Some(key) = map.next_key::<String>()? {
                    count += 1;
                    if count > MAX_CONNECTIONS {
                        return Err(de::Error::custom("too many connections"));
                    }
                    let entry = map.next_value::<ConnectionShape>()?;
                    if !entry.valid() {
                        return Err(de::Error::custom("invalid account metadata"));
                    }
                    // Well-known URL authentication is not a model provider ID.
                    if key.contains('/') {
                        continue;
                    }
                    let id = key
                        .parse()
                        .map_err(|_| de::Error::custom("invalid provider identifier"))?;
                    if !connections.insert(id) {
                        return Err(de::Error::custom("duplicate connection"));
                    }
                }
                Ok(ConnectionKeys(connections))
            }
        }
        deserializer.deserialize_map(KeysVisitor)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ConnectionKind {
    Api,
    Oauth,
    Wellknown,
}

#[derive(Deserialize)]
struct ConnectionShape {
    #[serde(rename = "type")]
    kind: ConnectionKind,
    #[serde(default, deserialize_with = "present_shape")]
    key: Option<ValueShape>,
    #[serde(default, deserialize_with = "present_shape")]
    access: Option<ValueShape>,
    #[serde(default, deserialize_with = "present_shape")]
    refresh: Option<ValueShape>,
    #[serde(default, deserialize_with = "present_shape")]
    token: Option<ValueShape>,
    #[serde(default, deserialize_with = "present_shape")]
    expires: Option<ValueShape>,
    #[serde(default, rename = "accountId", deserialize_with = "present_shape")]
    account_id: Option<ValueShape>,
    #[serde(default, rename = "enterpriseUrl", deserialize_with = "present_shape")]
    enterprise_url: Option<ValueShape>,
    #[serde(default, deserialize_with = "present_shape")]
    metadata: Option<ValueShape>,
}
impl ConnectionShape {
    fn valid(&self) -> bool {
        use ValueShape::{NonNegativeInteger, String, StringMap};
        match self.kind {
            ConnectionKind::Api => {
                self.key == Some(String) && self.metadata.is_none_or(|shape| shape == StringMap)
            }
            ConnectionKind::Oauth => {
                self.access == Some(String)
                    && self.refresh == Some(String)
                    && self.expires == Some(NonNegativeInteger)
                    && self.account_id.is_none_or(|shape| shape == String)
                    && self.enterprise_url.is_none_or(|shape| shape == String)
            }
            ConnectionKind::Wellknown => self.key == Some(String) && self.token == Some(String),
        }
    }
}

fn present_shape<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ValueShape>, D::Error> {
    ValueShape::deserialize(deserializer).map(Some)
}

/// Retain only structural categories, never credential text or numeric values.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueShape {
    String,
    NonNegativeInteger,
    StringMap,
    Other,
}
impl<'de> Deserialize<'de> for ValueShape {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ShapeVisitor;
        impl<'de> Visitor<'de> for ShapeVisitor {
            type Value = ValueShape;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("account value shape")
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(ValueShape::String)
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(ValueShape::Other)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(ValueShape::Other)
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(if value >= 0 {
                    ValueShape::NonNegativeInteger
                } else {
                    ValueShape::Other
                })
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(ValueShape::NonNegativeInteger)
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                Ok(if value >= 0.0 && value.fract() == 0.0 {
                    ValueShape::NonNegativeInteger
                } else {
                    ValueShape::Other
                })
            }
            fn visit_seq<S: de::SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
                while seq.next_element::<de::IgnoredAny>()?.is_some() {}
                Ok(ValueShape::Other)
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut strings = true;
                while let Some((_, value)) = map.next_entry::<de::IgnoredAny, ValueShape>()? {
                    strings &= value == ValueShape::String;
                }
                Ok(if strings {
                    ValueShape::StringMap
                } else {
                    ValueShape::Other
                })
            }
        }
        deserializer.deserialize_any(ShapeVisitor)
    }
}

pub(super) fn intersect(
    models: ProviderModels,
    connections: &Connections,
) -> Result<ProviderModels, ProviderFailure> {
    models
        .models()
        .iter()
        .filter(|model| connections.contains(&model.group))
        .cloned()
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| invalid_catalog())
}

#[cfg(test)]
mod tests;
