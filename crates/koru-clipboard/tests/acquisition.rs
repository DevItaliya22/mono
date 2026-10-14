// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;

use koru_clipboard::{
    Backend, Clipboard, Error, ErrorCode, ImageLimits, ImagePayload, RgbaBitmap, TextPayload,
};

#[derive(Default)]
struct Fixture {
    text: Option<Result<Option<String>, Error>>,
    bitmap: Option<Result<Option<RgbaBitmap>, Error>>,
    paths: Option<Result<Vec<PathBuf>, Error>>,
}

impl Backend for Fixture {
    fn text(&self) -> Result<Option<String>, Error> {
        self.text.clone().unwrap_or_else(|| Ok(None))
    }

    fn bitmap(&self) -> Result<Option<RgbaBitmap>, Error> {
        self.bitmap.clone().unwrap_or_else(|| Ok(None))
    }

    fn image_paths(&self) -> Result<Vec<PathBuf>, Error> {
        self.paths.clone().unwrap_or_else(|| Ok(Vec::new()))
    }
}

fn unavailable() -> Error {
    Error::new(ErrorCode::Unavailable)
}

#[test]
fn text_is_typed_without_a_desktop_clipboard() {
    let clipboard = Clipboard::new(Fixture {
        text: Some(Ok(Some("hello".into()))),
        ..Fixture::default()
    });
    assert_eq!(clipboard.text(), TextPayload::Text("hello".into()));
}

#[test]
fn a_bitmap_is_bounded_and_normalized_to_png() {
    let clipboard = Clipboard::new(Fixture {
        bitmap: Some(Ok(Some(RgbaBitmap {
            width: 2,
            height: 1,
            bytes: vec![255, 0, 0, 255, 0, 255, 0, 255],
        }))),
        ..Fixture::default()
    });
    let ImagePayload::Bitmap(image) = clipboard.images(ImageLimits::default()).unwrap() else {
        panic!("expected bitmap");
    };
    assert_eq!((image.width, image.height), (2, 1));
    assert_eq!(&image.bytes[..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn an_image_file_list_wins_and_keeps_only_images_in_order() {
    let clipboard = Clipboard::new(Fixture {
        paths: Some(Ok(vec![
            PathBuf::from("first.png"),
            PathBuf::from("notes.txt"),
            PathBuf::from("SECOND.JPEG"),
        ])),
        bitmap: Some(Err(unavailable())),
        ..Fixture::default()
    });
    assert_eq!(
        clipboard.images(ImageLimits::default()).unwrap(),
        ImagePayload::Files(vec![
            PathBuf::from("first.png"),
            PathBuf::from("SECOND.JPEG")
        ])
    );
}

#[test]
fn an_empty_clipboard_is_not_an_error() {
    let clipboard = Clipboard::new(Fixture::default());
    assert_eq!(clipboard.text(), TextPayload::NonText);
    assert_eq!(
        clipboard.images(ImageLimits::default()).unwrap(),
        ImagePayload::Empty
    );
}

#[test]
fn an_unavailable_clipboard_stays_distinct_from_non_text() {
    let clipboard = Clipboard::new(Fixture {
        text: Some(Err(unavailable())),
        bitmap: Some(Err(unavailable())),
        paths: Some(Err(unavailable())),
    });
    assert_eq!(clipboard.text(), TextPayload::Unavailable);
    assert_eq!(
        clipboard.images(ImageLimits::default()).unwrap_err().code,
        ErrorCode::Unavailable
    );
}

#[test]
fn malformed_rgba_is_recoverable() {
    let clipboard = Clipboard::new(Fixture {
        bitmap: Some(Ok(Some(RgbaBitmap {
            width: 2,
            height: 2,
            bytes: vec![0; 15],
        }))),
        ..Fixture::default()
    });
    assert_eq!(
        clipboard.images(ImageLimits::default()).unwrap_err().code,
        ErrorCode::CorruptImage
    );
}

#[test]
fn dimensions_are_rejected_before_an_oversized_allocation() {
    let clipboard = Clipboard::new(Fixture {
        bitmap: Some(Ok(Some(RgbaBitmap {
            width: 20_000,
            height: 20_000,
            bytes: Vec::new(),
        }))),
        ..Fixture::default()
    });
    assert_eq!(
        clipboard.images(ImageLimits::default()).unwrap_err().code,
        ErrorCode::OversizedImage
    );
}

#[test]
fn image_acquisition_runs_on_the_named_worker_with_an_injected_backend() {
    let fixture = Fixture {
        bitmap: Some(Ok(Some(RgbaBitmap {
            width: 1,
            height: 1,
            bytes: vec![1, 2, 3, 255],
        }))),
        ..Fixture::default()
    };
    let (send, receive) = std::sync::mpsc::channel();
    let worker =
        koru_clipboard::spawn_image_read_with(fixture, ImageLimits::default(), move |result| {
            send.send((std::thread::current().name().map(str::to_owned), result))
                .unwrap();
        })
        .unwrap();

    let (name, result) = receive.recv().unwrap();
    worker.join().unwrap();
    assert_eq!(name.as_deref(), Some("koru-clipboard image read"));
    assert!(matches!(result, Ok(ImagePayload::Bitmap(_))));
}
