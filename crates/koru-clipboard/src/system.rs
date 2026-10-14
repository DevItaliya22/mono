// SPDX-License-Identifier: MPL-2.0

use std::borrow::Cow;
use std::path::PathBuf;

use crate::{Backend, Error, ErrorCode, RawFormat, RawPayload, RgbaBitmap};

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemBackend;

impl Backend for SystemBackend {
    fn text(&self) -> Result<Option<String>, Error> {
        let mut clipboard = arboard::Clipboard::new().map_err(|_| unavailable())?;
        match clipboard.get_text() {
            Ok(text) => Ok(Some(text)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(_) => Err(unavailable()),
        }
    }

    fn bitmap(&self) -> Result<Option<RgbaBitmap>, Error> {
        let mut clipboard = arboard::Clipboard::new().map_err(|_| unavailable())?;
        match clipboard.get_image() {
            Ok(image) => Ok(Some(RgbaBitmap {
                width: image.width,
                height: image.height,
                bytes: image.bytes.into_owned(),
            })),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(_) => Err(unavailable()),
        }
    }

    fn image_paths(&self) -> Result<Vec<PathBuf>, Error> {
        platform::image_paths()
    }

    fn raw(&self, format: RawFormat) -> Result<Option<RawPayload>, Error> {
        platform::raw(format)
    }

    fn format_names(&self) -> Result<Vec<String>, Error> {
        platform::format_names()
    }

    fn set_text(&self, text: &str) -> Result<(), Error> {
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_text(text.to_owned()))
            .map_err(|_| unavailable())
    }

    fn set_text_and_html(&self, plain: &str, html: &str) -> Result<(), Error> {
        platform::set_text_and_html(plain, html)
    }

    fn set_bitmap(&self, bitmap: &RgbaBitmap) -> Result<(), Error> {
        let image = arboard::ImageData {
            width: bitmap.width,
            height: bitmap.height,
            bytes: Cow::Borrowed(&bitmap.bytes),
        };
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_image(image))
            .map_err(|_| unavailable())
    }
}

fn unavailable() -> Error {
    Error::new(ErrorCode::Unavailable)
}

#[cfg(target_os = "windows")]
mod platform {
    use std::path::PathBuf;

    use clipboard_win::raw;

    use crate::{Backend as _, Error, ErrorCode, RawFormat, RawPayload};

    const MARKDOWN_FORMATS: &[&str] = &["text/markdown", "text/x-markdown", "application/markdown"];

    pub fn image_paths() -> Result<Vec<PathBuf>, Error> {
        let mut bytes = Vec::new();
        if raw::open().is_err() {
            return Err(Error::new(ErrorCode::Unavailable));
        }
        let read = raw::get_vec(clipboard_win::formats::CF_HDROP, &mut bytes).is_ok();
        let _ = raw::close();
        Ok(if read {
            parse_hdrop(&bytes)
        } else {
            Vec::new()
        })
    }

    pub fn raw(format: RawFormat) -> Result<Option<RawPayload>, Error> {
        if format == RawFormat::Markdown {
            return markdown();
        }
        let name = match format {
            RawFormat::Html => "HTML Format",
            RawFormat::Rtf => "Rich Text Format",
            RawFormat::Markdown => unreachable!(),
        };
        let Some(format) = raw::register_format(name) else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        if raw::open().is_err() {
            return Err(Error::new(ErrorCode::Unavailable));
        }
        let read = raw::get_vec(format.get(), &mut bytes).is_ok();
        let _ = raw::close();
        Ok((read && !bytes.is_empty()).then_some(RawPayload {
            format: name.to_owned(),
            bytes,
        }))
    }

