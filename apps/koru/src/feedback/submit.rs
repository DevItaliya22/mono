// SPDX-License-Identifier: MPL-2.0

//! Build and POST a feedback payload. Network lives on a worker thread.

use std::sync::mpsc::{self, Receiver};
use std::thread;

use super::{Attachment, Frequency, Importance, Kind};

/// Canonical host is `www`. The apex (`koru.do`) answers 308 Permanent
/// Redirect, and a multipart POST cannot be replayed across that hop — Send
/// then reports the 308 instead of reaching the route.
pub const DEFAULT_URL: &str = "https://www.koru.do/api/feedback";
pub const MAX_SCREENSHOTS: usize = 3;
pub const MAX_EDGE: u32 = 1920;
const JPEG_QUALITY: u8 = 80;
const MAX_BODY: usize = 3_500_000;

#[derive(Clone, Debug)]
pub struct Payload {
    pub kind: Kind,
    pub message: String,
    pub frequency: Option<Frequency>,
    pub expected: Option<String>,
    pub importance: Option<Importance>,
    pub contact: Option<String>,
    pub screenshots: Vec<Attachment>,
    pub logs_zip: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitError {
    Empty,
    InvalidEmail,
    TooLarge,
    Network(String),
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("Add a few words so we can help"),
            Self::InvalidEmail => f.write_str("Enter an email we can reply to"),
            Self::TooLarge => {
                f.write_str("The report is too large to send. Remove a screenshot or the logs.")
            }
            Self::Network(message) => f.write_str(message),
        }
    }
}

pub trait Transport: Send + 'static {
    fn post(&self, url: &str, payload: &Payload) -> Result<(), SubmitError>;
}

pub fn endpoint() -> String {
    let raw = std::env::var(koru_env::name::KORU_FEEDBACK_URL)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_URL.to_string());
    canonical_feedback_url(&raw)
}

/// Apex `koru.do` 308s to `www.koru.do`. Multipart cannot follow that, so
/// rewrite the known hop before the POST leaves the machine.
fn canonical_feedback_url(url: &str) -> String {
    const APEX: &str = "://koru.do/";
    const WWW: &str = "://www.koru.do/";
    if let Some(scheme_end) = url.find(APEX) {
        let mut out = String::with_capacity(url.len() + 4);
        out.push_str(&url[..scheme_end]);
        out.push_str(WWW);
        out.push_str(&url[scheme_end + APEX.len()..]);
        return out;
    }
    url.to_string()
}

pub fn validate(payload: &Payload) -> Result<(), SubmitError> {
    if payload.message.trim().len() < 3 {
        return Err(SubmitError::Empty);
    }
    if let Some(email) = payload.contact.as_deref()
        && !is_email(email)
    {
        return Err(SubmitError::InvalidEmail);
    }
    if estimate_size(payload) > MAX_BODY {
        return Err(SubmitError::TooLarge);
    }
    Ok(())
}

pub fn is_email(value: &str) -> bool {
    let value = value.trim();
    let Some((user, host)) = value.split_once('@') else {
        return false;
    };
    !user.is_empty() && host.contains('.') && !host.starts_with('.') && !host.ends_with('.')
}

pub fn estimate_size(payload: &Payload) -> usize {
    payload.message.len()
        + payload.expected.as_ref().map_or(0, String::len)
        + payload.contact.as_ref().map_or(0, String::len)
        + payload
            .screenshots
            .iter()
            .map(|shot| shot.bytes.len())
            .sum::<usize>()
        + payload.logs_zip.as_ref().map_or(0, Vec::len)
}

/// Decode an image, shrink it to [`MAX_EDGE`], and re-encode as JPEG.
pub fn compress_image(bytes: &[u8], filename: &str) -> Result<Attachment, String> {
    let decoded = image::load_from_memory(bytes).map_err(|error| error.to_string())?;
    let decoded = decoded.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Triangle);
    let mut out = Vec::new();
    let rgb = decoded.to_rgb8();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
    encoder
        .encode(
            &rgb,
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|error| error.to_string())?;
    let stem = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("screenshot")
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or("screenshot");
    Ok(Attachment {
        filename: format!("{stem}.jpg"),
        mime: "image/jpeg".into(),
        bytes: out,
    })
}

/// Spawn the send, letting the worker finish assembling the payload.
///
/// **`prepare` runs on the worker, never on the caller.** Zipping the logs
/// walks the log directory, reads up to two megabytes and deflates them.
/// Doing that before the thread starts would stall the very frame that is
/// trying to draw the thank-you page.
pub fn spawn_prepared(
    url: String,
    payload: Payload,
    transport: impl Transport,
    prepare: impl FnOnce(&mut Payload) + Send + 'static,
) -> Receiver<Result<(), SubmitError>> {
    let (tx, rx) = mpsc::channel();
    let _ = thread::Builder::new()
        .name("koru-feedback submit".into())
        .spawn(move || {
            let mut payload = payload;
            prepare(&mut payload);
            let result = match validate(&payload) {
                Ok(()) => transport.post(&url, &payload),
                Err(error) => Err(error),
            };
            let _ = tx.send(result);
        });
    rx
}

