// SPDX-License-Identifier: MPL-2.0

//! The finite input batch and its owned-release accounting; no OS calls.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Virtual(u16),
    Unicode(u16),
    LeftMouse,
    RightMouse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub key: Key,
    pub up: bool,
}

pub fn chord(keys: &[u16]) -> Vec<Event> {
    keys.iter()
        .map(|&key| Event {
            key: Key::Virtual(key),
            up: false,
        })
        .chain(keys.iter().rev().map(|&key| Event {
            key: Key::Virtual(key),
            up: true,
        }))
        .collect()
}

pub fn text(text: &str) -> Vec<Event> {
    text.encode_utf16()
        .flat_map(|unit| {
            [
                Event {
                    key: Key::Unicode(unit),
                    up: false,
                },
                Event {
                    key: Key::Unicode(unit),
                    up: true,
                },
            ]
        })
        .collect()
}

pub fn click(right: bool) -> Vec<Event> {
    let key = if right {
        Key::RightMouse
    } else {
        Key::LeftMouse
    };
    vec![Event { key, up: false }, Event { key, up: true }]
}

/// Only accepted downs belong to this runner. Release in reverse order.
pub fn pending_releases(events: &[Event], accepted: usize) -> Vec<Event> {
    let mut held = Vec::new();
    for event in events.iter().take(accepted) {
        if event.up {
            held.retain(|key| key != &event.key);
        } else if !held.contains(&event.key) {
            held.push(event.key);
        }
    }
    held.into_iter()
        .rev()
        .map(|key| Event { key, up: true })
        .collect()
}

#[cfg(test)]
mod tests;
