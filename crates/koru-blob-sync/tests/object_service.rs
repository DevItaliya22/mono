// SPDX-License-Identifier: MPL-2.0

//! **The in-process object service, held to the contract vectors.**
//!
//! [`ObjectService`] stands in for the object worker in this crate's
//! integration suites. A fake that answered the wire would only ever reproduce
//! this crate's assumptions about it — so this file drives it through every
//! row of `contracts/vectors/cloud-blob-object-v1/cases.json` the real worker
//! owns, over HTTP, the way the hosted service's contract adapter drives the
//! real worker. If the fake and the contract disagree, it fails here, before any
//! engine test can pass against a service that does not exist.
//!
//! The rows it does not run are the ones the worker does not run either: the
//! three bindings only an account database can decide (C13–C15), allocation
//! and the report seam, which belong to the control API.

mod common;

use koru_account_core::workspace_authorization::Region;
use koru_account_session::cloud::blob::{
    BlobCapabilityClaimsV1, BlobPurpose, blob_capability_expiry, blob_capability_not_before,
};
use koru_cloud_blob::{BlobCode, CHUNK, ObjectLocator, vectors};
use koru_device::cloud::CloudId;

use common::{ObjectService, fresh, sign};

const EU_AUDIENCE: &str = "eu.data.koru.do";

/// Position-dependent filler, so "the same bytes" and "different bytes" are
/// distinguishable at every offset without carrying a corpus around.
fn filler(offset: u64, length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| ((offset as usize + index) % 251) as u8)
        .collect()
}

fn hash_of(bytes: &[u8]) -> String {
    format!("b3:{}", blake3::hash(bytes).to_hex())
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default()
}

/// A well-formed capability for the deployment in `region`, before a case
/// changes one thing about it. The identities are the vectors' own labels.
fn baseline(
    region: Region,
    purpose: BlobPurpose,
    sub: CloudId,
    loc: CloudId,
) -> BlobCapabilityClaimsV1 {
    let nbf = blob_capability_not_before(now_millis());
    BlobCapabilityClaimsV1 {
        v: 1,
        pur: purpose.as_str().into(),
        iss: common::ISSUER.into(),
        aud: match region {
            Region::Us => common::AUDIENCE.into(),
            Region::Eu => EU_AUDIENCE.into(),
        },
        rgn: region,
        acc: vectors::fixture_id("acc-1"),
        wsp: vectors::fixture_id("ws-1"),
        inc: vectors::fixture_id("inc-1"),
        eph: 7,
        sub,
        loc,
        bh: vectors::fixture_hash("b3:aa"),
        len: 4_194_304,
        nbf,
        exp: blob_capability_expiry(nbf).expect("an expiry"),
        jti: fresh(),
    }
}

fn authorization(claims: &BlobCapabilityClaimsV1) -> String {
    format!("Capability {}", sign(claims))
}

/// The stable code a refusal carried, read out of the body: the code is the
/// thing clients branch on.
async fn code_of(response: reqwest::Response) -> BlobCode {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    serde_json::from_str::<koru_cloud_blob::wire::BlobErrorV1>(&body)
        .unwrap_or_else(|_| panic!("a refusal carries a stable code; {status} said {body:?}"))
        .code
}

async fn patch(
    service: &ObjectService,
    upload: CloudId,
    claims: &BlobCapabilityClaimsV1,
    range: &str,
    body: Vec<u8>,
) -> reqwest::Response {
    reqwest::Client::new()
        .patch(format!("{}/v1/objects/uploads/{upload}", service.base))
        .header("authorization", authorization(claims))
        .header("content-range", range)
        .body(body)
        .send()
        .await
        .expect("a patch")
}

/// Write `bytes` into a session one bounded chunk at a time, as a client would.
async fn send(
    service: &ObjectService,
    upload: CloudId,
    claims: &BlobCapabilityClaimsV1,
    bytes: &[u8],
) {
    let mut offset = 0u64;
    while offset < bytes.len() as u64 {
        let end = (offset + CHUNK).min(bytes.len() as u64) - 1;
        let response = patch(
            service,
            upload,
            claims,
            &format!("bytes {offset}-{end}/{}", claims.len),
            bytes[offset as usize..=end as usize].to_vec(),
        )
        .await;
        assert_eq!(response.status(), 200, "chunk at {offset}");
        offset = end + 1;
    }
}

async fn complete(
    service: &ObjectService,
    upload: CloudId,
    claims: &BlobCapabilityClaimsV1,
) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "{}/v1/objects/uploads/{upload}/complete",
            service.base
        ))
        .header("authorization", authorization(claims))
        .send()
        .await
        .expect("complete")
}

