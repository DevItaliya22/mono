// SPDX-License-Identifier: MPL-2.0

//! Synthetic fixture capture and fresh-process measurement. Never opens a Store.
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use koru_core::{
    WorkspaceId,
    node::Node,
    schema::{Parent, WorkspaceDoc},
};
use koru_device::cloud::{CloudId, CloudReplicaPrincipalV1};
use koru_replication::cloud::object::*;
use loro::{ExportMode, LoroDoc, VersionVector};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Instant;

const GOLDENS: &str =
    include_str!("../../../../contracts/vectors/cloud-loro-object-v1/objects.json");
pub const JSON_CAP: u64 = 8 * 1024 * 1024;
const RAW_CAP: usize = 4 * 1024 * 1024;
const MAX_EDITS: usize = 4096;
const MAX_INSERTED: usize = 16 * 1024 * 1024;
const TAIL: usize = 32;

fn err(e: impl std::fmt::Debug) -> String {
    format!("{e:?}")
}
fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(s: &str) -> Vec<u8> {
    assert_eq!(s.len() % 2, 0);
    s.as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
fn id(v: &Value) -> CloudId {
    serde_json::from_value(v.clone()).unwrap()
}
fn limits() -> Limits {
    Limits::new(4 * 1024 * 1024, 1024 * 1024, 16 * 1024 * 1024, 1024 * 1024).unwrap()
}
fn goldens() -> Vec<Value> {
    serde_json::from_str(GOLDENS).unwrap()
}
fn metadata(v: &Value) -> Metadata {
    let dataset = if v["dataset_kind"] == 1 {
        CloudReplicaPrincipalV1::Workspace {
            workspace_id: id(&v["authority"]),
            incarnation_id: id(&v["lineage"]),
            logical_replica_id: id(&v["replica"]),
        }
    } else {
        CloudReplicaPrincipalV1::Profile {
            profile_id: id(&v["authority"]),
            lineage_id: id(&v["lineage"]),
            logical_replica_id: id(&v["replica"]),
        }
    };
    let unit = match v["unit_kind"].as_u64().unwrap() {
        1 => Unit::WorkspaceManifest,
        2 => Unit::WorkspaceDocument {
            document_id: id(&v["subject"]),
        },
        3 => Unit::ProfileIdentity,
        4 => Unit::ProfilePrivate,
        5 => Unit::ProfilePersonal {
            workspace_id: id(&v["subject"]),
        },
        _ => panic!("closed fixture unit"),
    };
    Metadata {
        kind: if v["object_kind"] == 1 {
            Kind::Checkpoint
        } else {
            Kind::Segment
        },
        dataset,
        unit,
        cloud_unit_id: id(&v["unit_id"]),
        object_id: id(&v["object_id"]),
        sequence: v["sequence"].as_u64().unwrap(),
        writer_worker_id: id(&v["worker"]),
        writer_lease_id: id(&v["lease_id"]),
        writer_lease_epoch: v["lease_epoch"].as_u64().unwrap(),
        authority_workspace_id: id(&v["authority_workspace"]),
        authorization_version: v["authorization_version"].as_u64().unwrap(),
        membership_epoch: v["membership_epoch"].as_u64().unwrap(),
        cloud_access_epoch: v["cloud_access_epoch"].as_u64().unwrap(),
        covers_through_sequence: v["coverage"].as_u64().unwrap(),
    }
}
fn expected(v: &Value) -> ExpectedObject<'_> {
    let h = |k: &str| unhex(v[k].as_str().unwrap()).try_into().unwrap();
    let optional = |k: &str| v[k].as_str().map(|s| unhex(s).try_into().unwrap());
    ExpectedObject {
        metadata: metadata(v),
        logical_key: v["key"].as_str().unwrap(),
        commitments: Commitments {
            unit_ref_digest: h("unit_digest"),
            predecessor_version_digest: optional("predecessor_digest"),
            result_version_digest: h("result_digest"),
            checkpoint_frontier_digest: optional("frontier_digest"),
            payload_sha256: h("payload_sha"),
            object_sha256: h("object_sha"),
            request_digest: h("request_digest"),
            payload_byte_length: v["payload_length"].as_u64().unwrap(),
            object_byte_length: v["object_length"].as_u64().unwrap(),
        },
    }
}

