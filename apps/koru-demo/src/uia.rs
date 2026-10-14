// SPDX-License-Identifier: MPL-2.0

//! **The apartment. Everything COM in this crate is in this file.**
//!
//! It is a [`Mechanism`](crate::supervise::Mechanism): it is constructed on the
//! supervisor's worker thread, it answers [`Request`]s with [`Response`]s, and
//! it holds every element behind a [`Handle`] so that nothing that is not `Send`
//! ever leaves. The point of that shape is in `supervise.rs`; what is here is
//! the part that talks to Windows.
//!
//! Five things it does that are not obvious from the API, each established
//! elsewhere and cited rather than rediscovered:
//!
//! - **MTA, and the client initialises COM while the provider does not.**
//!   `COINIT_MULTITHREADED`, paired with `CoUninitialize` on the way out —
//!   the arrangement `accesskit_windows`' own suite uses and the one a real
//!   screen reader creates. `guide/contributing/deps/ACCESSKIT.md`.
//! - **`CUIAutomation8`**, the class every interactive proof in the tree
//!   creates, bound into the base `IUIAutomation`.
//! - **The first `ElementFromHandle` answers a placeholder.** Koru's activation
//!   handler returns `None` until the window loop's next pass, so
//!   `accesskit_windows` serves one `Role::Window` node with `NodeId(0)` and no
//!   `author_id`. A client that reads a property straight after binding reads
//!   *that*. So [`Mechanism::perform`] polls, re-calling `ElementFromHandle`
//!   each pass, until the id starts with `koru:`. `guide/contributing/deps/ACCESSKIT.md` —
//!   and a test that asked once would be flaky and would look like a mapping
//!   bug.
//! - **`UIA_E_ELEMENTNOTAVAILABLE` is the element being gone; every other
//!   failure is unavailable and fails closed.** The split
//!   `crates/koru-windows-input/src/windows.rs` makes and
//!   `guide/contributing/deps/WIN32.md` records. Collapsing them turns *this control was
//!   destroyed* into *try again*, and a runner then waits out a full timeout on
//!   a window that closed a second ago.
//! - **An exact locator is one condition; a wildcard is a scan and a filter.**
//!   UIA has no prefix match on a property, so there is no condition that
//!   expresses `koru:w*/...`. Nothing else in this tree uses `FindAll` — this is
//!   the first — and it is `O(the subtree)`, which over a thousand-paragraph
//!   document is real. Binding the window number from the root before searching
//!   is what keeps the ordinary case off that path; see
//!   [`crate::locator::LocatorPattern::bound_to_window`].
//! - **The scope is `TreeScope_Subtree`, not `TreeScope_Descendants`.** An
//!   element is not a descendant of itself, so under `Descendants` the window
//!   root can never match its own `koru:w<n>/v0/s:window#window` locator —
//!   measured, and it reads as *no control has that locator* for the one
//!   control you are most certain exists. The four interactive proofs in this
//!   tree all use `Descendants` and none of them hit it, because each searches
//!   for something strictly inside the window it started from. A runner is
//!   asked *which control does this locator name*, and the window is a valid
//!   answer.
//!
//! ## Ordinary facts carry no name
//!
//! Names and text are compared and dropped in ordinary assertions. Neither is
//! stored in [`ElementFacts`]. Explicit fixture snapshots return a separate
//! redacted-Debug content type after bound-window and owner checks.

mod actions;
mod snapshot;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use koru_windows_input::AutomationClient;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, TreeScope_Subtree, UIA_AutomationIdPropertyId, UIA_ControlTypePropertyId,
    UIA_E_ELEMENTNOTAVAILABLE,
};

use crate::inspect::{ElementFacts, Handle, InspectError, Rect, Request, Response, Role};
use crate::supervise::Mechanism;

/// How long to wait for a window to swap its placeholder tree for a real one.
///
/// Twenty seconds, the budget every interactive proof in the tree uses, and for
/// the reason each of them records: *the first `WM_GETOBJECT` starts UI
/// Automation itself on this machine.*
const BIND_LIMIT: Duration = Duration::from_secs(20);
const BIND_INTERVAL: Duration = Duration::from_millis(50);

/// A live UI Automation client, and the elements it is holding.
pub struct Client {
    generation: u64,
    owner: Option<crate::input_owner::InputOwner>,
    desktop: Option<koru_windows_input::OrdinaryDesktop>,
    next_slot: u64,
    bound_root: Option<Handle>,
    elements: HashMap<u64, IUIAutomationElement>,
    // Elements release before the client uninitializes COM on this worker.
    client: AutomationClient,
    _dpi: koru_windows_input::dpi::PhysicalCoordinates,
}

