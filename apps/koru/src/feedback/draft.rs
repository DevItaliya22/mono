// SPDX-License-Identifier: MPL-2.0

//! Process-scoped in-memory draft. Closing Feedback and opening it again in
//! this session restores what was being written. Nothing is written to disk.

use super::{Attachment, Focus, Frequency, Importance, Kind};

#[derive(Clone, Debug, Default)]
pub struct Draft {
    pub kind: Kind,
    pub message: String,
    pub expected: String,
    pub frequency: Option<Frequency>,
    pub importance: Option<Importance>,
    pub include_logs: bool,
    pub more_open: bool,
    pub contact: bool,
    pub contact_email: String,
    pub attachments: Vec<Attachment>,
    pub focus: Focus,
}

#[cfg(not(test))]
mod slot {
    use super::Draft;
    use std::sync::Mutex;

    static DRAFT: Mutex<Option<Draft>> = Mutex::new(None);

    pub fn load() -> Draft {
        DRAFT
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .unwrap_or_default()
    }

    pub fn save(draft: Draft) {
        *DRAFT.lock().unwrap_or_else(|error| error.into_inner()) = Some(draft);
    }

    pub fn clear() {
        *DRAFT.lock().unwrap_or_else(|error| error.into_inner()) = None;
    }
}

#[cfg(test)]
mod slot {
    use super::Draft;
    use std::cell::RefCell;

    thread_local! {
        static DRAFT: RefCell<Option<Draft>> = const { RefCell::new(None) };
    }

    pub fn load() -> Draft {
        DRAFT.with(|slot| slot.borrow().clone().unwrap_or_default())
    }

    pub fn save(draft: Draft) {
        DRAFT.with(|slot| *slot.borrow_mut() = Some(draft));
    }

    pub fn clear() {
        DRAFT.with(|slot| *slot.borrow_mut() = None);
    }
}

pub fn load() -> Draft {
    slot::load()
}

pub fn save(draft: Draft) {
    slot::save(draft);
}

pub fn clear() {
    slot::clear();
}

#[cfg(test)]
pub fn reset() {
    clear();
}
