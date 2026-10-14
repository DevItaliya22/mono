// SPDX-License-Identifier: MPL-2.0

use super::*;

#[test]
fn a_filename_is_bounded_and_refuses_control_characters() {
    assert!(Filename::new("report.md").is_ok());
    assert_eq!(Filename::new("   "), Err(TextError::Empty));
    assert_eq!(Filename::new("a\nb"), Err(TextError::ControlCharacter));
    assert_eq!(
        Filename::new("x".repeat(MAX_FILENAME_CHARS + 1)),
        Err(TextError::TooLong {
            max_chars: MAX_FILENAME_CHARS
        })
    );
    // Bounded in scalar values, not bytes: the limit is one rule in every
    // language.
    assert!(Filename::new("報".repeat(MAX_FILENAME_CHARS)).is_ok());
}

#[test]
fn a_filename_is_refused_on_deserialization_too() {
    let refused: std::result::Result<Filename, _> = serde_json::from_str("\"a\\u0000b\"");
    assert!(refused.is_err(), "a request body is the boundary");
    let accepted: Filename = serde_json::from_str("\"notes.txt\"").unwrap();
    assert_eq!(accepted.as_str(), "notes.txt");
    assert_eq!(serde_json::to_string(&accepted).unwrap(), "\"notes.txt\"");
}

#[test]
fn a_media_type_is_type_slash_subtype_lowercased() {
    assert_eq!(
        MediaType::new("Text/Markdown").unwrap().as_str(),
        "text/markdown"
    );
    assert_eq!(
        MediaType::new("application/vnd.koru+json")
            .unwrap()
            .as_str(),
        "application/vnd.koru+json"
    );
    assert_eq!(MediaType::new("markdown"), Err(MediaTypeError::Malformed));
    assert_eq!(MediaType::new("text/"), Err(MediaTypeError::Malformed));
    assert_eq!(
        MediaType::new("text/plain; charset=utf-8"),
        Err(MediaTypeError::Malformed)
    );
    assert_eq!(
        MediaType::new(""),
        Err(MediaTypeError::Text(TextError::Empty))
    );
    let refused: std::result::Result<MediaType, _> = serde_json::from_str("\"nonsense\"");
    assert!(refused.is_err());
}

#[test]
fn a_media_type_prints_in_full_and_a_filename_does_not() {
    let mime = MediaType::new("image/png").unwrap();
    assert_eq!(format!("{mime:?}"), "MediaType(image/png)");
    assert_eq!(mime.to_string(), "image/png");
    let name = Filename::new(FILENAME_SENTINEL).unwrap();
    assert_eq!(format!("{name:?}"), "Filename(<redacted>)");
    assert_eq!(name.char_count(), FILENAME_SENTINEL.chars().count());
}

// ---------------------------------------------------------- relative paths

/// The listing's path type refuses everything the register path rule would
/// have to refuse later, so a listing this type could not have produced can
/// never be echoed back as a registration.
#[test]
fn a_relative_path_refuses_every_shape_that_is_not_inside_a_folder() {
    assert!(RelativePath::new("report.md").is_ok());
    assert!(RelativePath::new("out/report.md").is_ok());
    assert!(RelativePath::new("out/deep/report.md").is_ok());
    // A colon is not a drive letter. A Mac and a Linux box both allow one
    // in a filename, and a result nobody can see is worse than a rule
    // nobody needed.
    assert!(RelativePath::new("notes:draft.md").is_ok());
    assert!(RelativePath::new("out/a:b.md").is_ok());
    for refused in [
        "/etc/passwd",
        "..",
        "../secret",
        "out/../../secret",
        "out//report.md",
        "./report.md",
        "outBACKSLASHreport.md",
        "C:/Windows/system32",
    ] {
        let refused = refused.replace("BACKSLASH", "\\");
        assert_eq!(
            RelativePath::new(refused.clone()),
            Err(RelativePathError::Malformed),
            "{refused} was accepted"
        );
    }
    assert_eq!(
        RelativePath::new("   "),
        Err(RelativePathError::Text(TextError::Empty))
    );
    assert_eq!(
        RelativePath::new(String::from_iter(['a', char::from(7), 'b'])),
        Err(RelativePathError::Text(TextError::ControlCharacter))
    );
    assert_eq!(
        RelativePath::new("x".repeat(MAX_RELATIVE_PATH_CHARS + 1)),
        Err(RelativePathError::Text(TextError::TooLong {
            max_chars: MAX_RELATIVE_PATH_CHARS
        }))
    );
}

/// A path is content: it never reaches a `Debug`, and the last segment is
/// what a row is labelled with.
#[test]
fn a_relative_path_redacts_and_names_its_last_segment() {
    let path = RelativePath::new("out/deep/PATH-SENTINEL.md").unwrap();
    assert!(!format!("{path:?}").contains("PATH-SENTINEL"));
    assert_eq!(path.file_name(), "PATH-SENTINEL.md");
    assert_eq!(path.depth(), 3);
    assert_eq!(RelativePath::new("a.md").unwrap().depth(), 1);
    // It crosses the wire as the string it is, and a refused shape is
    // refused on the way back in too.
    let json = serde_json::to_string(&path).unwrap();
    assert_eq!(json, "\"out/deep/PATH-SENTINEL.md\"");
    assert!(serde_json::from_str::<RelativePath>("\"../escape\"").is_err());
}
