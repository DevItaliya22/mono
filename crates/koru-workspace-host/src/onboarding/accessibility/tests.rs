// SPDX-License-Identifier: MPL-2.0

//! **The account-free route, screen by screen, through the seam a screen
//! reader uses.**
//!
//! Nothing here opens a window, touches a disk, reaches a Store or creates a
//! Profile: an `OnboardingView` is a value, its effects leave as typed
//! `Action`s, and every assertion below is against the tree and the actions
//! that value produces. The one thing this lane cannot answer — *does an
//! external UIA client see this?* — is
//! `tests/accessibility_onboarding_native.rs`. The one thing neither can
//! answer — *what does Narrator say?* — is Plan U0.2.

use super::*;

use crate::accessibility::{Occurrence, SemanticNode, Slots, ViewSlot, WindowSlot};
use crate::onboarding::Action as FormAction;
use vello::{Scene, kurbo::Affine};
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};

// ------------------------------------------------------------------ helpers

fn slots() -> (WindowSlot, ViewSlot) {
    let mut slots = Slots::default();
    (slots.window(), slots.view())
}

/// A form sized like the real window, so the geometry functions answer the way
/// they do on screen.
fn form() -> OnboardingView {
    let mut view = OnboardingView::default();
    view.set_viewport(1000.0, 680.0);
    view.set_custom_caption(true);
    view
}

fn flatten(nodes: &[SemanticNode]) -> Vec<SemanticNode> {
    let mut out = Vec::new();
    for node in nodes {
        out.push(node.clone());
        out.extend(flatten(&node.children));
    }
    out
}

/// The tree this form publishes right now, flattened.
fn tree(view: &mut OnboardingView) -> Vec<SemanticNode> {
    let (window, slot) = slots();
    flatten(&view.accessibility_semantics(window, slot))
}

/// Find a control by its part, which is the only stable way to name one.
fn find(nodes: &[SemanticNode], part: ControlPart) -> Option<&SemanticNode> {
    nodes.iter().find(|node| node.occurrence.part == part)
}

fn at(nodes: &[SemanticNode], part: ControlPart) -> Occurrence {
    find(nodes, part)
        .unwrap_or_else(|| panic!("{part} is not in the tree"))
        .occurrence
}

fn labels(nodes: &[SemanticNode]) -> Vec<String> {
    nodes.iter().filter_map(|node| node.label.clone()).collect()
}

fn value(text: &str) -> ActionData {
    ActionData::Value(text.into())
}

/// Walk the account-free route to the Profile step the way a person does.
fn at_profile() -> OnboardingView {
    let (window, slot) = slots();
    let mut view = form();
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let offline = at(&nodes, parts::CONTINUE_OFFLINE);
    assert!(view.accessibility_action(offline, Action::Click, None));
    view
}

/// …and on to the Workspace step, with a complete Profile behind it.
fn at_workspace() -> OnboardingView {
    let (window, slot) = slots();
    let mut view = at_profile();
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let owner = at(&nodes, parts::OWNER_NAME);
    assert!(view.accessibility_action(owner, Action::SetValue, Some(&value("Ada"))));
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let work = at(&nodes, parts::PURPOSE_WORK);
    assert!(view.accessibility_action(work, Action::Click, None));
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let primary = at(&nodes, parts::PRIMARY);
    assert!(view.accessibility_action(primary, Action::Click, None));
    view
}

/// …and with a starting point chosen, which is what fills the workspace's
/// name. The Workspace step opens with *Project* marked and the field still
/// empty; pressing a card is one of the two ways to fill it, and typing is the
/// other. Until one of them happens the primary is unavailable, which is the
/// product's own rule and not something this projection invented.
fn at_workspace_ready() -> OnboardingView {
    let (window, slot) = slots();
    let mut view = at_workspace();
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let project = at(&nodes, parts::TEMPLATE[0]);
    assert!(view.accessibility_action(project, Action::Click, None));
    view
}

// -------------------------------------------------- the tree of every screen

/// **The welcome screen a first-run window actually opens on.**
///
/// This is the screen tested with Accessibility Insights and Narrator, which
/// showed one window element with nothing inside it. Everything named here is
/// what was missing.
#[test]
fn the_welcome_screen_publishes_both_ways_past_it() {
    let mut view = form();
    let nodes = tree(&mut view);

    let group = &nodes[0];
    assert_eq!(group.role, Role::Group);
    assert_eq!(group.label.as_deref(), Some("Welcome to Koru"));

    let offline = find(&nodes, parts::CONTINUE_OFFLINE).expect("Continue offline");
    assert_eq!(offline.role, Role::Button);
    // The label the button paints, which is not the account surface's
    // *Continue offline*.
    assert_eq!(
        offline.label.as_deref(),
        Some("Continue without an account")
    );
    assert!(offline.actions.contains(&Action::Click));
    assert!(offline.bounds.expect("a drawn rectangle").w > 0.0);

    // The account remains optional and is never a precondition: the Google
    // button is published because it is drawn, and nothing below depends on it.
    let google = find(&nodes, parts::GOOGLE).expect("Sign in with Google");
    assert_eq!(google.label.as_deref(), Some("Sign in with Google"));

    // The email form, which is now the first thing on this screen.
    let address = find(&nodes, parts::EMAIL_ADDRESS).expect("the address field");
    assert_eq!(address.role, Role::TextInput);
    let password = find(&nodes, parts::EMAIL_PASSWORD).expect("the password field");
    assert_eq!(
        password.value.as_deref(),
        Some(""),
        "an empty secret is empty"
    );
    assert_eq!(
        find(&nodes, parts::CONTINUE_WITH_EMAIL)
            .expect("the submit")
            .label
            .as_deref(),
        Some("Continue with email")
    );

    // The legal links leave Koru, and say so.
    assert_eq!(find(&nodes, parts::TERMS).expect("Terms").role, Role::Link);
    assert_eq!(
        find(&nodes, parts::PRIVACY).expect("Privacy").role,
        Role::Link
    );

    // Nothing has failed, so there is nothing to say about a failure.
    assert!(find(&nodes, parts::STATUS).is_none());
    assert!(find(&nodes, parts::RETRY).is_none());
}