pub struct HttpTransport;

impl Transport for HttpTransport {
    fn post(&self, url: &str, payload: &Payload) -> Result<(), SubmitError> {
        post_http(url, payload)
    }
}

fn post_http(url: &str, payload: &Payload) -> Result<(), SubmitError> {
    let mut form = reqwest::blocking::multipart::Form::new()
        .text("kind", payload.kind.wire())
        .text("message", payload.message.clone());
    if let Some(frequency) = payload.frequency {
        form = form.text("frequency", frequency.wire());
    }
    if let Some(expected) = payload.expected.clone() {
        form = form.text("expected", expected);
    }
    if let Some(importance) = payload.importance {
        form = form.text("importance", importance.wire());
    }
    if let Some(contact) = payload.contact.clone() {
        form = form.text("contact", contact);
    }
    // Empty honeypot. The marketing route treats a filled one as a bot.
    form = form.text("website", "");
    for (index, shot) in payload.screenshots.iter().enumerate() {
        let part = reqwest::blocking::multipart::Part::bytes(shot.bytes.clone())
            .file_name(shot.filename.clone())
            .mime_str(&shot.mime)
            .map_err(|error| SubmitError::Network(error.to_string()))?;
        form = form.part(format!("screenshot{index}"), part);
    }
    if let Some(zip) = payload.logs_zip.clone() {
        let part = reqwest::blocking::multipart::Part::bytes(zip)
            .file_name("koru-logs.zip")
            .mime_str("application/zip")
            .map_err(|error| SubmitError::Network(error.to_string()))?;
        form = form.part("logs", part);
    }
    let response = reqwest::blocking::Client::new()
        .post(url)
        .multipart(form)
        .send()
        .map_err(|error| SubmitError::Network(error.to_string()))?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let message = match status.as_u16() {
        429 => "Too many reports just now. Try again in a few minutes.".into(),
        _ => format!("Could not send ({status})"),
    };
    Err(SubmitError::Network(message))
}

/// Used by tests that need a JPEG without depending on a file.
#[cfg(test)]
pub fn tiny_png() -> Vec<u8> {
    let mut bytes = Vec::new();
    use image::ImageEncoder;
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&[255, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
        .expect("png");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feedback::Kind;

    #[test]
    fn an_empty_message_is_refused() {
        let payload = Payload {
            kind: Kind::Bug,
            message: "  hi".into(),
            frequency: None,
            expected: None,
            importance: None,
            contact: None,
            screenshots: Vec::new(),
            logs_zip: None,
        };
        assert_eq!(validate(&payload), Err(SubmitError::Empty));
    }

    #[test]
    fn a_contact_email_must_look_like_one() {
        let mut payload = valid();
        payload.contact = Some("not-an-email".into());
        assert_eq!(validate(&payload), Err(SubmitError::InvalidEmail));
        payload.contact = Some("person@koru.do".into());
        assert_eq!(validate(&payload), Ok(()));
    }

    #[test]
    fn a_png_becomes_a_jpeg_named_from_the_original() {
        let shot = compress_image(&tiny_png(), "Capture.PNG").expect("jpeg");
        assert_eq!(shot.filename, "Capture.jpg");
        assert_eq!(shot.mime, "image/jpeg");
        assert!(!shot.bytes.is_empty());
    }

    #[test]
    fn the_default_url_is_the_www_host_that_does_not_308() {
        assert_eq!(DEFAULT_URL, "https://www.koru.do/api/feedback");
        assert_eq!(
            canonical_feedback_url("https://koru.do/api/feedback"),
            "https://www.koru.do/api/feedback"
        );
        assert_eq!(
            canonical_feedback_url("http://localhost:3000/api/feedback"),
            "http://localhost:3000/api/feedback"
        );
    }

    #[test]
    fn the_payload_never_carries_a_diagnostics_object() {
        let payload = valid();
        // The type has no diagnostics field. This test exists so a later
        // contributor cannot add one without noticing the product decision.
        let encoded = format!("{payload:?}");
        assert!(!encoded.contains("diagnostic"));
    }

    fn valid() -> Payload {
        Payload {
            kind: Kind::Bug,
            message: "The tab would not close".into(),
            frequency: Some(Frequency::EveryTime),
            expected: None,
            importance: None,
            contact: None,
            screenshots: Vec::new(),
            logs_zip: None,
        }
    }
}
