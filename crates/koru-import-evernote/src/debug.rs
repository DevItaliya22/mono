// SPDX-License-Identifier: MPL-2.0

use std::fmt;
use std::sync::OnceLock;

use koru_import::ImportWarningCode;

static ENABLED: OnceLock<bool> = OnceLock::new();

fn enabled() -> bool {
    *ENABLED.get_or_init(|| koru_env::switch(koru_env::name::KORU_IMPORT_DEBUG))
}

enum Fact<'a> {
    Note {
        ordinal: usize,
    },
    ParserFailure {
        pass: ParsePass,
        scope: ParseScope,
        note_ordinal: Option<usize>,
        stream_offset: u64,
    },
    ParserRecovery {
        pass: ParsePass,
        scope: ParseScope,
        note_ordinal: Option<usize>,
        stream_offset: u64,
    },
    Warning {
        code: ImportWarningCode,
    },
    Resource {
        digest: &'a str,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum ParsePass {
    Inspection,
    Content,
}

impl fmt::Display for ParsePass {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(match self {
            Self::Inspection => "inspection",
            Self::Content => "content",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParseScope {
    Envelope,
    Note,
    Title,
    Guid,
    Deleted,
    Active,
    Tag,
    Content,
    Resource,
    ResourceAttributes,
    ResourceMime,
    ResourceWidth,
    ResourceHeight,
    ResourceFilename,
    ResourceAttachment,
    ResourceSourceUrl,
    ResourceTimestamp,
    ResourceLatitude,
    ResourceLongitude,
    ResourceAltitude,
    ResourceCameraMake,
    ResourceCameraModel,
    ResourceClientWillIndex,
    ResourceRecoType,
    ResourceApplicationData,
    ApplicationData,
}

impl fmt::Display for ParseScope {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(match self {
            Self::Envelope => "envelope",
            Self::Note => "note",
            Self::Title => "title",
            Self::Guid => "guid",
            Self::Deleted => "deleted",
            Self::Active => "active",
            Self::Tag => "tag",
            Self::Content => "content",
            Self::Resource => "resource",
            Self::ResourceAttributes => "resource_attributes",
            Self::ResourceMime => "resource_mime",
            Self::ResourceWidth => "resource_width",
            Self::ResourceHeight => "resource_height",
            Self::ResourceFilename => "resource_filename",
            Self::ResourceAttachment => "resource_attachment",
            Self::ResourceSourceUrl => "resource_source_url",
            Self::ResourceTimestamp => "resource_timestamp",
            Self::ResourceLatitude => "resource_latitude",
            Self::ResourceLongitude => "resource_longitude",
            Self::ResourceAltitude => "resource_altitude",
            Self::ResourceCameraMake => "resource_camera_make",
            Self::ResourceCameraModel => "resource_camera_model",
            Self::ResourceClientWillIndex => "resource_client_will_index",
            Self::ResourceRecoType => "resource_reco_type",
            Self::ResourceApplicationData => "resource_application_data",
            Self::ApplicationData => "application_data",
        })
    }
}

impl fmt::Display for Fact<'_> {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Note { ordinal } => write!(output, "note ordinal={ordinal}"),
            Self::ParserFailure {
                pass,
                scope,
                note_ordinal,
                stream_offset,
            }
            | Self::ParserRecovery {
                pass,
                scope,
                note_ordinal,
                stream_offset,
            } => {
                let event = if matches!(self, Self::ParserFailure { .. }) {
                    "parser_failure"
                } else {
                    "parser_recovery"
                };
                write!(output, "{event} pass={pass} scope={scope}")?;
                match note_ordinal {
                    Some(ordinal) => write!(output, " note_ordinal={ordinal}")?,
                    None => output.write_str(" note_ordinal=none")?,
                }
                write!(output, " stream_offset={stream_offset}")
            }
            Self::Warning { code } => write!(output, "warning code={}", code.as_str()),
            Self::Resource { digest } => write!(output, "resource digest={digest}"),
        }
    }
}

fn write(fact: Fact<'_>) {
    if enabled() {
        eprintln!("[Koru][import-debug] {fact}");
    }
}

pub(crate) fn note(ordinal: usize) {
    write(Fact::Note { ordinal });
}

pub(crate) fn parser_failure(
    pass: ParsePass,
    scope: ParseScope,
    note_ordinal: Option<usize>,
    stream_offset: u64,
) {
    write(Fact::ParserFailure {
        pass,
        scope,
        note_ordinal,
        stream_offset,
    });
}

pub(crate) fn parser_recovery(
    pass: ParsePass,
    scope: ParseScope,
    note_ordinal: Option<usize>,
    stream_offset: u64,
) {
    write(Fact::ParserRecovery {
        pass,
        scope,
        note_ordinal,
        stream_offset,
    });
}

pub(crate) fn warning(code: ImportWarningCode) {
    write(Fact::Warning { code });
}

pub(crate) fn resource(digest: &str) {
    write(Fact::Resource { digest });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_facts_have_a_closed_content_free_shape() {
        assert_eq!(Fact::Note { ordinal: 7 }.to_string(), "note ordinal=7");
        assert_eq!(
            Fact::ParserFailure {
                pass: ParsePass::Inspection,
                scope: ParseScope::Title,
                note_ordinal: Some(2),
                stream_offset: 4096,
            }
            .to_string(),
            "parser_failure pass=inspection scope=title note_ordinal=2 stream_offset=4096"
        );
        assert_eq!(
            Fact::ParserFailure {
                pass: ParsePass::Content,
                scope: ParseScope::ResourceFilename,
                note_ordinal: Some(0),
                stream_offset: 12,
            }
            .to_string(),
            "parser_failure pass=content scope=resource_filename note_ordinal=0 stream_offset=12"
        );
        assert_eq!(
            Fact::ParserRecovery {
                pass: ParsePass::Inspection,
                scope: ParseScope::ResourceApplicationData,
                note_ordinal: Some(0),
                stream_offset: 4403,
            }
            .to_string(),
            "parser_recovery pass=inspection scope=resource_application_data note_ordinal=0 stream_offset=4403"
        );
        assert_eq!(
            Fact::Warning {
                code: ImportWarningCode::EnexSecurityRejected,
            }
            .to_string(),
            "warning code=enex_security_rejected"
        );
        assert_eq!(
            Fact::Resource {
                digest: "blake3-digest",
            }
            .to_string(),
            "resource digest=blake3-digest"
        );
    }

    #[test]
    fn the_debug_variable_name_is_import_specific() {
        assert_eq!(koru_env::name::KORU_IMPORT_DEBUG, "KORU_IMPORT_DEBUG");
    }
}