/// **A sign-in that fails says so, on the screen and out loud** — Q-U1.
///
/// The welcome screen used to keep `account_failed`'s message and paint
/// nothing, so a person pressing *Sign in with Google* was returned to the
/// first screen and told nothing at all. It is now drawn in a banner under
/// the row that was pressed, and this is the same rectangle published to a
/// client.
#[test]
fn a_failed_sign_in_is_published_where_the_welcome_screen_draws_it() {
    // The one caller's message, not a message this test invented.
    let message = crate::account_ui::command_failure_surface().body;
    let mut view = form();
    let google = at(&tree(&mut view), parts::GOOGLE);
    assert!(view.accessibility_action(google, Action::Click, None));
    assert_eq!(view.take_action(), Some(FormAction::StartGoogle));
    view.account_failed(message.clone());
    assert_eq!(view.screen, Screen::Entry);

    let nodes = tree(&mut view);
    let status = find(&nodes, parts::STATUS).expect("the failure, said out loud");
    assert_eq!(status.label.as_deref(), Some(message.as_str()));
    // Assertive: the person pressed a button and is waiting on it. The
    // announcement follows the name, which for a `Role::Label` is this string.
    assert_eq!(status.live, Some(Live::Assertive));

    // **The rectangle is the one the pixels are in**, not a second derivation
    // of it: `draw_entry` hands `welcome::draw` this same `Layout`, and the
    // message is painted into `layout.notice`.
    let drawn = view.welcome_layout().notice.expect("a drawn rectangle");
    assert_eq!(status.bounds, Some(drawn));
    assert!(drawn.w > 0.0 && drawn.h > 0.0);

    // In a banner under the provider row it is about, above the no-account
    // card, and clear of the legal line.
    let layout = view.welcome_layout();
    let banner = layout.banner.expect("an attempt's failure is a banner");
    assert!(banner.contains(drawn.x, drawn.y));
    assert!(banner.y >= layout.google.y + layout.google.h);
    assert!(banner.y + banner.h <= layout.offline.y);
    assert!(layout.offline.y + layout.offline.h <= layout.terms.y);

    // The screen still works: the way in without an account is untouched.
    let offline = find(&nodes, parts::CONTINUE_OFFLINE).expect("Continue offline");
    assert!(!offline.disabled);

    // And the drawing path runs with a message set.
    view.draw(&mut Scene::new(), Affine::IDENTITY);
}

/// *Try again* repeats the attempt the banner is about, and is offered only
/// for an attempt that can be repeated.
#[test]
fn try_again_repeats_the_failed_sign_in() {
    let mut view = form();
    let apple = at(&tree(&mut view), parts::APPLE);
    assert!(view.accessibility_action(apple, Action::Click, None));
    assert_eq!(view.take_action(), Some(FormAction::StartApple));
    view.account_failed("Koru can't connect right now");

    let nodes = tree(&mut view);
    let retry = find(&nodes, parts::RETRY).expect("Try again");
    assert_eq!(retry.role, Role::Button);
    assert_eq!(retry.label.as_deref(), Some("Try again"));
    assert_eq!(retry.bounds, view.welcome_layout().retry);
    // Read after the failure it belongs to.
    assert!(
        nodes
            .iter()
            .position(|n| n.occurrence.part == parts::STATUS)
            < nodes.iter().position(|n| n.occurrence.part == parts::RETRY)
    );

    let retry = retry.occurrence;
    assert!(view.accessibility_action(retry, Action::Click, None));
    assert_eq!(view.screen, Screen::Browser);
    assert_eq!(view.take_action(), Some(FormAction::StartApple));
}

/// An email journey that ended because the password did not match points at
/// the password field, not at the attempt: the field is what to fix, and with
/// the password cleared there is nothing to try again.
#[test]
fn a_password_that_did_not_match_is_said_under_the_password_field() {
    let mut view = form();
    view.last_attempt = Some(koru_ui::welcome::Action::ContinueWithEmail);
    view.screen = Screen::EmailWorking;
    view.show_account_state(&koru_agent_wire::AccountState {
        stage: koru_agent_wire::AccountStage::Failed,
        failure_code: Some("email_already_registered".into()),
        ..Default::default()
    });
    assert_eq!(view.screen, Screen::Entry);

    let layout = view.welcome_layout();
    let drawn = layout.notice.expect("a rectangle under the field");
    assert!(layout.banner.is_none());
    assert!(layout.retry.is_none());
    assert!(drawn.y >= layout.password.y + layout.password.h);
    assert!(drawn.y + drawn.h <= layout.email_submit.y);
    let nodes = tree(&mut view);
    assert_eq!(
        find(&nodes, parts::STATUS).and_then(|n| n.bounds),
        Some(drawn)
    );
    assert!(find(&nodes, parts::RETRY).is_none());
    view.draw(&mut Scene::new(), Affine::IDENTITY);
}

/// The message goes when the person acts on it, so a second visit to the
/// welcome screen never shows a failure that is no longer current.
#[test]
fn acting_on_the_welcome_screen_clears_a_failure_it_was_showing() {
    // Continuing offline is the other action on that screen.
    let mut view = form();
    view.account_failed("Koru can't connect right now");
    let offline = at(&tree(&mut view), parts::CONTINUE_OFFLINE);
    assert!(view.accessibility_action(offline, Action::Click, None));
    assert_eq!(view.screen, Screen::Profile);
    assert!(find(&tree(&mut view), parts::STATUS).is_none());

    // Trying the sign-in again supersedes the last failure, so cancelling
    // back to the welcome screen does not bring it back.
    let mut view = form();
    view.account_failed("Koru can't connect right now");
    let google = at(&tree(&mut view), parts::GOOGLE);
    assert!(view.accessibility_action(google, Action::Click, None));
    assert_eq!(view.screen, Screen::Browser);
    assert_eq!(view.take_action(), Some(FormAction::StartGoogle));
    view.account_cancelled();
    assert_eq!(view.screen, Screen::Entry);
    let back = tree(&mut view);
    assert!(find(&back, parts::STATUS).is_none());
    assert!(view.welcome_layout().notice.is_none());
}

