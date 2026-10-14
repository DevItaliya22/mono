// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_i18n::Lang;

/// A rooted acceptance stack must neither read nor write the real machine's
/// consent. Agent and the window meet on this exact sibling path.
#[test]
fn a_rooted_consent_record_lives_beside_the_rooted_agent() {
    let root = std::path::Path::new("C:/isolated-koru");
    assert_eq!(
        consent_path(Some(root)),
        Some(root.join("agent").join(koru_voice_consent::CONSENT_FILE))
    );
}

fn at(stage: InstallStage, received: u64, total: u64) -> InstallProgress {
    InstallProgress {
        stage,
        received,
        total,
    }
}

/// **Every stage says something, and only one of them shows a bar.**
///
/// The bar is the download's alone: a determinate bar sitting still at 100%
/// through a two-minute re-hash reads as a hang, which is the reason
/// `InstallStage::Verifying` exists at all.
#[test]
fn each_stage_has_a_sentence_and_only_the_download_has_a_bar() {
    let (preparing, bar) = progress_line(Lang::En, at(InstallStage::Resolving, 0, 0));
    assert!(!preparing.is_empty());
    assert_eq!(bar, None);

    let (downloading, bar) = progress_line(Lang::En, at(InstallStage::Downloading, 50, 200));
    assert!(downloading.contains("25"), "{downloading}");
    assert_eq!(bar, Some(0.25));

    for stage in [
        InstallStage::Verifying,
        InstallStage::Unpacking,
        InstallStage::Publishing,
    ] {
        let (line, bar) = progress_line(Lang::En, at(stage, 0, 0));
        assert!(!line.is_empty(), "{stage:?} says nothing");
        assert_eq!(bar, None, "{stage:?} drew a bar");
    }
}

/// A download whose total is not known yet is indeterminate rather than a
/// division by zero.
#[test]
fn a_download_with_no_total_is_indeterminate() {
    let (line, bar) = progress_line(Lang::En, at(InstallStage::Downloading, 900, 0));
    assert!(!line.is_empty());
    assert_eq!(bar, None);
}

/// More received than the total — a server that lied, a resume that
/// over-counted — clamps rather than drawing a bar past its own end.
#[test]
fn a_bar_cannot_pass_its_own_end() {
    let (_, bar) = progress_line(Lang::En, at(InstallStage::Downloading, 500, 200));
    assert_eq!(bar, Some(1.0));
}

/// **Every sentence resolves in all eight languages**, which is the house rule
/// and is easy to break by adding a row in English and moving on.
#[test]
fn every_stage_speaks_all_eight_languages() {
    for lang in [
        Lang::En,
        Lang::Es,
        Lang::PtBr,
        Lang::De,
        Lang::Fr,
        Lang::Ar,
        Lang::ZhHans,
        Lang::Ja,
    ] {
        for stage in [
            InstallStage::Resolving,
            InstallStage::Downloading,
            InstallStage::Verifying,
            InstallStage::Unpacking,
            InstallStage::Publishing,
        ] {
            let (line, _) = progress_line(lang, at(stage, 1, 2));
            assert!(
                !line.trim().is_empty(),
                "{lang:?} says nothing at {stage:?}"
            );
        }
    }
}

/// Somebody pressing **Stop** is not a failure, and the toast must not follow
/// *Canceling* with a sentence saying the model could not be installed.
#[test]
fn a_cancellation_is_told_apart_from_a_failure() {
    assert!(was_cancelled(&ModelErrorCode::Cancelled.with("stopped")));
    for other in [
        ModelErrorCode::DownloadFailed,
        ModelErrorCode::NoTrustRoot,
        ModelErrorCode::ConsentRequired,
        ModelErrorCode::Io,
    ] {
        assert!(!was_cancelled(&other.with("x")), "{other} is not a Stop");
    }
}

// ----------------------------------------------------------- trust policy

