// SPDX-License-Identifier: MPL-2.0

//! **What a refused byte request is reported as, over a real socket.**
//!
//! The upload's first byte request is the resume read (§4.1), and it is a
//! `HEAD`. HTTP forbids a body on the answer to one, so the `BlobErrorV1` the
//! service writes never reaches the client — which means the code the whole
//! transfer engine branches on was, for the first request of every upload,
//! being guessed from the status alone. `401` is two codes, the guess was
//! `capability_expired`, and a person read that as a clock problem and spent
//! hours on it while the deployment's real disagreement was an issuer.
//!
//! An in-memory plane would prove nothing here: the stripping is the HTTP
//! server's, so this runs the object service on a real socket — the same
//! framework the worker answers with, and the contract's
//! `BLOB_CODE_HEADER` — and the real `HttpObjectPlane` against it. That the
//! real worker sets the header is asserted by the hosted service (not in this
//! repository), which also runs this file against itself.

mod common;

use koru_blob_sync::{BlobObjectPlane, ByteOrigins, HttpObjectPlane, TransferError};
use koru_cloud_blob::BlobCode;

use common::{ObjectService, claims, fresh, sign};

fn local_origins(service_base: &str) -> ByteOrigins {
    ByteOrigins::none()
        .with_development_origins(service_base)
        .expect("a local origin")
}

/// A well-formed capability signed with a key the service trusts, refused for
/// exactly one reason: it names another issuer. This is the shape a local
/// stack produces when the API signs as its regional issuer and the service was
/// configured with the API's legacy one.
fn wrong_issuer(upload: koru_device::cloud::CloudId) -> String {
    let mut wrong = claims(
        koru_account_session::cloud::blob::BlobPurpose::Write,
        (fresh(), fresh()),
        upload,
        fresh(),
        4,
        &format!("b3:{}", blake3::hash(b"four").to_hex()),
    );
    wrong.iss = "http://localhost:8080".into();
    sign(&wrong)
}

/// **The regression.** A capability the service cannot place is reported as
/// what it is, through the one route that cannot answer with a body.
#[tokio::test]
async fn a_bodiless_refusal_is_reported_as_what_the_service_said() {
    let service = ObjectService::start().await;
    let upload = fresh();
    let url = local_origins(&service.base)
        .admit(&format!("{}/v1/objects/uploads/{upload}", service.base))
        .expect("an admitted byte url");

    let objects = HttpObjectPlane::new().expect("an http object plane");
    let refused = objects
        .committed_offset(url, wrong_issuer(upload))
        .await
        .expect_err("the service refuses a capability naming another issuer");

    assert_eq!(
        refused,
        TransferError::Refused(BlobCode::CapabilityInvalid),
        "a refusal the client could not read the body of must not be reported \
         as an expiry: that is what sent a person to check two clocks",
    );
    service.shutdown().await;
}

/// And a capability that really is outside its window is still an expiry, so
/// the renewal path a real expiry needs has not been taken away.
#[tokio::test]
async fn an_expired_capability_is_still_reported_as_an_expiry() {
    let service = ObjectService::start().await;
    let upload = fresh();
    let url = local_origins(&service.base)
        .admit(&format!("{}/v1/objects/uploads/{upload}", service.base))
        .expect("an admitted byte url");

    let mut expired = claims(
        koru_account_session::cloud::blob::BlobPurpose::Write,
        (fresh(), fresh()),
        upload,
        fresh(),
        4,
        &format!("b3:{}", blake3::hash(b"four").to_hex()),
    );
    expired.nbf -= 600;
    expired.exp = expired.nbf + 1;

    let objects = HttpObjectPlane::new().expect("an http object plane");
    let refused = objects
        .committed_offset(url, sign(&expired))
        .await
        .expect_err("a capability outside its window is refused");

    assert_eq!(refused, TransferError::Refused(BlobCode::CapabilityExpired));
    service.shutdown().await;
}