#[test]
fn the_profile_step_publishes_its_fields_its_purposes_and_its_actions() {
    let mut view = at_profile();
    let nodes = tree(&mut view);

    assert_eq!(
        nodes[0].label.as_deref(),
        Some("First, who's working here?")
    );
    assert_eq!(
        find(&nodes, parts::STEP).and_then(|node| node.label.clone()),
        Some("Step 1 of 2".to_owned())
    );

    let owner = find(&nodes, parts::OWNER_NAME).expect("the owner field");
    assert_eq!(owner.role, Role::TextInput);
    assert_eq!(owner.label.as_deref(), Some("Display name"));
    assert_eq!(owner.value.as_deref(), Some(""));
    // A text input is never invocable whatever it advertises, so it advertises
    // what a client can actually do with one.
    assert!(!owner.actions.contains(&Action::Click));
    assert!(owner.actions.contains(&Action::SetValue));

    let group = find(&nodes, parts::PURPOSES).expect("the purpose group");
    assert_eq!(group.role, Role::ListBox);
    // The total lives on the container, never on the item.
    assert_eq!(group.size_of_set, Some(3));
    assert_eq!(
        labels(&group.children),
        vec!["Work".to_owned(), "School".into(), "Personal".into()]
    );
    // Zero-based: the adapter adds the one a person hears.
    assert_eq!(
        group
            .children
            .iter()
            .map(|card| card.position)
            .collect::<Vec<_>>(),
        vec![Some(0), Some(1), Some(2)]
    );
    // Choosing *is* the action, so every card says whether it is chosen.
    assert!(group.children.iter().all(|c| c.selected == Some(false)));

    assert!(find(&nodes, parts::PROFILE_ICON).is_some());
    assert_eq!(
        find(&nodes, parts::PROFILE_NAME).map(|node| node.role),
        Some(Role::TextInput)
    );
    // The account-free route draws no note under the form, so it publishes
    // none.
    assert!(find(&nodes, parts::NOTE).is_none());
    assert!(find(&nodes, parts::BACK).is_some());
}

#[test]
fn the_workspace_step_publishes_six_starting_points_and_the_chosen_one() {
    let mut view = at_workspace();
    let nodes = tree(&mut view);

    assert_eq!(
        nodes[0].label.as_deref(),
        Some("Now a workspace to put things in")
    );
    assert_eq!(
        find(&nodes, parts::STEP).and_then(|node| node.label.clone()),
        Some("Step 2 of 2".to_owned())
    );

    let group = find(&nodes, parts::TEMPLATES).expect("the starting points");
    assert_eq!(group.role, Role::ListBox);
    assert_eq!(group.size_of_set, Some(6));
    assert_eq!(
        labels(&group.children),
        vec![
            "Project".to_owned(),
            "Research".into(),
            "Course".into(),
            "Recipes".into(),
            "Home".into(),
            "Empty".into()
        ]
    );
    // Nothing is chosen for the person, and the form says so.
    assert!(group.children.iter().all(|c| c.selected == Some(false)));
    // Each card carries the sentence drawn under it.
    assert!(
        group.children[0]
            .children
            .iter()
            .any(|c| c.label.as_deref() == Some("Roadmap, meeting notes, decisions"))
    );
    // Work suggests the first two, and the badge is drawn, so it is published.
    assert!(
        group.children[0]
            .children
            .iter()
            .any(|c| c.label.as_deref() == Some("Suggested"))
    );
    assert!(
        !group.children[3]
            .children
            .iter()
            .any(|c| c.label.as_deref() == Some("Suggested"))
    );

    let name = find(&nodes, parts::WORKSPACE_NAME).expect("the workspace field");
    assert_eq!(name.label.as_deref(), Some("Workspace name"));
    // The step opens with a starting point marked and the name still blank, so
    // the primary is drawn greyed and published disabled. Pressing the marked
    // card is what fills the field.
    assert_eq!(name.value.as_deref(), Some(""));
    let primary = find(&nodes, parts::PRIMARY).expect("Create workspace");
    assert_eq!(primary.label.as_deref(), Some("Create workspace"));
    assert!(primary.disabled);

    let mut ready = at_workspace_ready();
    let nodes = tree(&mut ready);
    assert_eq!(
        find(&nodes, parts::WORKSPACE_NAME).and_then(|node| node.value.clone()),
        Some("Project".to_owned())
    );
    assert!(
        !find(&nodes, parts::PRIMARY)
            .expect("Create workspace")
            .disabled
    );
}

