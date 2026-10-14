// SPDX-License-Identifier: MPL-2.0

//! The store-status chip's popover, in the reader's language.
//!
//! **Words moved here; the facts stayed where they are known.** The chip's
//! third line used to be built by `koru_workspace::StoreTrouble`, a crate that
//! reaches no locale and had no business owning a sentence — so the same window
//! drew a translated chip, a translated title, and an English paragraph under
//! them. `koru-workspace` now hands over what it knows — a count, two flags and
//! the transport's own diagnostic — and this module spells it.
//!
//! **Here rather than in the two hosts.** The container caption and a
//! popped-out document's caption both draw this chip, and two copies of a
//! three-branch assembly is how they start saying the same fact two ways. The
//! hosts keep only the field-for-field map into [`Trouble`], which has no words
//! in it to disagree about.
//!
//! A sibling of [`crate::time`]: bucket-and-branch arithmetic here, every word
//! in the table.

use crate::strings::{Key, t};
use crate::{Lang, fill};

/// What the popover has to say, as facts rather than as words.
///
/// The shape `koru_workspace::StoreTrouble` already carries, minus everything
/// this crate cannot use: `owed` counts batches in flight and the popover does
/// not report it, and `since` is a clock reading nothing here is allowed to
/// interpret.
///
/// **`not_buffered` is a `bool` and the reason is dropped.** The reason is
/// English from the store's own error path and is shown by the closing dialog,
/// not by this line; what the line changes on is whether there *is* one.
pub struct Trouble<'a> {
    /// How many open documents hold work the store has not confirmed.
    pub documents: usize,
    /// Whether the workspace's own list of documents is unconfirmed too.
    /// Counted apart from `documents` because it is not one of them.
    pub document_list: bool,
    /// Whether some of that work could not reach this machine's disk either —
    /// the one state where closing the window loses something.
    pub not_buffered: bool,
    /// The link will not recover without somebody doing something.
    pub fatal: bool,
    /// The transport's own diagnosis, **verbatim and untranslated**. An
    /// endpoint, a protocol mismatch or an OS error, which is what tells a
    /// running-but-incompatible store apart from no store at all. Inventing
    /// eight translations of a socket error would lose the one thing it is for.
    pub message: &'a str,
}

/// The chip's second popover line: what is at stake, then what the link is
/// doing about it.
///
/// The safety statement comes first — it is the part about the person's own
/// work — and the transport diagnosis follows.
pub fn popover_detail(lang: Lang, trouble: Trouble<'_>) -> String {
    let safety = safety(lang, &trouble);
    // The diagnostic arrives with whatever punctuation its source gave it, and
    // the frame supplies the full stop. Trimming one rather than printing "..".
    let message = trouble.message.trim().trim_end_matches('.');
    let frame = if trouble.fatal {
        Key::StoreOfflineStopped
    } else {
        Key::StoreOfflineRetrying
    };
    fill(t(lang, frame), &[&safety, message])
}

