// SPDX-License-Identifier: MPL-2.0

use super::*;
use reqwest::header::HeaderValue;

#[test]
fn a_head_objects_content_length_is_read_from_the_header() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static("473983562"));
    assert_eq!(content_length_header(&headers).unwrap(), Some(473_983_562));
}

#[test]
fn a_missing_content_length_remains_distinct_from_zero() {
    assert_eq!(content_length_header(&HeaderMap::new()).unwrap(), None);
}

#[test]
fn a_malformed_content_length_is_not_treated_as_unknown() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static("not-a-size"));
    let error = content_length_header(&headers).unwrap_err();
    assert!(error.contains("not an unsigned byte count"), "{error}");
}