/// **Every screen the account-free route reaches has something in it**, which
/// is the whole of what the first-run window was missing.
#[test]
fn no_screen_on_the_route_publishes_an_empty_window() {
    let (window, slot) = slots();
    let mut view = form();
    for step in ["welcome", "profile", "workspace", "finishing"] {
        let nodes = flatten(&view.accessibility_semantics(window, slot));
        assert!(nodes.len() > 3, "{step} published {} node(s)", nodes.len());
        assert!(
            nodes.iter().all(|node| node.label.is_some()),
            "{step} published a node with no accessible name"
        );
        assert!(
            step == "finishing"
                || nodes.iter().any(|node| {
                    node.actions.contains(&Action::Click)
                        || node.actions.contains(&Action::SetValue)
                }),
            "{step} published nothing a person can act on"
        );
        match step {
            "welcome" => {
                let go = at(&nodes, parts::CONTINUE_OFFLINE);
                assert!(view.accessibility_action(go, Action::Click, None));
            }
            "profile" => {
                let owner = at(&nodes, parts::OWNER_NAME);
                view.accessibility_action(owner, Action::SetValue, Some(&value("Ada")));
                let nodes = flatten(&view.accessibility_semantics(window, slot));
                let work = at(&nodes, parts::PURPOSE_WORK);
                view.accessibility_action(work, Action::Click, None);
                let nodes = flatten(&view.accessibility_semantics(window, slot));
                let go = at(&nodes, parts::PRIMARY);
                assert!(view.accessibility_action(go, Action::Click, None));
            }
            "workspace" => {
                // The step opens with a starting point marked and the name
                // blank; pressing the card is what fills it, and the product's
                // own rule is that the primary waits until it is filled.
                let project = at(&nodes, parts::TEMPLATE[0]);
                assert!(view.accessibility_action(project, Action::Click, None));
                let nodes = flatten(&view.accessibility_semantics(window, slot));
                let go = at(&nodes, parts::PRIMARY);
                assert!(view.accessibility_action(go, Action::Click, None));
                assert!(matches!(
                    view.take_action(),
                    Some(FormAction::CreateLocalProfile { .. })
                ));
            }
            _ => {}
        }
    }
}

// -------------------------------------------- validation, as it already is

/// **The product's own rule, said out loud rather than reinvented.**
///
/// `form_controls` withholds the primary until the three values are present,
/// and `draw_button_enabled` paints it greyed in the meantime. A tree that
/// omitted the button would tell a screen reader it is absent while a sighted
/// person is looking straight at it, so it is published `disabled` — which is
/// also what stops `accesskit_windows` delivering an action to it.
#[test]
fn an_incomplete_profile_publishes_a_disabled_primary_and_refuses_to_act_on_it() {
    let mut view = at_profile();
    let nodes = tree(&mut view);
    let primary = find(&nodes, parts::PRIMARY).expect("Continue");
    assert!(primary.disabled);
    assert!(!view.accessibility_action(primary.occurrence, Action::Click, None));

    // A display name alone is not enough: nothing has named the Profile yet.
    // A purpose card suggests that name; typing one does the same.
    let owner = at(&nodes, parts::OWNER_NAME);
    view.accessibility_action(owner, Action::SetValue, Some(&value("Ada")));
    let nodes = tree(&mut view);
    assert!(find(&nodes, parts::PRIMARY).expect("Continue").disabled);

    let work = at(&nodes, parts::PURPOSE_WORK);
    view.accessibility_action(work, Action::Click, None);
    let nodes = tree(&mut view);
    assert!(!find(&nodes, parts::PRIMARY).expect("Continue").disabled);
}

/// **A blank-looking name, exactly as the product already handles it.**
///
/// `TextField::value` is trimmed and `is_empty` is not, so a display name of
/// two spaces derives an empty Profile name and the form leaves *Continue*
/// unavailable. That is the product's rule and the projection reports it
/// rather than inventing a message about it. The field still reads back what
/// is actually in the box, which is why it publishes `raw_text`.
#[test]
fn a_blank_looking_name_is_handled_the_way_the_form_already_handles_it() {
    let (window, slot) = slots();
    let mut view = at_profile();
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let owner = at(&nodes, parts::OWNER_NAME);
    let work = at(&nodes, parts::PURPOSE_WORK);
    view.accessibility_action(owner, Action::SetValue, Some(&value("  ")));
    view.accessibility_action(work, Action::Click, None);
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    assert!(find(&nodes, parts::PRIMARY).expect("Continue").disabled);
    assert_eq!(
        find(&nodes, parts::OWNER_NAME).and_then(|node| node.value.clone()),
        Some("  ".to_owned())
    );
    // The derived Profile name is empty, which is why. Both are readable, so a
    // person can hear what is missing.
    assert_eq!(
        find(&nodes, parts::PROFILE_NAME).and_then(|node| node.value.clone()),
        Some(String::new())
    );

    // One real character is enough, and nothing else had to change.
    view.accessibility_action(owner, Action::SetValue, Some(&value("Ada")));
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    assert!(!find(&nodes, parts::PRIMARY).expect("Continue").disabled);
    assert_eq!(
        find(&nodes, parts::PROFILE_NAME).and_then(|node| node.value.clone()),
        Some("Ada (Work)".to_owned())
    );
}

#[test]
fn a_typed_workspace_name_locks_the_starting_points_as_disabled_rather_than_hiding_them() {
    let (window, slot) = slots();
    let mut view = at_workspace();
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let name = at(&nodes, parts::WORKSPACE_NAME);
    view.accessibility_action(name, Action::SetValue, Some(&value("Kitchen rebuild")));
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let group = find(&nodes, parts::TEMPLATES).expect("still drawn");
    assert_eq!(group.children.len(), 6);
    assert!(group.children.iter().all(|card| card.disabled));
    let card = group.children[2].occurrence;
    assert!(!view.accessibility_action(card, Action::Click, None));
    assert_eq!(
        find(&nodes, parts::WORKSPACE_NAME).and_then(|node| node.value.clone()),
        Some("Kitchen rebuild".to_owned())
    );
}

// ------------------------------------------- transitions and stale requests

/// **A screen transition retires the last screen's identities.**
#[test]
fn a_transition_mints_new_identities_and_the_old_ones_are_gone() {
    let (window, slot) = slots();
    let mut view = form();
    let welcome = flatten(&view.accessibility_semantics(window, slot));
    let offline = at(&welcome, parts::CONTINUE_OFFLINE);
    view.accessibility_action(offline, Action::Click, None);
    let profile = flatten(&view.accessibility_semantics(window, slot));

    let before: Vec<Occurrence> = welcome.iter().map(|node| node.occurrence).collect();
    let after: Vec<Occurrence> = profile.iter().map(|node| node.occurrence).collect();
    assert!(
        before.iter().all(|old| !after.contains(old)),
        "a welcome-screen identity survived into the Profile step"
    );
    // The successor's number is the next one, so a locator says which screen a
    // control belonged to.
    assert!(matches!(
        profile[0].occurrence.object,
        ObjectKey::Instance { kind: "onb", id: 2 }
    ));
}

