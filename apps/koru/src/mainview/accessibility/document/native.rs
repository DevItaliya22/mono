// SPDX-License-Identifier: MPL-2.0

//! **Plan 1b.3's gate: does the *shipping window* hand a real document to a
//! real screen reader?**
//!
//! Everything else about this projection is asserted in `cargo test --workspace`
//! against a client model built out of the same `TreeUpdate`s the host
//! publishes. What that cannot establish is the thing the plan gates on: *does
//! a Windows UI Automation client — the same API Narrator and NVDA use — find a
//! paragraph of a document open in the container's own window, read its text
//! through the Text pattern, select a range, have real typing replace exactly
//! that range, undo it, and receive the native change events while it happens?*
//!
//! So this stands outside the process boundary the way a screen reader does: it
//! creates a UIA **client** in the COM multi-threaded apartment, reaches a real
//! window through `ElementFromHandle`, and asks only questions a client can ask.
//!
//! ## What it hosts, and how it differs from Plan 1b.1's proof
//!
//! `apps/koru-document/tests/accessibility_text_native.rs` hosts a bare
//! `koru_editor::Editor` and its own hand-written projection: it proved the
//! *text capability*. This hosts a real [`MainView`] — the container, its tab
//! strip, its explore column, its save status and its document — published
//! through the real `View::semantics`, the real `PaneAccessibility` and the real
//! `projection::build`, with the real `Scope::Delta` path running. There is no
//! second projection anywhere in it.
//!
//! It lives inside the binary because `apps/koru` is a `[[bin]]`: no
//! integration test can depend on it, so the only place a proof can host the
//! shipping container is a `#[cfg(test)]` module beside the code.
//!
//! ## Running it
//!
//! ```text
//! cargo test -p koru --bin koru accessibility::document::native -- --ignored --nocapture
//! ```
//!
//! It opens a real 1200×760 window for a few seconds. A separate lane, invoked
//! deliberately, and the window is created `with_active(false)` so it does not
//! take the foreground from whoever is running it.
//!
//! ## What it proves, and what it deliberately does not
//!
//! Proven: the container's window answers a client at all; a paragraph of the
//! open document is findable by `AutomationId` from a locator built out of the
//! window number, the view slot and the model node — nothing read out of the
//! tree; its exact text arrives through `ITextRangeProvider::GetText`; a
//! `Select()` on a range reaches the real editor; real typing replaces exactly
//! that range; `Ctrl+Z` puts it back; a real `IUIAutomationEventHandler`
//! receives text and selection notifications while it happens; and the window
//! published differences rather than whole trees while doing it.
//!
//! Not proven, and not provable here: what Narrator and NVDA *say*, which is a
//! judgement made by a person. That is `docs/features/accessibility/README.md`.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use koru_core::NodeId as KoruNodeId;
use koru_core::node::Node as CoreNode;
use koru_core::schema::Parent;
use koru_draw::geometry::Rect;
use koru_window::{AccessibilityProvider, WindowBridge};
use koru_workspace::Workspace;
use koru_workspace_host::View;
use koru_workspace_host::accessibility::{
    ControlPart, Occurrence, PaneAccessibility, Scope, Slots, ViewSlot, WindowSemantics, WindowSlot,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState};
use winit::platform::windows::EventLoopBuilderExtWindows;
use winit::window::{Window, WindowId};
// `#[implement]` expands to `windows_core::` paths, so the crate has to be in
// scope by its own name even though the macro arrives through `windows::core`.
use windows_core as _;

use super::{BODY, body_rows_of, editor_of, paragraph_occurrence, put_caret_in};
use crate::mainview::MainView;
use crate::mainview::accessibility::Shell;

/// How long the client waits for the provider to answer. Generous: the first
/// `WM_GETOBJECT` starts UI Automation itself.
const TIMEOUT: Duration = Duration::from_secs(20);

const WINDOW_W: f32 = 1200.0;
const WINDOW_H: f32 = 760.0;

