// SPDX-License-Identifier: MPL-2.0

use super::*;
fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Option<Rect> {
    Some(Rect {
        left,
        top,
        right,
        bottom,
    })
}
#[test]
fn intersection_is_distinct_from_a_clickable_center_and_edge_contact() {
    let body = rect(100, 100, 300, 300);
    assert_eq!(
        in_document_viewport(rect(120, 80, 200, 110), body),
        Ok(true)
    );
    assert_eq!(
        center_in_document_viewport(rect(120, 80, 200, 110), body),
        Ok(false)
    );
    assert_eq!(
        in_document_viewport(rect(120, 80, 200, 100), body),
        Ok(false)
    );
    assert_eq!(
        center_in_document_viewport(rect(120, 100, 200, 130), body),
        Ok(true)
    );
}
#[test]
fn unlaid_or_empty_subject_is_outside_but_invalid_body_is_an_error() {
    let body = rect(100, 100, 300, 300);
    for subject in [None, rect(120, 120, 120, 130)] {
        assert_eq!(in_document_viewport(subject, body), Ok(false));
        assert_eq!(center_in_document_viewport(subject, body), Ok(false));
    }
    for body in [None, rect(100, 100, 100, 300), rect(300, 100, 100, 300)] {
        assert_eq!(
            in_document_viewport(None, body),
            Err("document-viewport-unavailable")
        );
        assert_eq!(
            center_in_document_viewport(rect(120, 120, 200, 130), body),
            Err("document-viewport-unavailable")
        );
    }
}
#[test]
fn center_uses_native_euclidean_rounding_and_wide_arithmetic() {
    assert_eq!(
        center_in_document_viewport(rect(-3, -3, 0, 0), rect(-2, -2, -1, -1)),
        Ok(true)
    );
    assert_eq!(
        center_in_document_viewport(
            rect(i32::MIN, i32::MIN, i32::MAX, i32::MAX),
            rect(-1, -1, 1, 1)
        ),
        Ok(true)
    );
}