/// **Leaving a screen and coming back is a new screen**, so a request queued
/// against the one that went cannot land on the one that replaced it.
#[test]
fn returning_to_a_step_does_not_reuse_the_identities_it_had_before() {
    let (window, slot) = slots();
    let mut view = at_workspace_ready();
    let first = flatten(&view.accessibility_semantics(window, slot));
    let back = at(&first, parts::BACK);
    view.accessibility_action(back, Action::Click, None);
    let profile = flatten(&view.accessibility_semantics(window, slot));
    let go = at(&profile, parts::PRIMARY);
    view.accessibility_action(go, Action::Click, None);
    let second = flatten(&view.accessibility_semantics(window, slot));

    let old = at(&first, parts::PRIMARY);
    let new = at(&second, parts::PRIMARY);
    assert_ne!(old, new);
    assert_ne!(old.locator(), new.locator());
    // The stale one is refused rather than routed to its replacement.
    assert!(!view.accessibility_action(old, Action::Click, None));
    assert!(view.take_action().is_none());
}

/// **The dangerous one.** A person retrying must not create two Profiles, and
/// a client replaying the request that started the first must not either.
#[test]
fn a_replayed_create_never_produces_a_second_profile() {
    let (window, slot) = slots();
    let mut view = at_workspace_ready();
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let create = at(&nodes, parts::PRIMARY);

    assert!(view.accessibility_action(create, Action::Click, None));
    assert!(matches!(
        view.take_action(),
        Some(FormAction::CreateLocalProfile { .. })
    ));

    // The same request again, before the tree has been rebuilt: the screen has
    // moved on under it, so it is refused.
    assert!(!view.accessibility_action(create, Action::Click, None));
    assert!(view.take_action().is_none());

    // And after a rebuild, the finishing screen holds a different identity, so
    // it is refused there too.
    let finishing = flatten(&view.accessibility_semantics(window, slot));
    assert!(!view.accessibility_action(create, Action::Click, None));
    assert!(view.take_action().is_none());
    assert!(
        find(&finishing, parts::PRIMARY)
            .expect("still drawn")
            .disabled,
        "the primary is drawn while creation is in flight and must not be pressable"
    );
    assert!(
        find(&finishing, parts::BACK).is_none(),
        "Back stops being drawn once creation has started"
    );
}

/// A failed creation returns to the Workspace step with everything typed still
/// there, announces itself, and offers exactly one retry.
#[test]
fn a_failed_creation_announces_itself_and_the_retry_creates_once() {
    let (window, slot) = slots();
    let mut view = at_workspace_ready();
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let name = at(&nodes, parts::WORKSPACE_NAME);
    view.accessibility_action(name, Action::SetValue, Some(&value("Kitchen rebuild")));
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let create = at(&nodes, parts::PRIMARY);
    view.accessibility_action(create, Action::Click, None);
    assert!(matches!(
        view.take_action(),
        Some(FormAction::CreateLocalProfile { .. })
    ));

    // In flight: the news is the primary's own label, and it is live, so a
    // client is told without asking.
    let finishing = flatten(&view.accessibility_semantics(window, slot));
    let primary = find(&finishing, parts::PRIMARY).expect("the primary");
    assert_eq!(
        primary.label.as_deref(),
        Some("Creating your Profile and workspace")
    );
    assert_eq!(primary.live, Some(Live::Polite));

    view.creation_failed("The Store refused the write");
    let failed = flatten(&view.accessibility_semantics(window, slot));
    let status = find(&failed, parts::STATUS).expect("the failure, said out loud");
    assert_eq!(status.label.as_deref(), Some("The Store refused the write"));
    assert_eq!(status.live, Some(Live::Assertive));
    // Nothing the person typed was lost.
    assert_eq!(
        find(&failed, parts::WORKSPACE_NAME).and_then(|node| node.value.clone()),
        Some("Kitchen rebuild".to_owned())
    );

    let retry = find(&failed, parts::PRIMARY).expect("the retry");
    assert!(!retry.disabled);
    assert!(view.accessibility_action(retry.occurrence, Action::Click, None));
    assert!(matches!(
        view.take_action(),
        Some(FormAction::CreateLocalProfile { .. })
    ));
    // One request per press, and the press before it is dead.
    assert!(!view.accessibility_action(create, Action::Click, None));
    assert!(view.take_action().is_none());
}

/// A request for a control on a screen that is no longer showing is refused
/// even when the *part* still exists on the new screen. This is the case the
/// instance number exists for: `Hit::Primary` is a control on both steps.
#[test]
fn a_stale_primary_is_refused_rather_than_routed_to_its_replacement() {
    let (window, slot) = slots();
    let mut view = at_profile();
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let owner = at(&nodes, parts::OWNER_NAME);
    view.accessibility_action(owner, Action::SetValue, Some(&value("Ada")));
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let work = at(&nodes, parts::PURPOSE_WORK);
    view.accessibility_action(work, Action::Click, None);
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let profile_primary = at(&nodes, parts::PRIMARY);

    view.accessibility_action(profile_primary, Action::Click, None);
    let workspace = flatten(&view.accessibility_semantics(window, slot));
    let workspace_primary = at(&workspace, parts::PRIMARY);
    assert_eq!(profile_primary.part, workspace_primary.part);
    assert_ne!(profile_primary, workspace_primary);

    // The Profile step's Continue, replayed. It must not press Create.
    assert!(!view.accessibility_action(profile_primary, Action::Click, None));
    assert!(view.take_action().is_none());
}

// ------------------------------------------------------------------- focus