// The fixture is synthetic and stays that way: §7 forbids real document text in
// anything automated.
const FIRST: &str = "This first paragraph is deliberately long enough that it has to wrap onto \
                     more than one visual line at the width this window uses.";
const SECOND: &str = "The second paragraph.";
const EMPTY: &str = "";

// ------------------------------------------------------- the two threads

/// What the client asks the window's loop to do. Every one is a real input.
#[derive(Clone, Debug)]
enum Ask {
    Press {
        text: Option<String>,
        mods: ModifiersState,
    },
    State,
    Finish,
}

#[derive(Debug)]
enum Wake {
    /// A screen reader left a note on the window's mailbox.
    Accessibility,
    Ask(Ask),
}

/// What the container thinks, sent back after the tree has been republished, so
/// a client with an answer is looking at a tree that already reflects it.
#[derive(Clone, Debug, Default)]
struct ShellState {
    caret: (usize, usize),
    focus_locator: Option<String>,
    /// Trees built, and how many of them were differences. The ratio §5 asks
    /// for: activation may be O(the document), and everything after it may not.
    rebuilds: u64,
    deltas: u64,
    nodes_published: u64,
    actions_routed: u64,
    actions_refused: u64,
    unresolved_focus: u64,
    /// `parley::build_nodes` calls — the O(clusters) work.
    shapings: u64,
}

/// What the client needs to talk to this window.
#[derive(Clone, Debug)]
struct Ready {
    hwnd: isize,
    body: String,
    paragraphs: Vec<String>,
}

// ------------------------------------------------------------ the window

struct Harness {
    view: MainView,
    workspace: Workspace,
    rows: Vec<KoruNodeId>,
    window_slot: WindowSlot,
    view_slot: ViewSlot,
    pane: Option<PaneAccessibility>,
    window: Option<Arc<Window>>,
    proxy: EventLoopProxy<Wake>,
    ready_tx: Option<Sender<Ready>>,
    state_tx: Sender<ShellState>,
    owes_reply: bool,
    finished: bool,
}

impl Harness {
    fn new(
        proxy: EventLoopProxy<Wake>,
        state_tx: Sender<ShellState>,
        ready_tx: Sender<Ready>,
    ) -> Self {
        let (mut view, mut workspace) = crate::mainview::harness::container();
        let document = workspace
            .ws
            .insert(
                Parent::Root,
                CoreNode::document("Accessible document").unwrap(),
            )
            .expect("the document node");
        for text in [FIRST, SECOND, EMPTY] {
            workspace
                .ws
                .insert(Parent::Node(document), CoreNode::paragraph(text).unwrap())
                .expect("a paragraph");
        }
        workspace.ws.enable_undo();

        let mut slots = Slots::default();
        let window_slot = slots.window();
        let view_slot = slots.view();
        View::set_viewport(&mut view, WINDOW_W, WINDOW_H);
        view.seat_accessibility(window_slot);
        view.open_node(&workspace.ws, document);

        let rows = body_rows_of(&view);
        // Land the caret in the first body paragraph, the way a person opening
        // a document does.
        put_caret_in(&mut view, rows[0]);

        Self {
            view,
            workspace,
            rows,
            window_slot,
            view_slot,
            pane: None,
            window: None,
            proxy,
            ready_tx: Some(ready_tx),
            state_tx,
            owes_reply: false,
            finished: false,
        }
    }

    fn shell(&self) -> Shell {
        Shell {
            window: self.window_slot,
            view: self.view_slot,
            viewport: (WINDOW_W, WINDOW_H),
            lang: koru_workspace_host::accessibility::ui_lang(),
        }
    }

    fn locator(&self, row: KoruNodeId) -> String {
        paragraph_occurrence(&self.shell(), row).locator()
    }