impl Client {
    /// Open an apartment. **Call this on the thread that will use it.**
    pub fn open(generation: u64) -> Result<Self, String> {
        let dpi = koru_windows_input::dpi::PhysicalCoordinates::enter()
            .map_err(|_| "dpi-context-unavailable".to_owned())?;
        let client = AutomationClient::open_v8()
            .map_err(|error| format!("no UI Automation client: {error}"))?;
        Ok(Self {
            generation,
            owner: None,
            desktop: koru_windows_input::OrdinaryDesktop::capture().ok(),
            next_slot: 0,
            bound_root: None,
            elements: HashMap::new(),
            client,
            _dpi: dpi,
        })
    }

    pub fn with_owner(mut self, owner: crate::input_owner::InputOwner) -> Self {
        self.owner = Some(owner);
        self
    }

    fn keep(&mut self, element: IUIAutomationElement) -> Handle {
        self.next_slot += 1;
        let handle = Handle {
            generation: self.generation,
            slot: self.next_slot,
        };
        self.elements.insert(handle.slot, element);
        handle
    }

    fn element(&self, handle: Handle) -> Result<&IUIAutomationElement, InspectError> {
        if handle.generation != self.generation {
            return Err(InspectError::StaleHandle);
        }
        self.elements
            .get(&handle.slot)
            .ok_or(InspectError::StaleHandle)
    }

    fn facts(&mut self, element: IUIAutomationElement) -> Result<ElementFacts, InspectError> {
        let locator = read(unsafe { element.CurrentAutomationId() })?.to_string();
        let role = Role::from_control_type(read(unsafe { element.CurrentControlType() })?.0);
        let enabled = read(unsafe { element.CurrentIsEnabled() })?.as_bool();
        let offscreen = read(unsafe { element.CurrentIsOffscreen() })?.as_bool();
        let rectangle = read(unsafe { element.CurrentBoundingRectangle() })?;
        let process_id = read(unsafe { element.CurrentProcessId() })?;
        Ok(ElementFacts {
            handle: self.keep(element),
            locator: non_empty(locator),
            role,
            enabled,
            offscreen,
            bounds: Some(Rect {
                left: rectangle.left,
                top: rectangle.top,
                right: rectangle.right,
                bottom: rectangle.bottom,
            }),
            process_id: u32::try_from(process_id).ok().filter(|pid| *pid != 0),
        })
    }

    fn bind(&mut self, native_handle: i64) -> Response {
        let window = HWND(native_handle as *mut core::ffi::c_void);
        let deadline = Instant::now() + BIND_LIMIT;
        loop {
            // Re-called every pass, not held: the placeholder element and the
            // real root are different objects, so keeping the first one would
            // poll a node that is never going to change.
            let element = match unsafe { self.client.automation().ElementFromHandle(window) } {
                Ok(element) => element,
                Err(error) => return Response::Failed(classify(&error)),
            };
            let id = read(unsafe { element.CurrentAutomationId() })
                .map(|id| id.to_string())
                .unwrap_or_default();
            if id.starts_with(crate::locator::PREFIX) {
                let class_name = read(unsafe { element.CurrentClassName() })
                    .map(|name| name.to_string())
                    .ok()
                    .and_then(non_empty);
                let framework = read(unsafe { element.CurrentFrameworkId() })
                    .map(|name| name.to_string())
                    .ok()
                    .and_then(non_empty);
                return match self.facts(element) {
                    Ok(facts) => {
                        self.bound_root = Some(facts.handle);
                        Response::Bound(Box::new(crate::inspect::BoundWindow {
                            facts,
                            class_name,
                            framework,
                        }))
                    }
                    Err(error) => Response::Failed(error),
                };
            }
            if Instant::now() >= deadline {
                return Response::Failed(InspectError::NotKoru);
            }
            std::thread::sleep(BIND_INTERVAL);
        }
    }

    fn matches(&mut self, root: Handle, pattern: &crate::locator::LocatorPattern) -> Response {
        let root = match self.element(root) {
            Ok(root) => root.clone(),
            Err(error) => return Response::Failed(error),
        };

        // The fast path: a fully specified locator is one property condition.
        if let Some(exact) = pattern.exact() {
            let condition = match unsafe {
                self.client.automation().CreatePropertyCondition(
                    UIA_AutomationIdPropertyId,
                    &VARIANT::from(exact.as_str()),
                )
            } {
                Ok(condition) => condition,
                Err(error) => return Response::Failed(classify(&error)),
            };
            // `FindAll` rather than `FindFirst` even here, because *how many*
            // is the answer this runner needs. `FindFirst` would make the
            // ambiguity rule unenforceable in the one case it matters most.
            let found = match unsafe { root.FindAll(TreeScope_Subtree, &condition) } {
                Ok(found) => found,
                Err(error) => return Response::Failed(classify(&error)),
            };
            return self.collect(found, |_| true);
        }

        // The scan. See the module note on what it costs.
        let condition = match unsafe { self.client.automation().CreateTrueCondition() } {
            Ok(condition) => condition,
            Err(error) => return Response::Failed(classify(&error)),
        };
        let found = match unsafe { root.FindAll(TreeScope_Subtree, &condition) } {
            Ok(found) => found,
            Err(error) => return Response::Failed(classify(&error)),
        };
        let pattern = pattern.clone();
        self.collect(found, move |facts| {
            facts
                .locator
                .as_deref()
                .is_some_and(|locator| pattern.matches(locator))
        })
    }