#[test]
fn accessible_focus_is_the_control_the_focus_ring_is_on() {
    let (window, slot) = slots();
    let mut view = form();
    let _ = view.accessibility_semantics(window, slot);
    // Entry into a screen puts the keyboard on its first control, which on
    // the welcome screen is the email form's first field.
    let first = view.accessibility_focus(window, slot).expect("focus");
    assert_eq!(first.part, parts::EMAIL_ADDRESS);

    // Tab moves both, because there is only one of them.
    view.key(&WKey::Named(NamedKey::Tab), None, ModifiersState::empty());
    assert_eq!(
        view.accessibility_focus(window, slot).expect("focus").part,
        parts::EMAIL_PASSWORD
    );

    // And a client moving focus moves the ring.
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let terms = at(&nodes, parts::TERMS);
    assert!(view.accessibility_action(terms, Action::Focus, None));
    assert_eq!(view.accessibility_focus(window, slot), Some(terms));
}

/// After a transition the keyboard is on the new screen's first control, and
/// the occurrence it resolves to is one that is actually in the tree.
#[test]
fn focus_returns_to_the_new_screen_and_always_resolves_into_the_tree() {
    let (window, slot) = slots();
    let mut view = form();
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let offline = at(&nodes, parts::CONTINUE_OFFLINE);
    view.accessibility_action(offline, Action::Click, None);

    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let focus = view.accessibility_focus(window, slot).expect("focus");
    assert_eq!(focus.part, parts::OWNER_NAME);
    assert!(
        nodes.iter().any(|node| node.occurrence == focus),
        "the published focus named a control that is not in the tree"
    );
}

/// The icon-and-colour chooser owns everything under it, so nothing under it
/// is published — and the keyboard is said to be inside it.
#[test]
fn the_appearance_chooser_is_a_modal_with_nothing_published_beneath_it() {
    let (window, slot) = slots();
    let mut view = at_profile();
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let icon = at(&nodes, parts::PROFILE_ICON);
    assert!(view.accessibility_action(icon, Action::Click, None));

    let open = flatten(&view.accessibility_semantics(window, slot));
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].role, Role::Dialog);
    assert!(open[0].modal);
    assert_eq!(open[0].label.as_deref(), Some("Icon and color"));
    assert_eq!(
        view.accessibility_focus(window, slot).map(|at| at.part),
        Some(parts::APPEARANCE)
    );

    // And nothing under it routes: the tile that opened it is not in the tree
    // on screen, so a request naming it is refused rather than opening a
    // second chooser.
    assert!(!view.accessibility_action(icon, Action::Click, None));

    // Escape is the way out, and the form comes back.
    view.key(
        &WKey::Named(NamedKey::Escape),
        None,
        ModifiersState::empty(),
    );
    let back = flatten(&view.accessibility_semantics(window, slot));
    assert!(find(&back, parts::OWNER_NAME).is_some());
    // …and it routes again.
    assert!(view.accessibility_action(at(&back, parts::OWNER_NAME), Action::Focus, None));
}

// --------------------------------------------------- an action is a keystroke

