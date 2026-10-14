// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn a_declared_length_is_checked_against_what_is_present() {
    // 0xffffffff bytes declared, four bytes present. The failure must be a
    // parse error and not an allocation.
    let mut reader = Reader::new(&[0xff, 0xff, 0xff, 0xff]);
    assert_eq!(reader.var().unwrap_err(), CryptoError::Malformed);
}

#[test]
fn trailing_bytes_are_malformed() {
    let mut reader = Reader::new(&[0, 1, 2, 3]);
    reader.u16().unwrap();
    assert_eq!(reader.finish().unwrap_err(), CryptoError::Malformed);
}

#[test]
fn a_round_trip_is_byte_identical() {
    let id = uuid::Uuid::from_bytes([7u8; 16]);
    let mut writer = Writer::default();
    writer.u16(1).u32(9).uuid(id).var(b"abc").unwrap();
    let bytes = writer.finish();

    let mut reader = Reader::new(&bytes);
    assert_eq!(reader.u16().unwrap(), 1);
    assert_eq!(reader.u32().unwrap(), 9);
    assert_eq!(reader.uuid().unwrap(), id);
    assert_eq!(reader.var().unwrap(), b"abc");
    reader.finish().unwrap();
}

#[test]
fn consumed_is_the_exact_bytes_the_parser_read() {
    let bytes = [1u8, 2, 3, 4, 5, 6];
    let mut reader = Reader::new(&bytes);
    reader.u32().unwrap();
    assert_eq!(reader.consumed(), &bytes[..4]);
}