    /// The window's semantics, assembled exactly as `App::window_semantics`
    /// assembles them. Nothing here is a second projection.
    fn semantics(&self, scope: Scope) -> WindowSemantics {
        let window = self.window.as_ref().expect("a window");
        let scale = window.scale_factor();
        let size = window.inner_size();
        let mut children = if scope == Scope::Whole {
            koru_workspace_host::accessibility::window_commands(
                self.window_slot,
                window.is_maximized(),
                koru_workspace_host::accessibility::ui_lang(),
                View::window_command_bounds(&self.view),
            )
        } else {
            Vec::new()
        };
        children.extend(View::semantics(&self.view, self.view_slot, scope));
        WindowSemantics {
            window: self.window_slot,
            label: "Koru shell accessibility proof".to_string(),
            scale,
            client: Rect::new(
                0.0,
                0.0,
                size.width as f32 / scale as f32,
                size.height as f32 / scale as f32,
            ),
            focus: View::accessibility_focus(&self.view, self.view_slot),
            children,
            scope,
            retired: match scope {
                Scope::Whole => Vec::new(),
                Scope::Delta => View::accessibility_retired(&self.view),
            },
        }
    }

    fn state(&self) -> ShellState {
        let counters = self
            .pane
            .as_ref()
            .map(PaneAccessibility::counters)
            .unwrap_or_default();
        let editor = editor_of(&self.view).expect("a document tab");
        let row = editor.body.rows.get(editor.body.focus);
        ShellState {
            caret: row
                .map(|row| {
                    (
                        row.selection.anchor().index().min(row.text.len()),
                        row.caret(),
                    )
                })
                .unwrap_or_default(),
            focus_locator: View::accessibility_focus(&self.view, self.view_slot)
                .map(|at| at.locator()),
            rebuilds: counters.rebuilds,
            deltas: counters.deltas,
            nodes_published: counters.nodes_published,
            actions_routed: counters.actions_routed,
            actions_refused: counters.actions_refused,
            unresolved_focus: counters.unresolved_focus,
            shapings: self.view.document_counters().shapings,
        }
    }

    fn apply(&mut self, ask: Ask) {
        match ask {
            Ask::Press { text, mods } => {
                let text = text.unwrap_or_default();
                let key = Key::Character(text.as_str().into());
                let typed = (!mods.control_key()).then_some(text.as_str());
                View::key(&mut self.view, &mut self.workspace.ws, &key, typed, mods);
            }
            Ask::State => {}
            Ask::Finish => self.finished = true,
        }
        self.owes_reply = true;
    }
}

impl ApplicationHandler<Wake> for Harness {
    fn resumed(&mut self, events: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Koru shell accessibility proof")
            .with_visible(false)
            .with_active(false)
            .with_decorations(false)
            .with_inner_size(winit::dpi::LogicalSize::new(
                f64::from(WINDOW_W),
                f64::from(WINDOW_H),
            ));
        let window = Arc::new(events.create_window(attributes).expect("create window"));

        let proxy: EventLoopProxy<Wake> = self.proxy.clone();
        let bridge = WindowBridge::before_first_reveal(
            events,
            &window,
            Arc::new(move || {
                let _ = proxy.send_event(Wake::Accessibility);
            }),
        );
        assert!(
            bridge.is_some(),
            "no AccessKit adapter was installed on a hidden window — nothing below can mean \
             anything"
        );
        self.pane = Some(PaneAccessibility::new(
            self.window_slot,
            self.view_slot,
            bridge.map(|bridge| Box::new(bridge) as Box<dyn AccessibilityProvider>),
        ));
        window.set_visible(true);
        if let Some(pane) = self.pane.as_mut() {
            pane.revealed();
        }
        self.window = Some(window.clone());

        let ready = Ready {
            hwnd: hwnd_of(&window),
            body: Occurrence::surface(self.window_slot, self.view_slot, BODY, ControlPart::ROOT)
                .locator(),
            paragraphs: self.rows.iter().map(|id| self.locator(*id)).collect(),
        };
        if let Some(tx) = self.ready_tx.take() {
            let _ = tx.send(ready);
        }
    }