/// **A pinned key wins, and there is no way to ask for anything else.**
///
/// The MVP's digest-only arm is reachable only while `MODEL_SIGNING_KEYS` is
/// empty; the day it is not, this function answers `Signed` and the other arm
/// is dead code somebody deletes. That is the whole deletion plan, asserted.
#[test]
fn a_pinned_key_forces_the_signed_policy() {
    if koru_voice_models::MODEL_SIGNING_KEYS.is_empty() {
        // The state of the tree today: the guarded arm, or `Signed` on stable.
        let chosen = trust_policy();
        let expected = TrustPolicy::digest_only()
            .map(|p| p.is_digest_only())
            .unwrap_or(false);
        assert_eq!(chosen.is_digest_only(), expected);
    } else {
        assert_eq!(trust_policy(), TrustPolicy::SIGNED);
    }
}

/// **A stable build never chooses the MVP arm**, however this window is built:
/// `digest_only` refuses there and the fall-back is the policy that refuses the
/// install outright, which is the failure a public release must have.
#[test]
fn a_stable_build_falls_back_to_signed() {
    assert!(TrustPolicy::digest_only_on(Some("stable")).is_err());
}

// ------------------------------------------------- the sentences a person gets

/// **Every catalog failure gets its own sentence**, and the sentences are
/// asserted rather than merely counted.
///
/// This is the whole of the bug that prompted the pass: a person set
/// `KORU_VOICE_CATALOG` to a host with nothing published on it, pressed
/// Install, and got *"No voice model is available to download yet"* — which is
/// true of exactly one of these four and misleading about the other three.
/// Asserting the *codes are distinct* would have passed before the fix, so
/// what is asserted is what somebody reads.
#[test]
fn every_catalog_failure_says_something_a_person_can_act_on() {
    let unreachable = catalog_failure(&ModelErrorCode::Offline.with("dns"));
    assert!(
        unreachable.contains("Could not reach") && unreachable.contains("connection"),
        "{unreachable}"
    );

    let missing = catalog_failure(&ModelErrorCode::CatalogUnavailable.with("404"));
    assert!(
        missing.contains("not published") && missing.contains("KORU_VOICE_CATALOG"),
        "a 404 has to name the thing to check: {missing}"
    );

    let malformed = catalog_failure(&ModelErrorCode::CatalogInvalid.with("bad json"));
    assert!(malformed.contains("could not be read"), "{malformed}");

    let nothing_for_us = catalog_failure(&ModelErrorCode::LanguageUnsupported.with("no compact"));
    assert!(
        nothing_for_us.contains("no speech model this build can install"),
        "{nothing_for_us}"
    );

    let all = [&unreachable, &missing, &malformed, &nothing_for_us];
    for (i, one) in all.iter().enumerate() {
        for (j, other) in all.iter().enumerate() {
            if i != j {
                assert_ne!(one, other, "two catalog failures say the same thing");
            }
        }
    }
}

/// **No code reaches a person as silence, or as a token.**
///
/// Every arm answers a non-empty sentence that reads as English rather than as
/// a code — the fall-through arm names its code *inside* a sentence, which is
/// the honest answer for a failure nobody wrote copy for and is still not the
/// same as printing `catalog_invalid` at somebody.
#[test]
fn no_model_failure_is_ever_wordless() {
    for code in ModelErrorCode::ALL {
        for line in [
            catalog_failure(&code.with("detail")),
            install_failure(&code.with("detail")),
        ] {
            assert!(!line.trim().is_empty(), "{code:?} says nothing");
            assert!(
                line.contains(' '),
                "{code:?} answers a token rather than a sentence: {line}"
            );
            assert!(
                line.ends_with('.') || line.ends_with(".)"),
                "{code:?} is not a sentence: {line}"
            );
        }
    }
}

