// SPDX-License-Identifier: MPL-2.0

//! One file-type vocabulary for every list that can show an attachment.
//!
//! MIME is authoritative when a caller has it. The name is a compatibility
//! fallback for old manifest and search rows that predate MIME in their
//! projection; it also keeps an `application/octet-stream` upload useful.

use koru_draw::icons;

/// A lightweight, data-only file icon component.
///
/// Keeping the decision here means Explore, Trash and Search cannot slowly
/// acquire three different extension lists or generic-file fallbacks.
#[derive(Copy, Clone, Debug, Default)]
pub struct FileIcon<'a> {
    pub mime: Option<&'a str>,
    pub name: Option<&'a str>,
}

impl<'a> FileIcon<'a> {
    pub const fn new(mime: Option<&'a str>, name: Option<&'a str>) -> Self {
        Self { mime, name }
    }

    pub fn glyph(self) -> char {
        glyph(self.mime, self.name)
    }
}

/// Pick the Phosphor glyph for a stored file.
pub fn glyph(mime: Option<&str>, name: Option<&str>) -> char {
    let mime = mime
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or("")
        .to_ascii_lowercase();

    if !mime.is_empty() && mime != "application/octet-stream" {
        if let Some(icon) = mime_glyph(&mime) {
            return icon;
        }
    }

    extension_glyph(name).unwrap_or(icons::FILE)
}

fn mime_glyph(mime: &str) -> Option<char> {
    Some(match mime {
        "application/pdf" => icons::FILE_PDF,
        "text/markdown" | "text/x-markdown" => icons::FILE_MD,
        "application/msword"
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.oasis.opendocument.text"
        | "application/rtf"
        | "text/rtf" => icons::FILE_DOC,
        "application/vnd.ms-excel"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/vnd.oasis.opendocument.spreadsheet"
        | "text/csv" => icons::FILE_XLS,
        "application/vnd.ms-powerpoint"
        | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        | "application/vnd.oasis.opendocument.presentation" => icons::FILE_PPT,
        "application/zip"
        | "application/x-zip-compressed"
        | "application/x-7z-compressed"
        | "application/x-rar-compressed"
        | "application/gzip"
        | "application/x-tar" => icons::FILE_ZIP,
        "application/json"
        | "application/ld+json"
        | "application/xml"
        | "application/javascript"
        | "application/wasm" => icons::FILE_CODE,
        value if value.starts_with("image/") => icons::FILE_IMAGE,
        value if value.starts_with("audio/") => icons::FILE_AUDIO,
        value if value.starts_with("video/") => icons::FILE_VIDEO,
        "text/html" | "text/css" | "text/javascript" | "text/xml" => icons::FILE_CODE,
        value if value.starts_with("text/") => icons::DOCUMENT,
        _ => return None,
    })
}

fn extension_glyph(name: Option<&str>) -> Option<char> {
    let extension = name?.rsplit_once('.')?.1.trim().to_ascii_lowercase();
    Some(match extension.as_str() {
        "pdf" => icons::FILE_PDF,
        "doc" | "docx" | "odt" | "rtf" => icons::FILE_DOC,
        "xls" | "xlsx" | "ods" | "csv" => icons::FILE_XLS,
        "ppt" | "pptx" | "odp" => icons::FILE_PPT,
        "md" | "markdown" | "mdown" | "mkd" => icons::FILE_MD,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "svg" | "tif" | "tiff"
        | "heic" => icons::FILE_IMAGE,
        "zip" | "7z" | "rar" | "gz" | "tgz" | "tar" => icons::FILE_ZIP,
        "mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" => icons::FILE_AUDIO,
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" => icons::FILE_VIDEO,
        "rs" | "js" | "jsx" | "ts" | "tsx" | "json" | "html" | "css" | "xml" | "toml" | "yaml"
        | "yml" | "sh" | "ps1" | "py" | "go" | "java" | "c" | "h" | "cpp" | "hpp" => {
            icons::FILE_CODE
        }
        "txt" | "log" => icons::DOCUMENT,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_is_authoritative() {
        assert_eq!(
            glyph(Some("application/pdf"), Some("wrong.docx")),
            icons::FILE_PDF
        );
        assert_eq!(
            glyph(Some("image/png; charset=binary"), Some("photo.bin")),
            icons::FILE_IMAGE
        );
    }

    #[test]
    fn names_cover_old_rows_and_generic_uploads() {
        assert_eq!(glyph(None, Some("notes.md")), icons::FILE_MD);
        assert_eq!(
            glyph(Some("application/octet-stream"), Some("deck.pptx")),
            icons::FILE_PPT
        );
        assert_eq!(glyph(None, Some("README")), icons::FILE);
    }
}