/// Whether the person's work is safe — the sentence somebody actually opened
/// the popover for.
///
/// **Three answers, not a count.** Work this machine buffered is as safe as
/// work the store confirmed, and work in neither place is the only case worth
/// worrying about. Zero is a real answer and is said rather than hidden.
fn safety(lang: Lang, trouble: &Trouble<'_>) -> String {
    if trouble.documents == 0 && !trouble.document_list {
        return t(lang, Key::StoreOfflineNothingWaiting).to_owned();
    }
    // **Flat rows, one per shape, rather than a phrase glued into a phrase.**
    // "and so does this workspace's list" is a clause English can append and
    // Japanese has to weave in, so each combination is its own row.
    let outstanding = match (trouble.documents, trouble.document_list) {
        (0, _) => t(lang, Key::StoreOfflineListOnly).to_owned(),
        (1, false) => t(lang, Key::StoreOfflineOneDocument).to_owned(),
        (1, true) => t(lang, Key::StoreOfflineOneDocumentAndList).to_owned(),
        // The count is spelled the way `crate::time` spells its own: plain
        // digits, because these rows read as a sentence beside a relative time
        // and one of the two using Arabic-Indic digits would be the odd one.
        (n, false) => fill(t(lang, Key::StoreOfflineManyDocuments), &[&n.to_string()]),
        (n, true) => fill(
            t(lang, Key::StoreOfflineManyDocumentsAndList),
            &[&n.to_string()],
        ),
    };
    let frame = if trouble.not_buffered {
        Key::StoreOfflineNotBuffered
    } else {
        Key::StoreOfflineBuffered
    };
    fill(t(lang, frame), &[&outstanding])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trouble(
        documents: usize,
        document_list: bool,
        not_buffered: bool,
        fatal: bool,
    ) -> Trouble<'static> {
        Trouble {
            documents,
            document_list,
            not_buffered,
            fatal,
            message: "the store went away",
        }
    }

    /// **The English is the wording `koru_workspace::StoreTrouble` shipped**,
    /// character for character. Moving a sentence between crates is allowed to
    /// change who owns it and is not allowed to change what an English reader
    /// sees, and these goldens were taken from the code that was deleted.
    #[test]
    fn the_english_wording_is_unchanged_by_the_move() {
        let at = |t: Trouble<'_>| popover_detail(Lang::En, t);
        assert_eq!(
            at(trouble(0, false, false, false)),
            "Nothing is waiting to be saved. Koru is retrying automatically. \
             Last error: the store went away."
        );
        assert_eq!(
            at(trouble(1, false, false, false)),
            "1 document has changes, saved on this computer until the store is \
             back. Koru is retrying automatically. Last error: the store went away."
        );
        assert_eq!(
            at(trouble(3, false, false, false)),
            "3 documents have changes, saved on this computer until the store is \
             back. Koru is retrying automatically. Last error: the store went away."
        );
        assert_eq!(
            at(trouble(2, true, false, false)),
            "2 documents have changes, and so does this workspace's list of \
             documents, saved on this computer until the store is back. Koru is \
             retrying automatically. Last error: the store went away."
        );
        assert_eq!(
            at(trouble(0, true, false, false)),
            "This workspace's list of documents has changes, saved on this \
             computer until the store is back. Koru is retrying automatically. \
             Last error: the store went away."
        );
        assert_eq!(
            at(trouble(1, false, true, false)),
            "1 document has changes, and they could not be saved on this computer \
             either. Koru is retrying automatically. Last error: the store went away."
        );
        assert_eq!(
            at(trouble(1, true, true, true)),
            "1 document has changes, and so does this workspace's list of \
             documents, and they could not be saved on this computer either. \
             Connection stopped: the store went away."
        );
    }

    /// The diagnostic ends in the frame's full stop rather than in two of them,
    /// and surrounding whitespace never reaches the screen.
    #[test]
    fn the_diagnostic_keeps_one_full_stop_however_it_arrives() {
        let with = |message| {
            popover_detail(
                Lang::En,
                Trouble {
                    documents: 0,
                    document_list: false,
                    not_buffered: false,
                    fatal: true,
                    message,
                },
            )
        };
        assert_eq!(
            with("  refused. "),
            "Nothing is waiting to be saved. Connection stopped: refused."
        );
        assert_eq!(
            with("refused"),
            "Nothing is waiting to be saved. Connection stopped: refused."
        );
    }

    /// Every language answers for every shape, no `{}` survives to the screen,
    /// and the untranslatable half — the transport's own words — is still in
    /// there to be read out over a support call.
    #[test]
    fn every_language_says_something_for_every_shape() {
        let shapes = [
            (0, false),
            (1, false),
            (4, false),
            (0, true),
            (1, true),
            (4, true),
        ];
        for lang in Lang::ALL {
            for (documents, document_list) in shapes {
                for not_buffered in [false, true] {
                    for fatal in [false, true] {
                        let line = popover_detail(
                            lang,
                            trouble(documents, document_list, not_buffered, fatal),
                        );
                        assert!(!line.trim().is_empty(), "{} said nothing", lang.tag());
                        assert!(
                            !line.contains("{}"),
                            "{} left a placeholder in {line}",
                            lang.tag()
                        );
                        assert!(
                            line.contains("the store went away"),
                            "{} dropped the diagnostic",
                            lang.tag()
                        );
                    }
                }
            }
        }
    }

    /// The count reaches the sentence wherever the language puts it — the whole
    /// reason these are templates rather than a `format!` in the workspace.
    #[test]
    fn the_document_count_is_placed_by_each_language() {
        for lang in Lang::ALL {
            let line = popover_detail(lang, trouble(7, false, false, false));
            assert!(line.contains('7'), "{} lost the count: {line}", lang.tag());
        }
    }
}
