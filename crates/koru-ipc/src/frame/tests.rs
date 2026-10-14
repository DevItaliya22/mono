// SPDX-License-Identifier: MPL-2.0

//! The frame format is a wire format: a change here is a change two
//! independently-installed binaries have to agree about. These tests are what
//! makes such a change deliberate rather than incidental.

use super::*;

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Message {
    what: String,
    n: u32,
}

fn a_message() -> Message {
    Message {
        what: "hello".to_string(),
        n: 7,
    }
}

#[test]
fn a_frame_begins_with_the_magic_and_the_version_it_was_given() {
    // The version is a *parameter* — this module holds no constant of its own,
    // because two protocols are spoken over these frames and they version
    // independently.
    for version in [1u8, 2, 99] {
        let bytes = encode(version, &a_message()).unwrap();
        assert_eq!(&bytes[..4], b"KORU");
        assert_eq!(bytes[4], version);
        // The length is of the body alone, not of the whole frame — an
        // off-by-nine here would read the next frame's header as the tail of
        // this one.
        let declared = u32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]) as usize;
        assert_eq!(declared, bytes.len() - HEADER_LEN);
    }
}

#[test]
fn a_value_round_trips_and_carries_its_version_back() {
    let bytes = encode(3, &a_message()).unwrap();
    let mut cursor = std::io::Cursor::new(bytes);
    let (version, back): (u8, Message) = read_frame(&mut cursor).unwrap();
    assert_eq!(version, 3);
    assert_eq!(back, a_message());
}

#[test]
fn write_frame_and_read_frame_agree() {
    let mut buffer = Vec::new();
    write_frame(&mut buffer, 1, &a_message()).unwrap();
    let (version, back): (u8, Message) = read_frame(&mut std::io::Cursor::new(buffer)).unwrap();
    assert_eq!((version, back), (1, a_message()));
}

#[test]
fn something_that_is_not_a_koru_endpoint_says_so() {
    // The whole reason for four bytes of magic: this is the message somebody
    // gets when they point a Koru client at the wrong pipe, and "expected value
    // at line 1 column 1" would send them looking in the wrong place.
    let mut bogus = [0u8; HEADER_LEN];
    bogus[..4].copy_from_slice(b"HTTP");
    let complaint = parse_header(&bogus).unwrap_err().to_string();
    assert!(complaint.contains("not a Koru endpoint"), "{complaint}");
}

#[test]
fn an_oversized_length_is_refused_before_anything_is_allocated() {
    // Without this a peer sends a 4 GiB length and nothing else, and the reader
    // allocates 4 GiB waiting for a body that never arrives.
    let mut header = [0u8; HEADER_LEN];
    header[..4].copy_from_slice(&MAGIC);
    header[4] = 1;
    header[5..].copy_from_slice(&u32::MAX.to_le_bytes());
    let complaint = parse_header(&header).unwrap_err().to_string();
    assert!(complaint.contains("over the"), "{complaint}");
}

#[test]
fn an_unknown_version_still_parses_its_header() {
    // Load-bearing: a reader has to be able to see the version and the length
    // of a frame whose body it cannot read, or an unfamiliar peer costs it
    // framing altogether rather than one message.
    let mut header = [0u8; HEADER_LEN];
    header[..4].copy_from_slice(&MAGIC);
    header[4] = 99;
    header[5..].copy_from_slice(&12u32.to_le_bytes());
    let parsed = parse_header(&header).unwrap();
    assert_eq!(parsed.version, 99);
    assert_eq!(parsed.body_len, 12);
}

#[test]
fn a_body_can_be_read_without_being_given_a_shape() {
    // What a long-lived protocol needs: the version has to be *accepted* before
    // the body is deserialized, so a stream whose semantics are still being
    // negotiated is never parsed as though they were settled.
    let bytes = encode(99, &a_message()).unwrap();
    let (version, body) = read_raw(&mut std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(version, 99);
    assert!(!body.is_empty());
    // ...and the caller may still decode it once it has decided to.
    assert_eq!(
        serde_json::from_slice::<Message>(&body).unwrap(),
        a_message()
    );
}

#[test]
fn a_truncated_frame_is_an_error_and_not_a_default_value() {
    // `read_exact` rather than `read`: a short read that deserialized to a
    // default would be a peer with an empty name and version 0, which the
    // receiver would then have to decide about.
    let bytes = encode(1, &a_message()).unwrap();
    let mut cursor = std::io::Cursor::new(bytes[..HEADER_LEN + 3].to_vec());
    let outcome: std::io::Result<(u8, Message)> = read_frame(&mut cursor);
    assert!(outcome.is_err());
}

#[test]
fn a_body_over_the_cap_is_refused_at_encode_rather_than_on_the_wire() {
    // Cheaper to fail where the value is, and it keeps a sender from writing a
    // header its own reader would reject.
    let huge = Message {
        what: "x".repeat(MAX_BODY + 1),
        n: 0,
    };
    let complaint = encode(1, &huge).unwrap_err().to_string();
    assert!(complaint.contains("over the"), "{complaint}");
}