/// **The same call the keyboard makes**, asserted by making both and comparing
/// the state that comes out.
#[test]
fn a_client_invoke_lands_the_form_where_enter_does() {
    let (window, slot) = slots();

    let mut typed = form();
    let _ = typed.accessibility_semantics(window, slot);
    // Tab to Continue offline, then Enter — the keyboard route. It is past
    // the email form's four controls (the password's eye is one) and the
    // Google and Apple buttons, in the reading order `form_controls` publishes.
    for _ in 0..6 {
        typed.key(&WKey::Named(NamedKey::Tab), None, ModifiersState::empty());
    }
    typed.key(&WKey::Named(NamedKey::Enter), None, ModifiersState::empty());

    let mut invoked = form();
    let nodes = flatten(&invoked.accessibility_semantics(window, slot));
    let offline = at(&nodes, parts::CONTINUE_OFFLINE);
    invoked.accessibility_action(offline, Action::Click, None);

    let shape = |view: &mut OnboardingView| {
        flatten(&view.accessibility_semantics(window, slot))
            .iter()
            .map(|node| (node.occurrence.locator(), node.label.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(shape(&mut typed), shape(&mut invoked));
}

/// Filling a field from a client leaves the form exactly where typing the same
/// text does — including the Profile-name suggestion the owner's name drives.
#[test]
fn set_value_lands_the_form_where_typing_the_same_text_does() {
    let (window, slot) = slots();

    let mut typed = at_profile();
    let _ = typed.accessibility_semantics(window, slot);
    for ch in "Ada".chars() {
        let text = ch.to_string();
        typed.key(
            &WKey::Character(text.clone().into()),
            Some(&text),
            ModifiersState::empty(),
        );
    }

    let mut set = at_profile();
    let nodes = flatten(&set.accessibility_semantics(window, slot));
    let owner = at(&nodes, parts::OWNER_NAME);
    assert!(set.accessibility_action(owner, Action::SetValue, Some(&value("Ada"))));

    let values = |view: &mut OnboardingView| {
        flatten(&view.accessibility_semantics(window, slot))
            .iter()
            .map(|node| (node.occurrence.part, node.value.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(values(&mut typed), values(&mut set));
    assert!(!values(&mut set).is_empty());
}

/// A `SetValue` with no value, or against something that is not a field, is
/// declined rather than guessed at.
#[test]
fn set_value_is_declined_where_there_is_nothing_to_set() {
    let (window, slot) = slots();
    let mut view = at_profile();
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let owner = at(&nodes, parts::OWNER_NAME);
    let icon = at(&nodes, parts::PROFILE_ICON);
    assert!(!view.accessibility_action(owner, Action::SetValue, None));
    assert!(!view.accessibility_action(icon, Action::SetValue, Some(&value("nope"))));
}

/// An action naming a control this window never published — a different object
/// kind, or a screen instance the client made up — is refused.
#[test]
fn an_action_naming_something_this_form_never_published_is_refused() {
    let (window, slot) = slots();
    let mut view = at_profile();
    let _ = view.accessibility_semantics(window, slot);
    let foreign = Occurrence::surface(window, slot, "explore", parts::PRIMARY);
    assert!(!view.accessibility_action(foreign, Action::Click, None));
    let invented = Occurrence::instance(window, slot, FORM, 99, parts::PRIMARY);
    assert!(!view.accessibility_action(invented, Action::Click, None));
    assert!(view.take_action().is_none());
}

// ---------------------------------------------------------- what is not here

/// The Google route is named and not described, deliberately: §3 of the
/// roadmap forbids pulling account and enrollment flows into minimum local
/// setup by association. Pinned here so the boundary is a decision somebody
/// took rather than an omission.
#[test]
fn the_google_route_publishes_one_named_boundary_and_nothing_else() {
    let (window, slot) = slots();
    let mut view = form();
    view.begin_browser_signin(false);
    let nodes = flatten(&view.accessibility_semantics(window, slot));
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].role, Role::Group);
    assert_eq!(nodes[0].label.as_deref(), Some("Koru account setup"));
    assert!(nodes[0].actions.is_empty());
    // And it routes nothing.
    assert!(!view.accessibility_action(nodes[0].occurrence, Action::Click, None));
}

// ----------------------------------------------------- identity and language

/// **A locator is byte-identical in every language.** Nothing in one is a
/// name, a translated word or a position.
#[test]
fn identity_is_the_same_in_every_language() {
    let (window, slot) = slots();
    let mut reference: Option<Vec<String>> = None;
    for lang in Lang::ALL {
        let mut view = at_workspace();
        view.language = lang;
        let locators: Vec<String> = flatten(&view.accessibility_semantics(window, slot))
            .iter()
            .map(|node| node.occurrence.locator())
            .collect();
        match &reference {
            None => reference = Some(locators),
            Some(first) => assert_eq!(first, &locators, "{} moved an identity", lang.tag()),
        }
    }
}

/// Every authored accessible name on the route is translated in all eight
/// languages, and the tree does not quietly fall back to English.
#[test]
fn every_authored_name_on_the_route_is_translated() {
    let (window, slot) = slots();
    let english: Vec<String> = {
        let mut view = at_workspace();
        view.language = Lang::En;
        labels(&flatten(&view.accessibility_semantics(window, slot)))
    };
    assert!(english.len() > 15);
    for lang in Lang::ALL {
        if lang == Lang::En {
            continue;
        }
        let mut view = at_workspace();
        view.language = lang;
        let names = labels(&flatten(&view.accessibility_semantics(window, slot)));
        assert_eq!(names.len(), english.len(), "{} lost a name", lang.tag());
        let shared = names.iter().zip(&english).filter(|(a, b)| a == b).count();
        // The workspace's own name is content — it is whatever the template
        // put there — and a couple of short words coincide between languages.
        // Everything else must be the translated row.
        assert!(
            shared <= 4,
            "{} left {shared} accessible names in English: {names:?}",
            lang.tag()
        );
    }
}

// ---------------------------------------------------------------- the seam

/// **"No Store, no network, no disk in a semantic callback", asserted at the
/// seam rather than with a stopwatch.**
///
/// This projection is `&mut self` — the geometry it reuses measures text — so
/// it does not get the borrow-checker proof `View::semantics` has. Two things
/// replace it. The controller owns no Store, workspace or network handle at
/// all, which is a rule about the *imports* of this file; and building a tree
/// must raise nothing, which is asserted below.
#[test]
fn the_projection_cannot_reach_the_store_the_network_or_the_disk() {
    const FORBIDDEN: [&str; 9] = [
        "koru_workspace",
        "koru_docstore",
        "koru_editor",
        "koru_store",
        "koru_projection",
        "koru_search",
        "koru_agent_wire",
        "std::fs",
        "std::io",
    ];
    let source = include_str!("../accessibility.rs");
    for line in source
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("use "))
    {
        for forbidden in FORBIDDEN {
            assert!(
                !line.contains(forbidden),
                "onboarding/accessibility.rs imports {forbidden}: a semantic query reads state \
                 that is already in memory and must not reach the Store, the network or the \
                 disk. If a projection needs more, the form prepares it before the query \
                 arrives."
            );
        }
    }
}

/// Building a tree changes nothing a person can observe: no typed action
/// leaves, no telemetry is buffered, and the answer is the same twice.
#[test]
fn building_a_tree_raises_no_action_and_no_telemetry() {
    let (window, slot) = slots();
    let mut view = at_workspace();
    let _ = view.take_action();
    let _ = view.take_telemetry();

    let first = flatten(&view.accessibility_semantics(window, slot));
    let second = flatten(&view.accessibility_semantics(window, slot));
    assert_eq!(first, second);
    assert!(view.take_action().is_none());
    assert!(view.take_telemetry().is_empty());
    assert_eq!(
        view.accessibility_focus(window, slot),
        view.accessibility_focus(window, slot)
    );
}

/// **The revision moves when the tree does, and not otherwise.** A hover and a
/// pointer move are not among the things the tree carries.
#[test]
fn the_revision_follows_what_is_published_and_nothing_that_moves_per_frame() {
    let (window, slot) = slots();
    let mut view = at_profile();
    let _ = view.accessibility_semantics(window, slot);
    let quiet = view.semantics_revision();

    view.mouse_move(500.0, 400.0);
    assert_eq!(view.semantics_revision(), quiet, "a hover rebuilt the tree");

    view.key(&WKey::Named(NamedKey::Tab), None, ModifiersState::empty());
    let focused = view.semantics_revision();
    assert_ne!(focused, quiet, "a focus move did not rebuild the tree");

    let nodes = flatten(&view.accessibility_semantics(window, slot));
    let owner = at(&nodes, parts::OWNER_NAME);
    view.accessibility_action(owner, Action::SetValue, Some(&value("Ada")));
    assert_ne!(
        view.semantics_revision(),
        focused,
        "a typed value did not rebuild the tree"
    );
}

// ------------------------------------------------------------ the part table

/// [`part_of`] and [`hit_of`] are inverses over every control of every screen,
/// which is what stops a client's request being routed to the wrong control
/// after somebody adds one.
#[test]
fn every_control_of_every_screen_round_trips_through_its_part() {
    let screens = [
        Screen::Entry,
        Screen::Profile,
        Screen::Workspace,
        Screen::Finishing,
    ];
    let hits = [
        Hit::Primary,
        Hit::Secondary,
        Hit::Terms,
        Hit::Privacy,
        Hit::ResumeAccount,
        Hit::Name,
        Hit::ProfileName,
        Hit::Purpose(Purpose::Work),
        Hit::Purpose(Purpose::School),
        Hit::Purpose(Purpose::Personal),
        Hit::ProfileAppearance,
        Hit::WorkspaceAppearance,
        Hit::Template(0),
        Hit::Template(5),
        Hit::Back,
    ];
    let mut mapped = 0;
    for screen in screens {
        let mut seen: Vec<ControlPart> = Vec::new();
        for hit in hits {
            let Some(part) = part_of(screen, hit) else {
                continue;
            };
            assert!(
                !seen.contains(&part),
                "{screen:?} maps two controls onto {part}"
            );
            seen.push(part);
            assert_eq!(
                hit_of(screen, part),
                Some(hit),
                "{screen:?}/{part} does not come back as the control it names"
            );
            mapped += 1;
        }
    }
    assert!(mapped > 20, "the table went quiet: only {mapped} controls");
    // A template index that does not exist is not a control.
    assert!(part_of(Screen::Workspace, Hit::Template(6)).is_none());
    // And the Google route routes nothing at all.
    assert!(hit_of(Screen::Browser, parts::PRIMARY).is_none());
    assert!(hit_of(Screen::ExistingProfiles, parts::PRIMARY).is_none());
}

/// **Every control the form offers the keyboard is in the tree.** A control a
/// sighted person can Tab to and a screen reader cannot find is the defect this
/// whole plan is about.
#[test]
fn every_keyboard_reachable_control_on_the_route_is_published() {
    let (window, slot) = slots();
    for mut view in [form(), at_profile(), at_workspace()] {
        let screen = view.screen;
        let published: Vec<ControlPart> = flatten(&view.accessibility_semantics(window, slot))
            .iter()
            .map(|node| node.occurrence.part)
            .collect();
        for hit in view.controls() {
            let part =
                part_of(screen, hit).unwrap_or_else(|| panic!("{hit:?} on {screen:?} has no part"));
            assert!(
                published.contains(&part),
                "{hit:?} is reachable with Tab on {screen:?} and is not in the tree"
            );
        }
    }
}

/// **A control is published where it is drawn, in every language.**
///
/// The two action buttons size themselves to the string they *paint*, and that
/// string is still an English literal while the accessible name is translated.
/// If the rectangle were measured from the translated name, a client in a
/// German session would find *Weiter* somewhere the button on screen does not
/// answer a press. So the geometry comes from the drawn literal and the name
/// from `koru-i18n`, and this is what says the two never got crossed.
#[test]
fn a_control_sits_where_it_is_drawn_whatever_language_it_is_named_in() {
    let (window, slot) = slots();
    let english: Vec<(ControlPart, Option<LogicalRect>)> = {
        let mut view = at_workspace_ready();
        view.language = Lang::En;
        flatten(&view.accessibility_semantics(window, slot))
            .iter()
            .map(|node| (node.occurrence.part, node.bounds))
            .collect()
    };
    for lang in Lang::ALL {
        let mut view = at_workspace_ready();
        view.language = lang;
        let translated: Vec<(ControlPart, Option<LogicalRect>)> =
            flatten(&view.accessibility_semantics(window, slot))
                .iter()
                .map(|node| (node.occurrence.part, node.bounds))
                .collect();
        assert_eq!(
            english,
            translated,
            "{} moved a control away from where it is drawn",
            lang.tag()
        );
    }
    // …and the names really did differ, or the assertion above proves nothing.
    let mut de = at_workspace_ready();
    de.language = Lang::De;
    let nodes = flatten(&de.accessibility_semantics(window, slot));
    assert_eq!(
        find(&nodes, parts::PRIMARY).and_then(|node| node.label.clone()),
        Some("Arbeitsbereich anlegen".to_owned())
    );
    assert_eq!(
        find(&nodes, parts::BACK).and_then(|node| node.label.clone()),
        Some("Zurück".to_owned())
    );
}

/// The password field's eye is one control two ways — a click and a
/// client's invoke — and its node says whether the password is showing.
#[test]
fn the_password_eye_reveals_and_says_so() {
    let mut view = form();
    let nodes = tree(&mut view);
    let eye = find(&nodes, parts::REVEAL_PASSWORD).expect("the eye");
    assert_eq!(eye.role, Role::Button);
    assert_eq!(eye.label.as_deref(), Some("Show password"));
    let eye = eye.occurrence;
    assert!(!view.email_password.revealed());

    // A client's invoke.
    assert!(view.accessibility_action(eye, Action::Click, None));
    assert!(view.email_password.revealed());

    // A click on it, through the field that owns it.
    let layout = view.welcome_layout();
    let rect = view
        .email_password
        .end_icon_rect(layout.password)
        .expect("drawn inside the field");
    assert!(layout.password.contains(rect.x, rect.y));
    view.mouse_down(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
    view.mouse_up();
    assert!(!view.email_password.revealed());

    // And the draw path runs revealed.
    view.email_password.press_end_icon();
    view.draw(&mut Scene::new(), Affine::IDENTITY);
}
