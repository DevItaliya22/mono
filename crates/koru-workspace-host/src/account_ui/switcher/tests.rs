// SPDX-License-Identifier: MPL-2.0

use super::*;
use koru_agent_wire::{AccountHeldAccount, AccountStage};
fn state() -> AccountState {
    AccountState {
        active_scope: 1,
        capability: true,
        held_accounts: (1..=20)
            .map(|scope| AccountHeldAccount {
                scope_id: scope,
                account_id: format!("10000000-0000-4000-8000-{scope:012}"),
                auth_identity_id: None,
                stage: AccountStage::SignedIn,
                credential_ready: scope % 2 == 0,
                failure_code: None,
                active_generations: if scope % 3 == 0 { vec![scope] } else { vec![] },
            })
            .collect(),
        ..Default::default()
    }
}
#[test]
fn keyboard_scroll_and_hit_targets_choose_the_same_scoped_account_at_minimum_size() {
    let state = state();
    for lang in Lang::ALL {
        let mut picker = AccountSwitcher {
            open: true,
            ..Default::default()
        };
        for _ in 0..19 {
            picker.key(
                &state,
                lang,
                640.0,
                &Key::Named(NamedKey::Tab),
                ModifiersState::empty(),
            );
        }
        assert!(picker.first > 0);
        let rect = AccountSwitcher::row(452.0, picker.focus - picker.first);
        assert!(rect.x >= 0.0 && rect.x + rect.w <= 452.0 && rect.y + rect.h <= 640.0);
        let clicked = picker.click(
            &state,
            lang,
            (452.0, 640.0),
            rect.x + rect.w / 2.0,
            rect.y + rect.h / 2.0,
        );
        assert_eq!(
            clicked,
            Some(AccountCommand::SwitchAccount {
                account_id: state.held_accounts[19].account_id.clone()
            })
        );
        assert!(!picker.open);
        picker.open = true;
        let mut scene = Scene::new();
        picker.draw(
            &mut scene,
            Affine::IDENTITY,
            &mut TextEngine::new(),
            &state,
            lang,
            (452.0, 640.0),
        );
        assert_eq!(
            picker.key(
                &state,
                lang,
                640.0,
                &Key::Named(NamedKey::Escape),
                ModifiersState::empty()
            ),
            None
        );
        assert!(!picker.open);
    }
}
#[test]
fn add_is_explicit_and_current_row_never_restarts_provider() {
    let state = state();
    let mut picker = AccountSwitcher {
        open: true,
        ..Default::default()
    };
    assert_eq!(
        picker.key(
            &state,
            Lang::En,
            640.0,
            &Key::Named(NamedKey::Enter),
            ModifiersState::empty()
        ),
        None
    );
    picker.open = true;
    picker.focus = state.held_accounts.len();
    assert_eq!(
        picker.key(
            &state,
            Lang::En,
            640.0,
            &Key::Named(NamedKey::Enter),
            ModifiersState::empty()
        ),
        Some(AccountCommand::AddAccount {
            locale: "en".into()
        })
    );
}
#[test]
fn unavailable_capability_keeps_account_commands_disabled() {
    let mut state = state();
    state.capability = false;
    let mut picker = AccountSwitcher {
        open: true,
        focus: state.held_accounts.len(),
        first: 0,
    };
    assert_eq!(
        picker.key(
            &state,
            Lang::En,
            640.0,
            &Key::Named(NamedKey::Enter),
            ModifiersState::empty()
        ),
        None
    );
    assert!(picker.open);
    picker.focus = 1;
    assert_eq!(
        picker.key(
            &state,
            Lang::En,
            640.0,
            &Key::Named(NamedKey::Enter),
            ModifiersState::empty()
        ),
        None
    );
}
#[test]
fn wrapped_status_copy_fits_each_real_row_in_every_locale() {
    for lang in Lang::ALL {
        let mut engine = TextEngine::new();
        let rect = AccountSwitcher::row(452.0, 0);
        for status in [
            I18nKey::AccountSwitcherActive,
            I18nKey::AccountSwitcherPending,
            I18nKey::AccountSwitcherRepair,
        ] {
            let label = format!(
                "{} · {}",
                t(lang, I18nKey::AccountSwitcherLabel).replace("{}", "abcdef12"),
                t(lang, status)
            );
            let layout = engine.simple_layout_wrapped(&label, theme::role::meta(), rect.w - 20.0);
            assert!(layout.width() <= rect.w - 20.0 + 0.5, "{lang:?}");
            assert!(
                layout.height() + 10.0 <= rect.h,
                "{lang:?}: {}",
                layout.height()
            );
        }
    }
}