// Captured expectations come from producer fixture intent and its raw bytes,
// never from decode().metadata()/commitments() in the measured consumer.
fn capture(mut intent: Value, body: Body<'_>, sequence: u64) -> Result<Value, String> {
    let checkpoint = sequence == 1;
    intent["object_kind"] = json!(if checkpoint { 1 } else { 2 });
    intent["sequence"] = json!(sequence);
    intent["coverage"] = json!(0);
    intent["object_id"] = json!(format!("00000000-0000-4000-8000-{sequence:012x}"));
    intent["key"] = json!(format!(
        "w/{}/{}/{}-{}-{}.loro",
        intent["authority"].as_str().unwrap(),
        intent["subject"].as_str().unwrap(),
        intent["lineage"].as_str().unwrap(),
        if checkpoint { "c" } else { "s" },
        intent["object_id"].as_str().unwrap()
    ));
    let raw = encode(metadata(&intent), body, limits()).map_err(err)?;
    // SHA of the captured exact bytes is a measurement input, not a SQL receipt.
    let header_len = u32::from_be_bytes(raw[10..14].try_into().unwrap()) as usize;
    let mut request = Sha256::new();
    request.update(b"koru/cloud/loro-object-request/v1\0");
    request.update(&raw[22..22 + header_len]);
    for (k, bytes) in [
        ("result_digest", body.result_version),
        ("payload_sha", body.payload),
        ("object_sha", raw.as_slice()),
    ] {
        intent[k] = json!(hex(&hash(bytes)));
    }
    intent["request_digest"] = json!(hex(&request.finalize()));
    intent["predecessor_digest"] = if checkpoint {
        Value::Null
    } else {
        json!(hex(&hash(body.predecessor_version)))
    };
    intent["frontier_digest"] = if checkpoint {
        json!(hex(&hash(body.checkpoint_frontiers)))
    } else {
        Value::Null
    };
    intent["payload_length"] = json!(body.payload.len());
    intent["object_length"] = json!(raw.len());
    // Drop the old golden payload/header fields: they are not these bytes.
    for k in ["payload", "result", "header"] {
        intent.as_object_mut().unwrap().remove(k);
    }
    Ok(
        json!({"expected":intent,"envelope":B64.encode(&raw),"payload":B64.encode(body.payload),
        "predecessor":B64.encode(body.predecessor_version),"result":B64.encode(body.result_version),"frontiers":B64.encode(body.checkpoint_frontiers)}),
    )
}

fn text(step: usize, bytes: usize) -> String {
    let mut s = String::with_capacity(bytes);
    let mut block = 0;
    while s.len() < bytes {
        s.push_str(&hex(&hash(
            format!("plan20 synthetic text {step}/{block}").as_bytes(),
        )));
        block += 1;
    }
    s.truncate(bytes);
    s
}

pub fn generate(family: &str, tier: usize) -> Result<Value, String> {
    if !matches!(family, "ordinary" | "history") || ![65536, 262144, 1048576].contains(&tier) {
        return Err("closed corpus plan".into());
    }
    let template = goldens()
        .into_iter()
        .find(|v| v["id"] == "O-2-C")
        .ok_or("golden template")?["expected"]
        .clone();
    let mut doc =
        WorkspaceDoc::create(WorkspaceId::new(), "Synthetic capacity document").map_err(err)?;
    doc.doc().set_peer_id(719).map_err(err)?;
    let node = doc
        .insert(Parent::Root, Node::task("synthetic").map_err(err)?)
        .map_err(err)?;
    doc.commit();
    let mut last = doc.export_snapshot().map_err(err)?;
    let (mut edits, mut inserted, mut saved_edits, mut saved_inserted) = (1, 9, 1, 9);
    let mut reason = "edit_budget";
    while edits + TAIL < MAX_EDITS && inserted + 1024 + TAIL * 64 <= MAX_INSERTED {
        let s = text(edits, 1024);
        if family == "ordinary" {
            doc.insert(Parent::Root, Node::task(&s).map_err(err)?)
                .map_err(err)?;
        } else {
            doc.set_text(node, &s).map_err(err)?;
        }
        doc.commit();
        edits += 1;
        inserted += s.len();
        // Export at bounded eight-edit intervals; retain a complete prior state.
        if edits % 8 != 0 {
            continue;
        }
        let bytes = doc.export_snapshot().map_err(err)?;
        if bytes.len() > tier {
            reason = "next_export_exceeded_tier";
            break;
        }
        last = bytes;
        saved_edits = edits;
        saved_inserted = inserted;
    }
    let mut replay = WorkspaceDoc::from_snapshot(&last).map_err(err)?;
    replay.doc().set_peer_id(719).map_err(err)?;
    let mut result = replay.doc().oplog_vv().encode();
    let fronts = replay.doc().state_frontiers().encode();
    let mut objects = vec![capture(
        template.clone(),
        Body {
            predecessor_version: &[],
            result_version: &result,
            checkpoint_frontiers: &fronts,
            payload: &last,
        },
        1,
    )?];
    for n in 0..TAIL {
        let before = replay.doc().oplog_vv();
        replay.set_text(node, &text(5000 + n, 64)).map_err(err)?;
        replay.commit();
        let payload = replay
            .doc()
            .export(ExportMode::updates(&before))
            .map_err(err)?;
        let next = replay.doc().oplog_vv().encode();
        objects.push(capture(
            template.clone(),
            Body {
                predecessor_version: &result,
                result_version: &next,
                checkpoint_frontiers: &[],
                payload: &payload,
            },
            n as u64 + 2,
        )?);
        result = next;
    }
    let bundle = json!({"format":"plan20-capacity-v1","family":family,"tier_ceiling_bytes":tier,
        "checkpoint_payload_bytes":last.len(),"stop_reason":reason,"generation_attempted_edits":edits,
        "generation_attempted_inserted_bytes":inserted,"captured_edits_including_segments":saved_edits+TAIL,
        "captured_inserted_bytes_including_segments":saved_inserted+TAIL*64,
        "segments":TAIL,"expected_head":hex(&hash(&result)),"origin":"first","objects":objects});
    check_raw_cap(&bundle)?;
    Ok(bundle)
}

