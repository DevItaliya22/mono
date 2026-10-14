// SPDX-License-Identifier: MPL-2.0

use super::*;
use serde_json::json;

fn compacted(schema: Value) -> Value {
    Value::Object(compact_input_schema(schema.as_object().unwrap()))
}

#[test]
fn a_tagged_union_becomes_one_closed_object_that_names_every_variant() {
    let schema = json!({
        "type": "object",
        "properties": {
            "operations": {
                "type": "array",
                "items": {
                    "oneOf": [
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "description": "Remove one node.",
                            "properties": {
                                "op": {"type": "string", "const": "remove"},
                                "node_id": {"type": "string", "description": "UUID."}
                            },
                            "required": ["op", "node_id"]
                        },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "op": {"type": "string", "const": "set_value"},
                                "node_id": {"type": "string"},
                                "value": {"type": "number", "format": "double"},
                                "note": {"type": ["string", "null"], "default": null}
                            },
                            "required": ["op", "node_id", "value"]
                        },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "op": {"type": "string", "const": "set_flag"},
                                "value": {"type": "boolean"}
                            },
                            "required": ["op", "value"]
                        }
                    ]
                }
            }
        }
    });
    assert_eq!(
        compacted(schema),
        json!({
            "type": "object",
            "properties": {
                "operations": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["op"],
                        "description": "remove(node_id) | set_value(node_id, value, note?) | set_flag(value)",
                        "properties": {
                            "op": {"type": "string", "enum": ["remove", "set_value", "set_flag"]},
                            "node_id": {"type": "string"},
                            "note": {"type": ["string", "null"]},
                            "value": {"type": ["number", "boolean"]}
                        }
                    }
                }
            }
        })
    );
}

#[test]
fn a_reference_is_inlined_with_its_default_and_defs_are_gone() {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Request",
        "description": "A developer's note.",
        "type": "object",
        "$defs": {
            "Sort": {"type": "string", "enum": ["due", "priority"], "description": "Order."},
            "Window": {
                "type": "object",
                "properties": {"after_ms": {"type": ["integer", "null"], "format": "int64"}}
            }
        },
        "properties": {
            "sort": {"$ref": "#/$defs/Sort", "default": "due"},
            "window": {"anyOf": [{"$ref": "#/$defs/Window"}, {"type": "null"}], "default": null},
            "limit": {"type": ["integer", "null"], "format": "uint", "minimum": 0},
            "tags": {"type": "array", "items": {"type": "string"}, "default": []},
            "title": {"type": "string", "description": "A property named title stays."}
        }
    });
    assert_eq!(
        compacted(schema),
        json!({
            "type": "object",
            "properties": {
                "sort": {"type": "string", "enum": ["due", "priority"], "default": "due"},
                "window": {
                    "type": ["object", "null"],
                    "properties": {"after_ms": {"type": ["integer", "null"]}}
                },
                "limit": {"type": ["integer", "null"], "minimum": 0},
                "tags": {"type": "array", "items": {"type": "string"}},
                "title": {"type": "string"}
            }
        })
    );
}

#[test]
fn a_documented_unit_enum_becomes_an_enum() {
    let schema = json!({
        "type": "object",
        "properties": {
            "lifecycle": {"oneOf": [
                {"type": "string", "const": "open", "description": "Not done."},
                {"type": "string", "const": "all"}
            ]}
        }
    });
    assert_eq!(
        compacted(schema),
        json!({
            "type": "object",
            "properties": {"lifecycle": {"type": "string", "enum": ["open", "all"]}}
        })
    );
}

#[test]
fn fields_that_differ_in_more_than_type_accept_anything_and_leave_it_to_the_server() {
    let schema = json!({
        "oneOf": [
            {"type": "object", "properties": {"t": {"const": "a"}, "x": {"type": "string", "enum": ["p"]}}, "required": ["t"]},
            {"type": "object", "properties": {"t": {"const": "b"}, "x": {"type": "object"}}, "required": ["t"]}
        ]
    });
    let out = compacted(schema);
    assert_eq!(out["properties"]["x"], json!({}));
    assert!(
        out.get("additionalProperties").is_none(),
        "branches were open"
    );
}