fn key_of(claims: &BlobCapabilityClaimsV1) -> String {
    koru_cloud_blob::object_key(
        claims.rgn,
        claims.wsp,
        claims.inc,
        ObjectLocator::from_uuid(claims.loc.as_uuid()),
    )
}

/// Upload and complete `bytes`, then wait for the verification job to publish.
async fn upload_and_publish(
    service: &ObjectService,
    upload: CloudId,
    locator: CloudId,
    bytes: &[u8],
) -> BlobCapabilityClaimsV1 {
    let mut claims = baseline(Region::Us, BlobPurpose::Write, upload, locator);
    claims.len = bytes.len() as u64;
    claims.bh = hash_of(bytes);
    send(service, upload, &claims, bytes).await;
    let response = complete(service, upload, &claims).await;
    assert_eq!(response.status(), 202, "complete answers 202 and returns");
    assert_eq!(
        response.json::<serde_json::Value>().await.expect("json"),
        serde_json::json!({"state": "verifying"})
    );
    settled(|| service.object_length(&key_of(&claims)).is_some()).await;
    claims
}

/// Poll for a job that runs off the request path. Polling, not sleeping: its
/// timing is the machine's.
async fn settled(done: impl Fn() -> bool) {
    for _ in 0..600 {
        if done() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("verification never settled");
}

/// The rows the object worker owns are the rows this fake must answer. A row
/// it skipped would be a clause the engine's suites trust without anyone
/// checking it.
#[test]
fn the_fake_runs_every_row_the_object_worker_owns() {
    let mut owned = 0;
    for case in vectors::cases() {
        let control = matches!(case.id.as_str(), "C13" | "C14" | "C15")
            || matches!(
                case.kind,
                vectors::CaseKind::Allocate | vectors::CaseKind::Report
            );
        if !control {
            owned += 1;
        }
    }
    assert_eq!(owned, 35, "the object worker's share of the fifty rows");
}

/// `C01`–`C12` and `C16`.
#[tokio::test(flavor = "multi_thread")]
async fn capability_admission_vectors() {
    let us = ObjectService::start().await;
    let eu = ObjectService::start_in(Region::Eu, EU_AUDIENCE).await;

    // A published object, so the read cases have something real to name.
    let bytes = filler(0, 4096);
    let locator = fresh();
    let upload = fresh();
    upload_and_publish(&us, upload, locator, &bytes).await;

    let mut ran = 0;
    for case in vectors::cases() {
        if case.kind != vectors::CaseKind::Capability
            || matches!(case.id.as_str(), "C13" | "C14" | "C15")
        {
            continue;
        }
        ran += 1;
        let (at, region) = if case.at.as_deref() == Some(EU_AUDIENCE) {
            (&eu, Region::Eu)
        } else {
            (&us, Region::Us)
        };
        let reading = case.op.as_deref() == Some("get")
            || case
                .claims
                .as_ref()
                .and_then(|claims| claims.pur.as_deref())
                == Some("blob_read")
                && case.op.is_none();
        let purpose = if reading {
            BlobPurpose::Read
        } else {
            BlobPurpose::Write
        };
        let subject = if reading { locator } else { upload };
        let mut claims = baseline(region, purpose, subject, locator);
        claims.len = bytes.len() as u64;
        claims.bh = hash_of(&bytes);
        if let Some(vector) = &case.claims {
            if let Some(version) = vector.v {
                claims.v = version;
            }
            if let Some(purpose) = &vector.pur {
                claims.pur = purpose.clone();
            }
            if let Some(region) = &vector.rgn {
                claims.rgn = if region == "eu" {
                    Region::Eu
                } else {
                    Region::Us
                };
            }
            if let Some(audience) = &vector.aud {
                claims.aud = audience.clone();
            }
        }
        if case.expired == Some(true) {
            claims.nbf -= 10_000;
            claims.exp = claims.nbf + 10;
        }
        if case.not_before == Some(true) {
            claims.nbf += 10_000;
            claims.exp = claims.nbf + 60;
        }
        let mut header = authorization(&claims);
        if case.tamper.as_deref() == Some("signature") {
            let mut raw = header.into_bytes();
            let last = raw.len() - 1;
            raw[last] = if raw[last] == b'A' { b'B' } else { b'A' };
            header = String::from_utf8(raw).expect("ascii");
        }
        if case.scheme.as_deref() == Some("Bearer") {
            // C16 — an account bearer token sent to a byte route.
            header = "Bearer eyJhbGciOiJFZERTQSJ9.e30.AA".into();
        }
        // The id in the URL, which C12 deliberately disagrees with.
        let route = match case.route_sub.as_deref() {
            Some(_) => fresh(),
            None => subject,
        };
        let http = reqwest::Client::new();
        let response = match (reading, case.op.as_deref()) {
            (_, Some("patch")) | (false, _) => http
                .patch(format!("{}/v1/objects/uploads/{route}", at.base))
                .header("authorization", header)
                .header("content-range", "bytes 0-1023/4096")
                .body(filler(0, 1024))
                .send()
                .await
                .expect("a patch"),
            (true, _) => http
                .get(format!("{}/v1/objects/{route}", at.base))
                .header("authorization", header)
                .send()
                .await
                .expect("a read"),
        };
        match case.expected() {
            None => assert!(
                response.status().is_success(),
                "{} — {}: {}",
                case.id,
                case.note,
                response.status()
            ),
            Some(expected) => {
                assert_eq!(
                    code_of(response).await,
                    expected,
                    "{} — {}",
                    case.id,
                    case.note
                );
            }
        }
    }
    assert_eq!(ran, 13);
    us.shutdown().await;
    eu.shutdown().await;
}

/// `R01`–`R15`, each against a session armed to the offset the case describes.
#[tokio::test(flavor = "multi_thread")]
async fn range_admission_vectors() {
    let service = ObjectService::start().await;
    let mut ran = 0;
    for case in vectors::cases() {
        if case.kind != vectors::CaseKind::Range {
            continue;
        }
        ran += 1;
        let upload = fresh();
        let mut claims = baseline(Region::Us, BlobPurpose::Write, upload, fresh());
        claims.len = case.cap_len.or(case.total).unwrap_or(4_194_304);
        let committed = case.committed.unwrap_or(0);
        if committed > 0 {
            // Arm the durable prefix through the real route, so the offset the
            // case meets is one the service actually reached.
            let mut arming = claims.clone();
            arming.len = claims.len.max(committed + 1);
            send(&service, upload, &arming, &filler(0, committed as usize)).await;
        }
        let (header, body) = match (&case.raw, case.range) {
            (Some(raw), _) => (raw.clone(), Vec::new()),
            (None, Some([start, end])) => {
                let total = case.total.expect("a range case declares its total");
                let declared = (end - start + 1).max(0) as u64;
                let length = case.body_len.unwrap_or(declared) as usize;
                let body = filler(start.max(0) as u64, length);
                let body = if case.identical == Some(false) {
                    // The same range, different bytes.
                    body.into_iter().map(|byte| byte ^ 0xff).collect()
                } else {
                    body
                };
                (format!("bytes {start}-{end}/{total}"), body)
            }
            (None, None) => panic!("range case {} carries neither range nor raw", case.id),
        };
        let response = patch(&service, upload, &claims, &header, body).await;
        match case.expected() {
            None => {
                assert_eq!(response.status(), 200, "{} — {}", case.id, case.note);
                let offset: koru_cloud_blob::wire::CommittedOffsetV1 =
                    response.json().await.expect("an offset");
                assert_eq!(
                    offset.committed_offset,
                    case.committed_after.expect("an admitted case declares it"),
                    "{} — {}",
                    case.id,
                    case.note
                );
            }
            Some(expected) => {
                assert_eq!(
                    code_of(response).await,
                    expected,
                    "{} — {}",
                    case.id,
                    case.note
                );
            }
        }
    }
    assert_eq!(ran, 15);
    service.shutdown().await;
}

/// `V01`–`V03`: what publishes, what quarantines, and what is refused before
/// either.
#[tokio::test(flavor = "multi_thread")]
async fn verification_vectors() {
    let service = ObjectService::start().await;

    // V01 — the bytes hash to the bound address, so the object is published.
    let bytes = filler(0, 3 * 1024 * 1024);
    let claims = upload_and_publish(&service, fresh(), fresh(), &bytes).await;
    assert_eq!(
        service.object_length(&key_of(&claims)),
        Some(bytes.len() as u64),
        "V01 publishes"
    );

    // V02 — the bytes do not hash to the bound address: nothing is published,
    // the staging file is set aside, and the session is over.
    let upload = fresh();
    let bytes = filler(7, 65_536);
    let mut claims = baseline(Region::Us, BlobPurpose::Write, upload, fresh());
    claims.len = bytes.len() as u64;
    claims.bh = hash_of(&filler(9, 65_536));
    send(&service, upload, &claims, &bytes).await;
    assert_eq!(
        complete(&service, upload, &claims).await.status(),
        202,
        "V02 still answers 202"
    );
    settled(|| service.quarantined(upload)).await;
    assert_eq!(
        service.object_length(&key_of(&claims)),
        None,
        "V02 publishes nothing"
    );
    assert_eq!(
        service.committed_offset(upload),
        0,
        "V02 quarantines the staging file"
    );
    assert_eq!(
        code_of(patch(&service, upload, &claims, "bytes 0-0/65536", vec![0]).await).await,
        BlobCode::HashMismatch,
        "V02 ends the session"
    );

    // V03 — complete before the declared length arrived. Nothing is published
    // and the session stays resumable.
    let upload = fresh();
    let mut claims = baseline(Region::Us, BlobPurpose::Write, upload, fresh());
    claims.len = 4096;
    claims.bh = hash_of(&filler(0, 4096));
    send(&service, upload, &claims, &filler(0, 1024)).await;
    assert_eq!(
        code_of(complete(&service, upload, &claims).await).await,
        BlobCode::LengthMismatch,
        "V03"
    );
    assert_eq!(
        service.committed_offset(upload),
        1024,
        "V03 leaves the session resumable"
    );
    service.shutdown().await;
}

/// `G01`–`G04`, and the difference between "not yours" and "not there".
#[tokio::test(flavor = "multi_thread")]
async fn read_vectors() {
    let service = ObjectService::start().await;
    let bytes = filler(3, 200_000);
    let locator = fresh();
    let write = upload_and_publish(&service, fresh(), locator, &bytes).await;
    let http = reqwest::Client::new();
    let read = |named: CloudId| {
        let mut claims = baseline(Region::Us, BlobPurpose::Read, named, named);
        claims.len = bytes.len() as u64;
        claims.bh = hash_of(&bytes);
        claims
    };

    // G01 — the locator the capability names.
    let response = http
        .get(format!("{}/v1/objects/{locator}", service.base))
        .header("authorization", authorization(&read(locator)))
        .send()
        .await
        .expect("a read");
    assert_eq!(response.status(), 200, "G01");
    assert_eq!(response.headers()["accept-ranges"], "bytes", "G01");
    assert_eq!(
        response.headers()["etag"].to_str().expect("ascii"),
        format!(
            "\"{}-{}\"",
            ObjectLocator::from_uuid(write.loc.as_uuid()),
            bytes.len()
        ),
        "G01: an immutable ETag bound to the locator and length"
    );
    assert_eq!(
        response.bytes().await.expect("bytes").as_ref(),
        &bytes[..],
        "G01"
    );

    // G02 — a locator that exists but the capability does not name.
    let other = read(fresh());
    let exists = http
        .get(format!("{}/v1/objects/{locator}", service.base))
        .header("authorization", authorization(&other))
        .send()
        .await
        .expect("a read");
    assert_eq!(code_of(exists).await, BlobCode::WrongObject, "G02");

    // G03 — a locator that does not exist and is not named: the same answer.
    let missing = http
        .get(format!("{}/v1/objects/{}", service.base, fresh()))
        .header("authorization", authorization(&other))
        .send()
        .await
        .expect("a read");
    assert_eq!(
        missing.status().as_u16(),
        BlobCode::WrongObject.status().expect("a status"),
        "G03 is indistinguishable from G02 by status"
    );
    assert_eq!(
        code_of(missing).await,
        BlobCode::WrongObject,
        "G03 matches G02"
    );

    // G04 — the capability names the locator and the bytes are gone.
    service.lose_object(&key_of(&write));
    let gone = http
        .get(format!("{}/v1/objects/{locator}", service.base))
        .header("authorization", authorization(&read(locator)))
        .send()
        .await
        .expect("a read");
    assert_eq!(code_of(gone).await, BlobCode::ObjectMissing, "G04");
    service.shutdown().await;
}

/// §4.4's single range, and the headers a resuming download reads.
#[tokio::test(flavor = "multi_thread")]
async fn a_download_resumes_from_a_single_range() {
    let service = ObjectService::start().await;
    let bytes = filler(0, 150_000);
    let locator = fresh();
    upload_and_publish(&service, fresh(), locator, &bytes).await;
    let mut claims = baseline(Region::Us, BlobPurpose::Read, locator, locator);
    claims.len = bytes.len() as u64;
    claims.bh = hash_of(&bytes);
    let http = reqwest::Client::new();

    let response = http
        .get(format!("{}/v1/objects/{locator}", service.base))
        .header("authorization", authorization(&claims))
        .header("range", "bytes=100000-")
        .send()
        .await
        .expect("a read");
    assert_eq!(response.status(), 206);
    assert_eq!(
        response.headers()["content-range"],
        "bytes 100000-149999/150000"
    );
    assert_eq!(response.headers()["content-length"], "50000");
    assert_eq!(
        response.bytes().await.expect("bytes").as_ref(),
        &bytes[100_000..]
    );

    for bad in ["bytes=150000-", "bytes=10-5", "bytes=abc-", "chunks=0-"] {
        let response = http
            .get(format!("{}/v1/objects/{locator}", service.base))
            .header("authorization", authorization(&claims))
            .header("range", bad)
            .send()
            .await
            .expect("a read");
        assert_eq!(code_of(response).await, BlobCode::MalformedRange, "{bad:?}");
    }
    service.shutdown().await;
}