    fn matches_by_name(&mut self, root: Handle, role: Role, name: &str) -> Response {
        let root = match self.element(root) {
            Ok(root) => root.clone(),
            Err(error) => return Response::Failed(error),
        };
        let condition = match unsafe {
            self.client.automation().CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &VARIANT::from(role.control_type()),
            )
        } {
            Ok(condition) => condition,
            Err(error) => return Response::Failed(classify(&error)),
        };
        let found = match unsafe { root.FindAll(TreeScope_Subtree, &condition) } {
            Ok(found) => found,
            Err(error) => return Response::Failed(classify(&error)),
        };

        // The one place a name is read. It is compared and dropped; nothing
        // downstream has anywhere to put it.
        let length = unsafe { found.Length() }.unwrap_or_default();
        let mut matched = Vec::new();
        for index in 0..length {
            let Ok(element) = (unsafe { found.GetElement(index) }) else {
                continue;
            };
            let this_name = read(unsafe { element.CurrentName() })
                .map(|name| name.to_string())
                .unwrap_or_default();
            if this_name != name {
                continue;
            }
            match self.facts(element) {
                Ok(facts) => matched.push(facts),
                // One element that has gone mid-walk is not the whole search
                // failing: the tree is live and a control disappearing while it
                // is being enumerated is ordinary.
                Err(InspectError::ElementGone) => continue,
                Err(error) => return Response::Failed(error),
            }
        }
        Response::Found(matched)
    }

    fn collect(
        &mut self,
        found: windows::Win32::UI::Accessibility::IUIAutomationElementArray,
        keep: impl Fn(&ElementFacts) -> bool,
    ) -> Response {
        let length = unsafe { found.Length() }.unwrap_or_default();
        let mut matched = Vec::new();
        for index in 0..length {
            let Ok(element) = (unsafe { found.GetElement(index) }) else {
                continue;
            };
            match self.facts(element) {
                Ok(facts) => {
                    if keep(&facts) {
                        matched.push(facts);
                    } else {
                        // Nothing kept a handle to it, so let it go rather than
                        // filling the table with the whole subtree.
                        self.elements.remove(&facts.handle.slot);
                    }
                }
                Err(InspectError::ElementGone) => continue,
                Err(error) => return Response::Failed(error),
            }
        }
        Response::Found(matched)
    }
}

impl Mechanism for Client {
    fn perform(&mut self, request: Request) -> Response {
        let permit = koru_windows_input::DeliveryPermit::default();
        permit.revoke();
        self.perform_guarded(request, &permit)
    }
    fn perform_guarded(
        &mut self,
        request: Request,
        permit: &koru_windows_input::DeliveryPermit,
    ) -> Response {
        match request {
            Request::Input(input) => self.input(input, permit),
            Request::FixtureChildren {
                root,
                part,
                max_nodes,
            } => match self.fixture_children(root, &part, max_nodes) {
                Ok(found) => Response::Found(found),
                Err(error) => Response::Failed(error),
            },
            Request::Snapshot {
                root,
                element,
                max_utf16,
            } => match self.snapshot(root, element, max_utf16) {
                Ok(snapshot) => Response::Snapshot(snapshot),
                Err(error) => Response::Failed(error),
            },
            Request::Check { element, condition } => match self.check(element, condition) {
                Ok(result) => Response::Checked(result),
                Err(error) => Response::Failed(error),
            },
            Request::MatchesPart { root, part } => self.matches_part(root, &part),
            Request::Focused { root } => self.focused_in(root),
            Request::BindWindow { native_handle } => self.bind(native_handle),
            Request::Matches { root, pattern } => self.matches(root, &pattern),
            Request::MatchesByName {
                root, role, name, ..
            } => self.matches_by_name(root, role, &name),
            Request::Facts { element } => match self.element(element) {
                Ok(element) => {
                    let element = element.clone();
                    match self.facts(element) {
                        Ok(facts) => Response::Facts(facts),
                        Err(error) => Response::Failed(error),
                    }
                }
                Err(error) => Response::Failed(error),
            },
        }
    }
}

/// The one classification, in one place.
fn read<T>(result: windows::core::Result<T>) -> Result<T, InspectError> {
    result.map_err(|error| classify(&error))
}

fn classify(error: &windows::core::Error) -> InspectError {
    if error.code().0 as u32 == UIA_E_ELEMENTNOTAVAILABLE {
        InspectError::ElementGone
    } else {
        InspectError::Unavailable(error.code().0.to_string())
    }
}

/// An element with no `author_id` answers the empty string, not an error.
fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}