/// **A download failure and a verification failure are not the same news.**
///
/// One is *try again*; the other is *nothing was installed and something is
/// wrong with the bytes*. The old single sentence said neither.
#[test]
fn an_install_failure_says_which_kind_it_was() {
    let network = install_failure(&ModelErrorCode::DownloadFailed.with("reset"));
    assert!(network.contains("connection"), "{network}");
    assert!(
        network.contains("carry on"),
        "a resume is worth saying: {network}"
    );

    let signature = install_failure(&ModelErrorCode::SignatureInvalid.with("bad sig"));
    assert!(signature.contains("signature"), "{signature}");
    assert!(signature.contains("Nothing was installed"), "{signature}");

    let disk = install_failure(&ModelErrorCode::Io.with("no space"));
    assert!(disk.contains("room on the drive"), "{disk}");

    assert_ne!(network, signature);
    assert_ne!(network, disk);
    assert_ne!(signature, disk);
}

/// **A press that starts nothing still says why.** Both of these used to be a
/// silent `return`, which makes *Install…* indistinguishable from a broken
/// button.
#[test]
fn a_setup_that_cannot_start_is_not_silent() {
    assert_eq!(
        setup_refused(false, false),
        None,
        "the ordinary case proceeds"
    );

    let busy = setup_refused(true, false).expect("a busy press says something");
    assert!(busy.contains("already being set up"), "{busy}");

    let off = setup_refused(false, true).expect("a switched-off press says something");
    assert!(off.contains("KORU_VOICE_DISABLED"), "{off}");

    // The kill switch outranks being busy: an installation that offers no
    // dictation has nothing in flight worth reporting.
    assert_eq!(setup_refused(true, true), setup_refused(false, true));
}

// ------------------------------------------------------- what a tier weighs

/// **The size is a descriptor's, and an unread one is no size at all.**
///
/// A literal in the UI drifts from what actually downloads the first time a
/// model is repackaged, so the only two sources are the installed cache and a
/// catalog that was actually fetched — and before either has happened the
/// honest answer is `None`.
#[test]
fn a_tier_weighs_what_a_descriptor_said_or_nothing() {
    // Bytes to megabytes is `koru_voice_consent::megabytes`, rounded once here
    // rather than at each surface.
    note_tier_size(ModelTier::HighAccuracy, 475 * 1024 * 1024);
    assert_eq!(tier_megabytes(ModelTier::HighAccuracy), Some(475));

    // A descriptor claiming nothing records nothing: zero is not a size, and
    // storing it would make "unknown" and "empty" the same answer.
    let was = tier_megabytes(ModelTier::Compact);
    note_tier_size(ModelTier::Compact, 0);
    assert_eq!(tier_megabytes(ModelTier::Compact), was);
}

#[test]
fn the_product_offer_combines_both_downloads_and_both_licences() {
    let offer = VoiceOffer {
        terms: vec![
            Terms {
                model: "whisper-base".into(),
                version: "q5_1".into(),
                licence: "MIT".into(),
                attribution: "whisper.cpp".into(),
                download_bytes: 57 * 1024 * 1024,
            },
            Terms {
                model: "nemotron".into(),
                version: "int8".into(),
                licence: "OpenMDW-1.1".into(),
                attribution: "NVIDIA".into(),
                download_bytes: 475 * 1024 * 1024,
            },
        ],
    };
    assert_eq!(offer.megabytes(), 532);
    assert_eq!(offer.licences(), "MIT + OpenMDW-1.1");
    assert_eq!(offer.attributions(), "whisper.cpp; NVIDIA");
}

#[test]
fn installed_is_not_the_same_as_both_tiers_ready() {
    let partial = VoiceModels {
        builds: 1,
        bytes: 57,
        tiers: [true, false],
    };
    assert!(partial.installed());
    assert!(!partial.complete());

    assert!(
        VoiceModels {
            builds: 2,
            bytes: 532,
            tiers: [true, true],
        }
        .complete()
    );
}
