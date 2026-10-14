// SPDX-License-Identifier: MPL-2.0

use super::*;

fn menu() -> CommandMenu {
    CommandMenu::opened(
        &mut TextEngine::new(),
        crate::trigger::Trigger::at_start(),
        Allows {
            image: true,
            image_offline: false,
            divider: true,
            quote: true,
            headings: true,
            code: true,
            table: true,
            tasks: true,
            blocks: true,
            pills: true,
            attrs: true,
            task_capable: true,
            task: false,
            voice: true,
        },
    )
}

#[test]
fn identity_survives_filtering_but_never_another_opening() {
    let first = menu();
    let filtered = first.refilter(first.trigger.clone(), "task").unwrap();
    assert_eq!(first.instance, filtered.instance);
    assert_ne!(first.instance, menu().instance);
    let names: std::collections::HashSet<_> = Command::ALL.iter().map(|c| c.name()).collect();
    assert_eq!(
        names.len(),
        Command::ALL.len(),
        "result identities must be unique"
    );
}

#[test]
fn scrolled_visible_geometry_is_the_owners_pointer_hit_and_activation() {
    let mut menu = menu();
    menu.scroll_rows(3);
    assert!(menu.first_visible() > 0);
    for (command, rect) in menu.visible_controls(340.0) {
        let local_y = rect.y + rect.h * 0.5 - theme::PANEL_PAD;
        let index = menu.item_at(local_y).unwrap();
        assert_eq!(menu.hits[index], command);
        if disabled(command, &menu.allows).is_some() {
            assert!(matches!(menu.click(local_y), Outcome::Consumed));
            assert!(menu.invoke_visible(command.name()).is_none());
            continue;
        }
        assert!(matches!(menu.click(local_y), Outcome::Become(c) if c == command));
        assert!(
            matches!(menu.invoke_visible(command.name()), Some(Outcome::Become(c)) if c == command)
        );
    }
    assert!(menu.invoke_visible(menu.hits[0].name()).is_none());
}

#[test]
fn filtered_missing_and_disabled_commands_do_not_activate() {
    let mut menu = menu();
    menu.hits = vec![Command::Task];
    assert!(menu.invoke_visible("image").is_none());
    menu.allows.task_capable = false;
    assert!(menu.invoke_visible("task").is_none());
    menu.allows.task_capable = true;
    assert!(matches!(
        menu.invoke_visible("task"),
        Some(Outcome::Become(Command::Task))
    ));
}