    fn window_event(&mut self, _events: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let (Some(pane), Some(window)) = (self.pane.as_mut(), self.window.as_ref()) {
            pane.process_event(window, &event);
        }
    }

    fn user_event(&mut self, _events: &ActiveEventLoop, wake: Wake) {
        match wake {
            Wake::Accessibility => {}
            Wake::Ask(ask) => self.apply(ask),
        }
    }

    fn about_to_wait(&mut self, events: &ActiveEventLoop) {
        if self.pane.is_none() {
            return;
        }
        // The loop's own accessibility pass, twice: an action a client sent may
        // change what the tree should say, and the client is entitled to see
        // the result before it is told anything.
        for _ in 0..2 {
            if self.pane.as_ref().expect("a pane").listening() {
                let revision = View::semantics_revision(&self.view);
                self.pane.as_mut().expect("a pane").observe(revision);
            }
            let poll = self.pane.as_mut().expect("a pane").poll();
            if poll.snapshot {
                let scope = if poll.full {
                    Scope::Whole
                } else {
                    Scope::Delta
                };
                let semantics = self.semantics(scope);
                self.pane.as_mut().expect("a pane").publish(&semantics);
            }
            let resolved = self.pane.as_mut().expect("a pane").resolve();
            if resolved.is_empty() {
                break;
            }
            for request in resolved {
                View::accessibility_action(
                    &mut self.view,
                    &mut self.workspace.ws,
                    request.occurrence,
                    request.action,
                    request.data.as_ref(),
                );
            }
        }
        if self.owes_reply {
            self.owes_reply = false;
            let _ = self.state_tx.send(self.state());
        }
        if self.finished {
            if let Some(pane) = self.pane.as_mut() {
                pane.retire();
            }
            // `exit()` from `about_to_wait` is not honoured until the loop's next
            // message: winit blocks in `MsgWaitForMultipleObjectsEx` *before* it
            // reads the exit flag, so under `ControlFlow::Wait` this parks until
            // some unrelated message happens to arrive. See guide/contributing/deps/WINIT.md.
            events.set_control_flow(ControlFlow::Poll);
            events.exit();
        }
    }
}

fn hwnd_of(window: &Window) -> isize {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window
        .window_handle()
        .expect("a live window has a handle")
        .as_raw()
    {
        RawWindowHandle::Win32(handle) => handle.hwnd.get(),
        other => panic!("not a Win32 window: {other:?}"),
    }
}

// ----------------------------------------------------------------- gates

#[derive(Debug, Default)]
struct Findings {
    root_automation_id: Option<String>,
    body_found: bool,
    body_name: Option<String>,
    paragraph_found_by_automation_id: bool,
    first_paragraph_text: Option<String>,
    second_paragraph_text: Option<String>,
    empty_paragraph_text: Option<String>,
    control_type_is_edit: bool,
    supported_text_selection: Option<i32>,
    /// **Does a paragraph's read-only state reach a Windows client at all?**
    /// `accesskit_windows` carries `IsReadOnly` only on `ValuePattern`, and it
    /// only offers `ValuePattern` to a node that publishes a `value`. A Koru
    /// paragraph publishes none — deliberately, because a `ValuePattern` also
    /// offers `SetValue`, which no accessibility request may perform. So this
    /// records the answer rather than asserting one.
    value_pattern_offered: Option<bool>,
    is_read_only_property: Option<String>,
    selection_after_set: Option<String>,
    caret_after_set: Option<(usize, usize)>,
    typed_replacement: Option<String>,
    after_undo: Option<String>,
    events_seen: Vec<i32>,
    incremental: Option<ShellState>,
    error: Option<String>,
}

/// Run a closure on the way out, whichever way the client leaves.
fn scopeguard(f: impl FnOnce()) -> impl Drop {
    struct Guard<F: FnOnce()>(Option<F>);
    impl<F: FnOnce()> Drop for Guard<F> {
        fn drop(&mut self) {
            if let Some(f) = self.0.take() {
                f();
            }
        }
    }
    Guard(Some(f))
}

