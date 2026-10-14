// SPDX-License-Identifier: MPL-2.0

//! Geometric document visibility, separate from the provider's raw IsOffscreen fact.
//! Bounds use UIA physical screen pixels. Body geometry must be valid; an unlaid
//! paragraph with no rectangle is discoverable but is outside this viewport.
use crate::inspect::Rect;

fn body(bounds: Option<Rect>) -> Result<Rect, &'static str> {
    bounds
        .filter(|rect| !rect.is_empty())
        .ok_or("document-viewport-unavailable")
}

pub fn in_document_viewport(
    subject: Option<Rect>,
    viewport: Option<Rect>,
) -> Result<bool, &'static str> {
    let viewport = body(viewport)?;
    Ok(subject.filter(|rect| !rect.is_empty()).is_some_and(|rect| {
        rect.left < viewport.right
            && rect.right > viewport.left
            && rect.top < viewport.bottom
            && rect.bottom > viewport.top
    }))
}

/// Match the shared native mechanism's integer center. An intersection alone
/// does not make that point safe: a near-viewport layout band can have bounds
/// while its center is outside the actual document surface.
pub fn center_in_document_viewport(
    subject: Option<Rect>,
    viewport: Option<Rect>,
) -> Result<bool, &'static str> {
    let viewport = body(viewport)?;
    Ok(subject.filter(|rect| !rect.is_empty()).is_some_and(|rect| {
        let x = (i64::from(rect.left) + i64::from(rect.right)).div_euclid(2);
        let y = (i64::from(rect.top) + i64::from(rect.bottom)).div_euclid(2);
        x >= i64::from(viewport.left)
            && x < i64::from(viewport.right)
            && y >= i64::from(viewport.top)
            && y < i64::from(viewport.bottom)
    }))
}

#[cfg(test)]
mod tests;