    fn markdown() -> Result<Option<RawPayload>, Error> {
        if raw::open().is_err() {
            return Err(Error::new(ErrorCode::Unavailable));
        }
        let mut found = None;
        for format in raw::EnumFormats::new() {
            let Some(name) = raw::format_name_big(format) else {
                continue;
            };
            if !MARKDOWN_FORMATS
                .iter()
                .any(|candidate| name.eq_ignore_ascii_case(candidate))
            {
                continue;
            }
            let mut bytes = Vec::new();
            if raw::get_vec(format, &mut bytes).is_ok() {
                while bytes.last() == Some(&0) {
                    bytes.pop();
                }
                if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
                    bytes.drain(..3);
                }
                if !bytes.is_empty() {
                    found = Some(RawPayload {
                        format: name,
                        bytes,
                    });
                    break;
                }
            }
        }
        let _ = raw::close();
        Ok(found)
    }

    pub fn format_names() -> Result<Vec<String>, Error> {
        if raw::open().is_err() {
            return Err(Error::new(ErrorCode::Unavailable));
        }
        let names = raw::EnumFormats::new()
            .filter_map(raw::format_name_big)
            .take(64)
            .map(|name| name.replace(['\r', '\n'], " "))
            .collect();
        let _ = raw::close();
        Ok(names)
    }

    pub fn set_text_and_html(plain: &str, html: &str) -> Result<(), Error> {
        use clipboard_win::Clipboard;

        let Some(format) = raw::register_format("HTML Format") else {
            return super::SystemBackend.set_text(plain);
        };
        let Ok(_clipboard) = Clipboard::new_attempts(10) else {
            return super::SystemBackend.set_text(plain);
        };
        raw::set_string(plain).map_err(|_| Error::new(ErrorCode::Unavailable))?;
        raw::set_without_clear(format.get(), html.as_bytes())
            .map_err(|_| Error::new(ErrorCode::Unavailable))
    }

    fn parse_hdrop(bytes: &[u8]) -> Vec<PathBuf> {
        const HEADER: usize = 20;
        if bytes.len() < HEADER {
            return Vec::new();
        }
        let offset = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let wide = i32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]) != 0;
        let Some(data) = bytes.get(offset..) else {
            return Vec::new();
        };
        if wide {
            let units: Vec<_> = data
                .chunks_exact(2)
                .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
                .collect();
            units
                .split(|unit| *unit == 0)
                .take_while(|part| !part.is_empty())
                .filter_map(|part| String::from_utf16(part).ok())
                .map(PathBuf::from)
                .collect()
        } else {
            data.split(|byte| *byte == 0)
                .take_while(|part| !part.is_empty())
                .filter_map(|part| std::str::from_utf8(part).ok())
                .map(PathBuf::from)
                .collect()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn wide_hdrop_paths_keep_order() {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&20_u32.to_le_bytes());
            bytes.extend_from_slice(&[0; 12]);
            bytes.extend_from_slice(&1_i32.to_le_bytes());
            for path in [r"C:\a.png", r"C:\b.jpg"] {
                for unit in path.encode_utf16().chain([0]) {
                    bytes.extend_from_slice(&unit.to_le_bytes());
                }
            }
            bytes.extend_from_slice(&0_u16.to_le_bytes());
            assert_eq!(
                parse_hdrop(&bytes),
                vec![PathBuf::from(r"C:\a.png"), PathBuf::from(r"C:\b.jpg")]
            );
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::PathBuf;

    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::NSString;

    use crate::{Backend as _, Error, RawFormat, RawPayload};

    pub fn image_paths() -> Result<Vec<PathBuf>, Error> {
        let board = NSPasteboard::generalPasteboard();
        let Some(items) = board.pasteboardItems() else {
            return Ok(Vec::new());
        };
        let ty = NSString::from_str("public.file-url");
        Ok(items
            .iter()
            .filter_map(|item| item.stringForType(&ty))
            .filter_map(|value| path_from_file_url(&value.to_string()))
            .collect())
    }

    pub fn raw(format: RawFormat) -> Result<Option<RawPayload>, Error> {
        let (name, uti) = match format {
            RawFormat::Html => ("public.html", "public.html"),
            RawFormat::Rtf => ("public.rtf", "public.rtf"),
            RawFormat::Markdown => return Ok(None),
        };
        let board = NSPasteboard::generalPasteboard();
        Ok(board
            .stringForType(&NSString::from_str(uti))
            .map(|value| value.to_string())
            .filter(|value| !value.is_empty())
            .map(|value| RawPayload {
                format: name.to_owned(),
                bytes: value.into_bytes(),
            }))
    }

    pub fn format_names() -> Result<Vec<String>, Error> {
        Ok(Vec::new())
    }

    pub fn set_text_and_html(plain: &str, _html: &str) -> Result<(), Error> {
        super::SystemBackend.set_text(plain)
    }

    fn path_from_file_url(url: &str) -> Option<PathBuf> {
        let rest = url.strip_prefix("file://")?;
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        rest.starts_with('/')
            .then(|| PathBuf::from(percent_decode(rest)))
    }

    fn percent_decode(value: &str) -> String {
        let bytes = value.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%'
                && index + 2 < bytes.len()
                && let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3])
                && let Ok(byte) = u8::from_str_radix(hex, 16)
            {
                out.push(byte);
                index += 3;
                continue;
            }
            out.push(bytes[index]);
            index += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn file_urls_decode_paths_without_accepting_web_urls() {
            assert_eq!(
                path_from_file_url("file:///Users/me/caf%C3%A9%20photo.png"),
                Some(PathBuf::from("/Users/me/café photo.png"))
            );
            assert_eq!(path_from_file_url("https://example.com/a.png"), None);
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod platform {
    use std::path::PathBuf;

    use crate::{Backend as _, Error, RawFormat, RawPayload};

    pub fn image_paths() -> Result<Vec<PathBuf>, Error> {
        Ok(Vec::new())
    }

    pub fn raw(_format: RawFormat) -> Result<Option<RawPayload>, Error> {
        Ok(None)
    }

    pub fn format_names() -> Result<Vec<String>, Error> {
        Ok(Vec::new())
    }

    pub fn set_text_and_html(plain: &str, _html: &str) -> Result<(), Error> {
        super::SystemBackend.set_text(plain)
    }
}