/// A real UIA event client. The gate is about *receipt*: an event this process
/// created and never delivered is not an event a screen reader hears.
#[windows::core::implement(windows::Win32::UI::Accessibility::IUIAutomationEventHandler)]
struct EventSink(Arc<Mutex<Vec<i32>>>);

impl windows::Win32::UI::Accessibility::IUIAutomationEventHandler_Impl for EventSink_Impl {
    fn HandleAutomationEvent(
        &self,
        _sender: windows::core::Ref<windows::Win32::UI::Accessibility::IUIAutomationElement>,
        event: windows::Win32::UI::Accessibility::UIA_EVENT_ID,
    ) -> windows::core::Result<()> {
        if let Ok(mut seen) = self.0.lock() {
            seen.push(event.0);
        }
        Ok(())
    }
}

fn client(
    ready: &Receiver<Ready>,
    states: &Receiver<ShellState>,
    proxy: &EventLoopProxy<Wake>,
    findings: &Arc<Mutex<Findings>>,
) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::*;

    let ready = ready.recv_timeout(TIMEOUT).expect("the window was created");
    let hwnd = HWND(ready.hwnd as *mut core::ffi::c_void);

    // The client initialises COM; the provider deliberately does not.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .expect("COM initialised");
    let _com = scopeguard(|| unsafe { CoUninitialize() });

    let uia: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
            .expect("a UI Automation client");

    // Poll: the first `ElementFromHandle` is what sends `WM_GETOBJECT`, and the
    // adapter serves its placeholder tree until the loop's next pass.
    let deadline = Instant::now() + TIMEOUT;
    let mut root = None;
    while Instant::now() < deadline {
        let element = unsafe { uia.ElementFromHandle(hwnd) }.expect("the window has an element");
        let id = unsafe { element.CurrentAutomationId() }
            .map(|id| id.to_string())
            .unwrap_or_default();
        if id.starts_with("koru:") {
            findings.lock().expect("no poison").root_automation_id = Some(id);
            root = Some(element);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let root = root.expect("the container window never published a Koru locator");

    let find = |locator: &str| -> Option<IUIAutomationElement> {
        let condition = unsafe {
            uia.CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(locator))
        }
        .expect("a property condition");
        unsafe { root.FindFirst(TreeScope_Descendants, &condition) }.ok()
    };
    let find_waiting = |locator: &str| -> Option<IUIAutomationElement> {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if let Some(found) = find(locator) {
                return Some(found);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        None
    };
    let ask = |ask: Ask| -> ShellState {
        proxy.send_event(Wake::Ask(ask)).expect("the loop is alive");
        states.recv_timeout(TIMEOUT).expect("the loop answered")
    };
    let text_of = |element: &IUIAutomationElement| -> String {
        let pattern: IUIAutomationTextPattern =
            unsafe { element.GetCurrentPatternAs(UIA_TextPatternId) }
                .expect("the paragraph offers the Text pattern");
        let range = unsafe { pattern.DocumentRange() }.expect("a document range");
        unsafe { range.GetText(-1) }
            .map(|text| text.to_string())
            .unwrap_or_default()
    };

    // --------------------------------------------- the body and its paragraphs
    if let Some(body) = find_waiting(&ready.body) {
        let mut found = findings.lock().expect("no poison");
        found.body_found = true;
        found.body_name = unsafe { body.CurrentName() }.ok().map(|n| n.to_string());
    }
    let first = find_waiting(&ready.paragraphs[0])
        .expect("the first paragraph of the open document, by AutomationId");
    {
        let mut found = findings.lock().expect("no poison");
        found.paragraph_found_by_automation_id = true;
        found.control_type_is_edit = unsafe { first.CurrentControlType() }
            .map(|control| control == UIA_EditControlTypeId)
            .unwrap_or(false);
        found.first_paragraph_text = Some(text_of(&first));
        found.second_paragraph_text =
            find_waiting(&ready.paragraphs[1]).map(|element| text_of(&element));
        found.empty_paragraph_text =
            find_waiting(&ready.paragraphs[2]).map(|element| text_of(&element));
        let pattern: IUIAutomationTextPattern =
            unsafe { first.GetCurrentPatternAs(UIA_TextPatternId) }.expect("the Text pattern");
        found.supported_text_selection = unsafe { pattern.SupportedTextSelection() }
            .ok()
            .map(|supported| supported.0);
        // Recorded, not asserted — see `Findings::value_pattern_offered`.
        found.value_pattern_offered = Some(
            unsafe { first.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }
                .is_ok(),
        );
        found.is_read_only_property =
            unsafe { first.GetCurrentPropertyValue(UIA_ValueIsReadOnlyPropertyId) }
                .ok()
                .map(|value| format!("{value:?}"));
    }

    // ------------------------------------------------------------- the events
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink: IUIAutomationEventHandler = EventSink(seen.clone()).into();
    for id in [
        UIA_Text_TextChangedEventId,
        UIA_Text_TextSelectionChangedEventId,
    ] {
        unsafe { uia.AddAutomationEventHandler(id, &root, TreeScope_Subtree, None, &sink) }
            .expect("an automation event handler");
    }
    let _handlers = scopeguard({
        let uia = uia.clone();
        let root = root.clone();
        let sink = sink.clone();
        move || {
            for id in [
                UIA_Text_TextChangedEventId,
                UIA_Text_TextSelectionChangedEventId,
            ] {
                let _ = unsafe { uia.RemoveAutomationEventHandler(id, &root, &sink) };
            }
        }
    });

    // ---------------------------------------- select, replace, and put it back
    let second = find_waiting(&ready.paragraphs[1]).expect("the second paragraph");
    let pattern: IUIAutomationTextPattern =
        unsafe { second.GetCurrentPatternAs(UIA_TextPatternId) }.expect("the Text pattern");
    let document_range = unsafe { pattern.DocumentRange() }.expect("a document range");

    // "The second paragraph." — select the word `second`, characters 4..10.
    let range = unsafe { document_range.Clone() }.expect("a cloned range");
    unsafe { range.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, 4) }
        .expect("move the start to character 4");
    unsafe {
        range.MoveEndpointByRange(
            TextPatternRangeEndpoint_End,
            &range,
            TextPatternRangeEndpoint_Start,
        )
    }
    .expect("collapse the end onto the start");
    unsafe { range.MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, 6) }
        .expect("extend the end by six characters");
    unsafe { range.Select() }.expect("Select() reached the provider");

    // The provider resolves on the loop, so give it a pass and then ask.
    let state = ask(Ask::State);
    {
        let mut found = findings.lock().expect("no poison");
        found.caret_after_set = Some(state.caret);
        found.selection_after_set = unsafe { pattern.GetSelection() }
            .ok()
            .and_then(|selection| unsafe { selection.GetElement(0) }.ok())
            .and_then(|r| unsafe { r.GetText(-1) }.ok())
            .map(|text| text.to_string());
    }

    // Real typing, through `View::key`, replacing exactly that range.
    let _ = ask(Ask::Press {
        text: Some("X".to_string()),
        mods: ModifiersState::empty(),
    });
    findings.lock().expect("no poison").typed_replacement =
        find_waiting(&ready.paragraphs[1]).map(|element| text_of(&element));

    // …and undo it, which is the other half of A2.
    let _ = ask(Ask::Press {
        text: Some("z".to_string()),
        mods: ModifiersState::CONTROL,
    });
    findings.lock().expect("no poison").after_undo =
        find_waiting(&ready.paragraphs[1]).map(|element| text_of(&element));

    let last = ask(Ask::State);
    {
        let mut found = findings.lock().expect("no poison");
        found.incremental = Some(last);
        found.events_seen = seen.lock().expect("no poison").clone();
    }

    let _ = proxy.send_event(Wake::Ask(Ask::Finish));
}