struct Bytes {
    raw: Vec<u8>,
    payload: Vec<u8>,
    predecessor: Vec<u8>,
    result: Vec<u8>,
    frontiers: Vec<u8>,
}
impl Bytes {
    fn body(&self) -> Body<'_> {
        Body {
            payload: &self.payload,
            predecessor_version: &self.predecessor,
            result_version: &self.result,
            checkpoint_frontiers: &self.frontiers,
        }
    }
}
fn unpack(v: &Value) -> Result<Bytes, String> {
    let get = |key: &str| {
        B64.decode(v[key].as_str().ok_or("missing capture")?)
            .map_err(err)
    };
    Ok(Bytes {
        raw: get("envelope")?,
        payload: get("payload")?,
        predecessor: get("predecessor")?,
        result: get("result")?,
        frontiers: get("frontiers")?,
    })
}
fn check_raw_cap(bundle: &Value) -> Result<(), String> {
    let objects = bundle["objects"].as_array().ok_or("objects")?;
    if objects.is_empty() || objects.len() > 33 {
        return Err("object count".into());
    }
    let mut total = 0_usize;
    for v in objects {
        for key in ["envelope", "payload", "predecessor", "result", "frontiers"] {
            let value = v[key].as_str().ok_or("capture")?;
            // Refuse the encoded bound before decoding/allocation.
            if value.len() > RAW_CAP * 4 / 3 + 4 {
                return Err("encoded capture cap".into());
            }
            total = total
                .checked_add(B64.decode(value).map_err(err)?.len())
                .ok_or("overflow")?;
            if total > RAW_CAP {
                return Err("raw captured bundle cap".into());
            }
        }
    }
    Ok(())
}
fn phase<T>(f: impl FnOnce() -> Result<T, String>) -> Result<(T, Value), String> {
    let start_cpu = super::guard::cpu_ns()?;
    let start = Instant::now();
    let result = f()?;
    let wall = start.elapsed().as_nanos();
    let cpu = super::guard::cpu_ns()?.saturating_sub(start_cpu);
    Ok((result, json!({"wall_ns":wall as u64,"cpu_ns":cpu})))
}
pub fn measure(bundle: Value) -> Result<Value, String> {
    if bundle["format"] != "plan20-capacity-v1" {
        return Err("format".into());
    }
    check_raw_cap(&bundle)?;
    let captures = bundle["objects"].as_array().ok_or("objects")?;
    let bytes: Vec<Bytes> = captures.iter().map(unpack).collect::<Result<_, _>>()?;
    let (doc, checkpoint_import) = phase(|| {
        let d = LoroDoc::new();
        d.import(&bytes[0].payload).map_err(err)?;
        Ok(d)
    })?;
    let (_, segment_import) = phase(|| {
        for b in &bytes[1..] {
            doc.import(&b.payload).map_err(err)?;
        }
        Ok(())
    })?;
    // Encoding order is not canonical. Receipt hashes retain captured bytes;
    // this separate reconstruction comparison is semantic VV equality only.
    if doc.oplog_vv() != VersionVector::decode(&bytes.last().unwrap().result).map_err(err)? {
        return Err("semantic imported VV".into());
    }
    let peers = doc.oplog_vv().iter().count();
    let operations: u64 = doc.oplog_vv().iter().map(|(_, end)| *end as u64).sum();
    let frontiers = doc.state_frontiers().iter().count();
    let (exported, export) = phase(|| doc.export(ExportMode::Snapshot).map_err(err))?;
    let export_size = exported.len();
    drop(exported);
    drop(doc);
    let (encoded, encode_time) = phase(|| {
        captures
            .iter()
            .zip(&bytes)
            .map(|(v, b)| {
                let encoded = encode(metadata(&v["expected"]), b.body(), limits()).map_err(err)?;
                if encoded != b.raw {
                    return Err("encode differs from captured bytes".into());
                }
                Ok(encoded)
            })
            .collect::<Result<Vec<_>, String>>()
    })?;
    drop(encoded);
    let (bound, decode_bind) = phase(|| {
        captures
            .iter()
            .zip(&bytes)
            .map(|(v, b)| {
                decode(&b.raw, limits())
                    .map_err(err)?
                    .bind(expected(&v["expected"]))
                    .map_err(err)
            })
            .collect::<Result<Vec<_>, String>>()
    })?;
    let expected_head = HeadCommitment {
        result_version_digest: unhex(bundle["expected_head"].as_str().ok_or("head")?)
            .try_into()
            .map_err(|_| "head length")?,
    };
    let origin = if bundle["origin"] == "first" {
        CheckpointOrigin::FirstCloudCheckpoint
    } else {
        CheckpointOrigin::LaterCheckpoint
    };
    let (_, chain) = phase(|| {
        verify_chain(bound[0], origin, &bound[1..], expected_head, limits()).map_err(err)
    })?;
    Ok(
        json!({"family":bundle["family"],"tier_ceiling_bytes":bundle["tier_ceiling_bytes"],"checkpoint_payload_bytes":bytes[0].payload.len(),
        "segments":bytes.len()-1,"post_replay_export_bytes":export_size,"import_checkpoint":checkpoint_import,"import_segments":segment_import,"export_post_replay_snapshot":export,
        "encode_all_envelopes":encode_time,"decode_and_independent_bind_all":decode_bind,"verify_chain_fresh":chain,
        "actual_post_replay_peers":peers,"actual_post_replay_frontiers":frontiers,"actual_post_replay_oplog_counter_sum":operations,
        "head_sha256":hex(&expected_head.result_version_digest),"content_proof_only":true}),
    )
}