#[test]
#[ignore = "interactive Windows proof: opens a real container window and drives it from a UIA client"]
fn the_shipping_window_hands_a_real_document_to_a_real_client() {
    use windows::Win32::UI::Accessibility::{
        UIA_Text_TextChangedEventId, UIA_Text_TextSelectionChangedEventId,
    };

    let event_loop: EventLoop<Wake> = EventLoop::with_user_event()
        // The Rust harness does not run tests on the process's main thread.
        .with_any_thread(true)
        .build()
        .expect("an event loop");
    let proxy = event_loop.create_proxy();
    let (ready_tx, ready_rx) = channel();
    let (state_tx, state_rx) = channel();
    let findings = Arc::new(Mutex::new(Findings::default()));

    let client_findings = findings.clone();
    let client_proxy = proxy.clone();
    let worker = std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            client(&ready_rx, &state_rx, &client_proxy, &client_findings);
        }));
        if result.is_err() {
            client_findings.lock().expect("no poison").error =
                Some("the client thread panicked; see the captured output".to_string());
            let _ = client_proxy.send_event(Wake::Ask(Ask::Finish));
        }
    });

    let mut harness = Harness::new(proxy, state_tx, ready_tx);
    event_loop.run_app(&mut harness).expect("the loop ran");
    worker.join().expect("the client thread finished");

    let found = findings.lock().expect("no poison");
    println!("\n--- Findings: the shipping container, from outside the process ---");
    println!("{found:#?}");
    assert!(found.error.is_none(), "{:?}", found.error);
    assert!(
        found.paragraph_found_by_automation_id,
        "a paragraph of the open document was not findable by AutomationId"
    );
    assert!(found.body_found, "the document body was not in the tree");
    assert_eq!(found.first_paragraph_text.as_deref(), Some(FIRST));
    assert_eq!(found.second_paragraph_text.as_deref(), Some(SECOND));
    assert_eq!(found.empty_paragraph_text.as_deref(), Some(EMPTY));
    assert!(
        found.control_type_is_edit,
        "a paragraph is not an edit control to a client"
    );
    assert_eq!(found.selection_after_set.as_deref(), Some("second"));
    assert_eq!(found.caret_after_set, Some((4, 10)));
    assert_eq!(found.typed_replacement.as_deref(), Some("The X paragraph."));
    assert_eq!(found.after_undo.as_deref(), Some(SECOND));
    // **At least one, never exactly one.** UIA has been observed delivering one
    // change to a subtree-scoped handler more than once; what a provider
    // controls is that nothing is raised when nothing changed.
    assert!(
        found.events_seen.contains(&UIA_Text_TextChangedEventId.0),
        "no native text-changed event reached the client"
    );
    assert!(
        found
            .events_seen
            .contains(&UIA_Text_TextSelectionChangedEventId.0),
        "no native selection-changed event reached the client"
    );
    let state = found.incremental.as_ref().expect("the window's accounting");
    assert!(
        state.deltas > 0 && state.deltas < state.rebuilds,
        "the shipping window did not publish a whole tree and then differences:          {} rebuild(s), {} of them differences",
        state.rebuilds,
        state.deltas
    );
    assert!(state.nodes_published > 0);
    assert_eq!(state.unresolved_focus, 0, "a published focus named nothing");
    assert!(state.actions_routed > 0, "no action reached the view");
    assert_eq!(
        state.actions_refused, 0,
        "a request this proof sent was refused"
    );
    // The window's own answer to "where is the keyboard", which must be a
    // paragraph of the open document rather than the region around it.
    assert!(
        state
            .focus_locator
            .as_deref()
            .is_some_and(|at| at.ends_with("#text")),
        "the focus is not in a paragraph: {:?}",
        state.focus_locator
    );
    // parley shaped something — the document was really laid out — and not once
    // per publish.
    assert!(state.shapings > 0 && state.shapings < state.nodes_published);
}