pub fn smoke(index: usize) -> Result<Value, String> {
    if index >= 5 {
        return Err("smoke index".into());
    }
    let g = goldens()
        .into_iter()
        .find(|v| v["id"] == format!("O-{}-C", index + 1))
        .ok_or("smoke golden")?;
    let raw = unhex(g["envelope"].as_str().unwrap());
    let e = &g["expected"];
    // Bound against the original golden expectation, not decoder-derived identity.
    let d = decode(&raw, limits()).map_err(err)?;
    let body = d.body();
    let bundle = json!({"format":"plan20-capacity-v1","origin":"later","family":format!("frozen-unit-{}",index+1),
        "tier_ceiling_bytes":1048576,"expected_head":e["result_digest"],"objects":[{
        "expected":e,"envelope":B64.encode(&raw),"payload":B64.encode(body.payload),"predecessor":B64.encode(body.predecessor_version),
        "result":B64.encode(body.result_version),"frontiers":B64.encode(body.checkpoint_frontiers)}]});
    measure(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_plan_and_small_text_are_bounded_without_generation() {
        assert!(generate("bad", 65536).is_err());
        assert!(generate("ordinary", 1073741824).is_err());
        assert_eq!(text(1, 64).len(), 64);
        assert_ne!(text(1, 64), text(2, 64));
        assert_eq!(MAX_EDITS, 4096);
        assert_eq!(MAX_INSERTED, 16777216);
    }
    #[test]
    fn missing_empty_and_too_many_captures_refuse_without_loro() {
        assert!(check_raw_cap(&json!({})).is_err());
        assert!(check_raw_cap(&json!({"objects":[]})).is_err());
        assert!(check_raw_cap(&json!({"objects":vec![json!({});34]})).is_err());
    }
}
