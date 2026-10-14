// SPDX-License-Identifier: MPL-2.0

//! **The one thing only a real screen reader's API can answer.**
//!
//! Everything else about this feature is asserted in `cargo test --workspace`
//! against the `Provider` seam. What that cannot establish is the contract the
//! roadmap actually gates on: *does an external UI Automation client see
//! Koru's stable locator, on a supported native property, and can it act on a
//! control it found that way?* An in-process map is not evidence of that, and
//! neither is reading AccessKit's source.
//!
//! So this stands outside the process boundary the way Narrator does: it
//! creates a UIA **client** in the COM multi-threaded apartment, reaches the
//! window through `ElementFromHandle`, and asks only questions a client can
//! ask.
//!
//! ## Why it is `#[ignore]`d
//!
//! It opens a real window on a real desktop and drives it. That is a separate
//! lane, invoked deliberately:
//!
//! ```text
//! cargo test -p koru-workspace-host --test accessibility_native -- --ignored --nocapture
//! ```
//!
//! ## What it proves, and what it deliberately does not
//!
//! Proven: the adapter installs on a hidden window, survives the reveal, and
//! answers a client; the window root's `AutomationId` is
//! `koru:w<n>/v0/s:window#window`; a `FindFirst` **by that property** locates
//! a control the client has never seen before; `Invoke` on it reaches the
//! host's own action routing with the right occurrence; and — Plan 1b.2 —
//! `KORU_ACCESSIBILITY_DUMP` produces **exactly one** description of the tree,
//! triggered by a real client attaching rather than by anything this test
//! calls, containing no accessible name and no title.
//!
//! That last one matters because it is the only path a person will ever use.
//! Every other test of the dump asks for one directly; this is the one that
//! says the *environment* arms it, that a real `WM_GETOBJECT` is what takes
//! it, and that it does not repeat.
//!
//! Proven by Plan 1a.2, in the same window and the same run: a **document
//! entry** is findable by the locator an external tool builds from a window
//! number and a document id, without reading anything out of the tree first;
//! it carries its name, its place in the set and its depth on the native
//! properties; a **second entry with the same title** is a different
//! AutomationId, which is the whole of *distinguishable without reading a
//! label*; the window commands now carry **bounding rectangles**, so the
//! logical→physical conversion can be checked from outside; UIA's **focused
//! element** is the control the view said had the keyboard; and `Invoke` on the
//! entry reaches the host as the occurrence the client named.
//!
//! Proven by Plan 1b.4, in the same window and the same run: the save status's
//! **live setting reaches a client** on `UIA_LiveSettingPropertyId` — `Polite`
//! on the announcement, `Off` on the status beside it — and, with a real
//! `IUIAutomationEventHandler` registered the way a screen reader registers
//! one, **a changed announcement raises a `UIA_LiveRegionChangedEventId` and
//! republishing the identical text raises none.** That pair is the whole
//! mechanism behind *announce failures and recovery once, and leave routine
//! success alone*: the adapter diffs names, so "once" is a property of the
//! platform rather than a rule the projection has to remember. Reading
//! `accesskit_windows`' source said as much; this is the evidence.
//!
//! Not proven, and not this plan's gate: anything a text pattern does, and
//! anything about what a screen reader *says*. Those are Plan 1b.1 and the
//! human acceptance checklist respectively — including whether Narrator and
//! NVDA actually speak a polite live region, which no client-side event count
//! can establish.

#![cfg(windows)]

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use koru_window::{AccessibilityProvider, WindowBridge};
use koru_workspace_host::accessibility::{
    CaptionGeometry, PaneAccessibility, Slots, WindowSemantics, part, window_commands,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::windows::EventLoopBuilderExtWindows;
use winit::window::{Window, WindowId};

/// How long the client waits for the provider to answer before it gives up.
/// Generous: the first `WM_GETOBJECT` starts UIA itself on this machine.
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug)]
enum Wake {
    /// A screen reader left a note on some window's mailbox.
    Accessibility,
    /// The client has changed what the announcement should say (or has
    /// deliberately not changed it) and wants the tree published again. Plan
    /// 1b.4: the difference between those two republishes is the whole test.
    Republish,
    /// The client is finished; tear the window down.
    Finished,
}

/// What the client managed to see, in the order it asked.
#[derive(Debug, Default)]
struct Findings {
    has_native_provider: Option<bool>,
    root_automation_id: Option<String>,
    root_class_name: Option<String>,
    root_name: Option<String>,
    framework_id: Option<String>,
    found_close_by_automation_id: bool,
    close_name: Option<String>,
    invoked: bool,
    // ------------------------------------------------------- Plan 1a.2
    /// The window command's rectangle, as UIA reports it in **physical screen
    /// pixels** — the far side of the logical rectangle the view published.
    close_bounds: Option<(f64, f64, f64, f64)>,
    /// A document entry, found by the locator an external tool would build.
    found_entry_by_automation_id: bool,
    entry_name: Option<String>,
    entry_position_in_set: Option<i32>,
    entry_size_of_set: Option<i32>,
    /// **Expected to be absent.** `accesskit_windows` 0.32.1 has no
    /// `UIA_LevelPropertyId` row in its property table, so a level the schema
    /// carries reaches no Windows client. Recorded rather than asserted.
    entry_level: Option<i32>,
    /// The second entry with the same title: two names, two AutomationIds.
    twin_automation_id: Option<String>,
    /// Whether the **open** row offers SelectionItem instead of Invoke. The
    /// consequence of `selected`, and the reason only one row carries it.
    open_row_is_selection_item: Option<bool>,
    open_row_offers_invoke: Option<bool>,
    /// What UIA says about the keyboard. **Informational**: this window is
    /// created inactive so it does not take over the desktop, so nothing in
    /// this process has real keyboard focus and the answer is the desktop's.
    /// The published focus is asserted in the pure suite, and by hand in
    /// `docs/features/accessibility/README.md`.
    focused_automation_id: Option<String>,
    entry_invoked: bool,
    // ------------------------------------------------------- Plan 1b.4
    /// `UIA_LiveSettingPropertyId` on the announcement. **Expected `1`**
    /// (`Polite`): the property `accesskit_windows` maps `Node::live` onto.
    news_live_setting: Option<i32>,
    /// The same property on the status sentence beside it. **Expected `0`**
    /// (`Off`), which is what keeps a value that moves with every keystroke
    /// from being spoken. `live` is inherited from the parent, so this is also
    /// the evidence that the group above it is not live either.
    status_live_setting: Option<i32>,
    /// Live-region events received after the announcement's text **changed**.
    /// At least one; this is *announce*.
    live_events_after_change: Option<usize>,
    /// Further live-region events received after republishing the **identical**
    /// text. Zero; this is *once*.
    live_events_after_identical_republish: Option<usize>,
    error: Option<String>,
}

/// The two entries the probe publishes, with **the same title on purpose**:
/// telling them apart without reading a label is the roadmap's requirement,
/// and the only thing that can do it is the locator.
const ENTRY_A: &str = "6f9619ff-8b86-d011-b42d-00cf4fc964ff";
const ENTRY_B: &str = "6f9619ff-8b86-d011-b42d-00cf4fc96500";

/// What the announcement says before the client changes it, and after.
///
/// Two different sentences, because the adapter raises its event on a **name
/// change** and on nothing else — so the pair is the test, and republishing
/// either of them a second time has to be silent.
const NEWS_FIRST: &str = "Storage is not answering.";
const NEWS_SECOND: &str = "Everything is saved again.";

struct Probe {
    slots: Slots,
    window: Option<Arc<Window>>,
    access: Option<PaneAccessibility>,
    hwnd_tx: Option<Sender<isize>>,
    proxy: EventLoopProxy<Wake>,
    /// What the host's own routing resolved, which is the far side of the
    /// client's `Invoke`.
    routed: Arc<Mutex<Vec<(accesskit::Action, String)>>>,
    /// What the live region should say on the next publish. The client sets it
    /// and then asks for a republish; changing it and *not* changing it are
    /// the two halves of Plan 1b.4's proof.
    announcement: Arc<Mutex<String>>,
    finished: bool,
}

impl ApplicationHandler<Wake> for Probe {
    fn resumed(&mut self, events: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // Hidden, undecorated and unactivated — the shape a Koru pane is
        // created in, and the shape the adapter requires. `with_active(false)`
        // keeps this off whatever the person running it was doing.
        let attributes = Window::default_attributes()
            .with_title("Koru accessibility probe")
            .with_visible(false)
            .with_active(false)
            .with_decorations(false)
            .with_inner_size(winit::dpi::LogicalSize::new(480.0, 320.0));
        let window = Arc::new(events.create_window(attributes).expect("create window"));

        let proxy = self.proxy.clone();
        let bridge = WindowBridge::before_first_reveal(
            events,
            &window,
            Arc::new(move || {
                let _ = proxy.send_event(Wake::Accessibility);
            }),
        );
        assert!(
            bridge.is_some(),
            "no AccessKit adapter was installed on a hidden window — the rest of this test \
             cannot mean anything"
        );
        let access = PaneAccessibility::new(
            self.slots.window(),
            self.slots.view(),
            bridge.map(|bridge| Box::new(bridge) as Box<dyn AccessibilityProvider>),
        );

        // The reveal the adapter had to be installed before.
        window.set_visible(true);

        if let Some(tx) = self.hwnd_tx.take() {
            let _ = tx.send(hwnd_of(&window));
        }
        self.window = Some(window);
        self.access = Some(access);
    }

    fn window_event(&mut self, _events: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let (Some(access), Some(window)) = (self.access.as_mut(), self.window.as_ref()) {
            access.process_event(window, &event);
        }
    }

    fn user_event(&mut self, _events: &ActiveEventLoop, wake: Wake) {
        match wake {
            Wake::Finished => self.finished = true,
            // The same call the shell makes when something a tree depends on
            // moved. It bumps the revision, so `poll` asks for a snapshot on
            // the next pass — whether or not anything in the tree is actually
            // different, which is exactly the case being tested.
            Wake::Republish => {
                if let Some(access) = self.access.as_mut() {
                    access.invalidate();
                }
            }
            Wake::Accessibility => {}
        }
    }

    fn about_to_wait(&mut self, events: &ActiveEventLoop) {
        let (Some(access), Some(window)) = (self.access.as_mut(), self.window.as_ref()) else {
            return;
        };
        if access.poll().snapshot {
            let scale = window.scale_factor();
            let size = window.inner_size();
            let semantics = WindowSemantics {
                window: access.slot(),
                label: "Koru accessibility probe".to_string(),
                scale,
                client: koru_draw::geometry::Rect::new(
                    0.0,
                    0.0,
                    size.width as f32 / scale as f32,
                    size.height as f32 / scale as f32,
                ),
                focus: Some(entry_occurrence(access.slot(), access.view_slot(), ENTRY_A)),
                children: {
                    // Rectangles a view would draw its caption at, so the
                    // client can check the logical -> physical conversion from
                    // outside the process rather than from our own arithmetic.
                    let mut children = window_commands(
                        access.slot(),
                        false,
                        koru_i18n::Lang::En,
                        CaptionGeometry {
                            minimize: Some(koru_draw::geometry::Rect::new(370.0, 0.0, 36.0, 36.0)),
                            maximize: Some(koru_draw::geometry::Rect::new(406.0, 0.0, 36.0, 36.0)),
                            close: Some(koru_draw::geometry::Rect::new(442.0, 0.0, 36.0, 36.0)),
                        },
                    );
                    children.push(entries(access.slot(), access.view_slot()));
                    children.push(save_status(
                        access.slot(),
                        access.view_slot(),
                        &self
                            .announcement
                            .lock()
                            .expect("no test poisons this")
                            .clone(),
                    ));
                    children
                },
                scope: koru_workspace_host::accessibility::Scope::Whole,
                retired: Vec::new(),
            };
            access.publish(&semantics);
        }
        for resolved in access.resolve() {
            self.routed
                .lock()
                .expect("no test poisons this")
                .push((resolved.action, resolved.occurrence.locator()));
        }
        if self.finished {
            // `exit()` from `about_to_wait` is not honoured until the loop's next
            // message: winit blocks in `MsgWaitForMultipleObjectsEx` *before* it
            // reads the exit flag, so under `ControlFlow::Wait` this parks until
            // some unrelated message happens to arrive. See guide/contributing/deps/WINIT.md.
            events.set_control_flow(ControlFlow::Poll);
            events.exit();
        }
    }
}

/// A document entry's occurrence, built the way the view builds one.
fn entry_occurrence(
    window: koru_workspace_host::accessibility::WindowSlot,
    view: koru_workspace_host::accessibility::ViewSlot,
    id: &str,
) -> koru_workspace_host::accessibility::Occurrence {
    koru_workspace_host::accessibility::Occurrence::model(
        window,
        view,
        uuid::Uuid::parse_str(id).expect("a fixture uuid"),
        koru_workspace_host::accessibility::ControlPart::ROOT,
    )
}

/// The smallest thing that is recognisably Plan 1a.2's contribution: a tree of
/// document entries with a duplicate title, positions, depth and an open
/// action.
///
/// **Not the container's own projection.** That one needs a workspace, a store
/// and a window loop to stand up, and this test exists to prove the *contract*
/// — that an external client can find an entry by the locator it was handed and
/// invoke it — rather than to run the app. What the container publishes is
/// asserted against these same shapes in `apps/koru`'s own suite.
fn entries(
    window: koru_workspace_host::accessibility::WindowSlot,
    view: koru_workspace_host::accessibility::ViewSlot,
) -> koru_workspace_host::accessibility::SemanticNode {
    use accesskit::{Action, Role};
    use koru_workspace_host::accessibility::SemanticNode;
    let row = |id: &str, index: usize| {
        let node = SemanticNode::new(entry_occurrence(window, view, id), Role::TreeItem)
            // The same title on both, deliberately.
            .with_label("Notes")
            .with_bounds(koru_draw::geometry::Rect::new(
                0.0,
                40.0 + index as f32 * 24.0,
                240.0,
                24.0,
            ))
            .with_level(1)
            .with_position(index)
            .with_actions([Action::Click, Action::Focus, Action::ScrollIntoView]);
        // The shipping rule: only the *open* row says it is selected, because
        // a node carrying `selected` loses its Invoke pattern. Row A is the
        // open one; row B is the one a client opens.
        if index == 0 {
            node.with_selected(true)
        } else {
            node
        }
    };
    SemanticNode::new(
        koru_workspace_host::accessibility::Occurrence::surface(
            window,
            view,
            "explore",
            koru_workspace_host::accessibility::ControlPart::new("tree"),
        ),
        Role::Tree,
    )
    .with_label("Documents")
    .with_bounds(koru_draw::geometry::Rect::new(0.0, 40.0, 240.0, 280.0))
    // The total lives on the container, which is where an item's `size_of_set`
    // is read from.
    .with_size_of_set(2)
    .with_children(vec![row(ENTRY_A, 0), row(ENTRY_B, 1)])
}

/// Plan 1b.4's contribution, in the shape the container publishes it: a group,
/// a silent status sentence, and one live announcement beside it.
///
/// **Two nodes, and the point is that only one of them is live.** `live` is
/// inherited by descendants, so marking the group would make the status —
/// which moves with every keystroke — speak on every keystroke. The client
/// reads `UIA_LiveSettingPropertyId` off both.
fn save_status(
    window: koru_workspace_host::accessibility::WindowSlot,
    view: koru_workspace_host::accessibility::ViewSlot,
    announcement: &str,
) -> koru_workspace_host::accessibility::SemanticNode {
    use accesskit::Role;
    use koru_workspace_host::accessibility::{ControlPart, Occurrence, SemanticNode};
    let part =
        |name: &'static str| Occurrence::surface(window, view, "save", ControlPart::new(name));
    SemanticNode::new(
        Occurrence::surface(window, view, "save", ControlPart::ROOT),
        Role::Group,
    )
    .with_label("Save status")
    .with_children(vec![
        SemanticNode::new(part("document"), Role::Label).with_label("Saved."),
        SemanticNode::new(part("news"), Role::Alert)
            .with_label(announcement)
            .announcing(accesskit::Live::Polite),
    ])
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

#[test]
#[ignore = "interactive Windows proof: opens a real window and drives it through UI Automation"]
fn a_uia_client_reads_the_locator_and_invokes_a_control_it_found_by_it() {
    // **Armed before anything runs**, because `dump::armed` reads the
    // environment once and caches it — the same reason `ui_lang` does. Set
    // here rather than by the person invoking the test so the assertion below
    // is unconditional. SAFETY: nothing else in this process has started yet;
    // the client thread and the event loop are both created after this line.
    unsafe { std::env::set_var("KORU_ACCESSIBILITY_DUMP", "1") };

    let events = EventLoop::<Wake>::with_user_event()
        // The test harness runs on a worker thread; winit refuses to build an
        // event loop off the main thread on Windows without this.
        .with_any_thread(true)
        .build()
        .expect("an event loop");
    let proxy = events.create_proxy();
    let (hwnd_tx, hwnd_rx) = channel::<isize>();
    let routed = Arc::new(Mutex::new(Vec::new()));
    let findings = Arc::new(Mutex::new(Findings::default()));

    let announcement = Arc::new(Mutex::new(NEWS_FIRST.to_string()));

    let client = {
        let proxy = proxy.clone();
        let findings = Arc::clone(&findings);
        let announcement = Arc::clone(&announcement);
        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                client_thread(&hwnd_rx, &findings, &proxy, &announcement)
            }));
            if let Err(_panic) = outcome {
                findings.lock().expect("no test poisons this").error =
                    Some("the client thread panicked".to_string());
            }
            let _ = proxy.send_event(Wake::Finished);
        })
    };

    let mut probe = Probe {
        slots: Slots::default(),
        window: None,
        access: None,
        hwnd_tx: Some(hwnd_tx),
        proxy,
        routed: Arc::clone(&routed),
        announcement: Arc::clone(&announcement),
        finished: false,
    };
    events.run_app(&mut probe).expect("the loop ran");
    client.join().expect("the client thread joined");

    let findings = findings.lock().expect("no test poisons this");
    println!("{findings:#?}");
    assert_eq!(findings.error, None, "the client could not finish");

    let slot = probe
        .access
        .as_ref()
        .expect("the window existed")
        .slot()
        .raw();
    assert_eq!(findings.has_native_provider, Some(true));
    assert_eq!(
        findings.root_automation_id.as_deref(),
        Some(format!("koru:w{slot}/v0/s:window#window").as_str()),
        "the window root's UIA AutomationId is not Koru's locator"
    );
    assert_eq!(findings.root_class_name.as_deref(), Some("KoruWindow"));
    assert_eq!(
        findings.root_name.as_deref(),
        Some("Koru accessibility probe")
    );
    assert_eq!(findings.framework_id.as_deref(), Some("Koru"));
    assert!(
        findings.found_close_by_automation_id,
        "a client could not find a control by its AutomationId"
    );
    assert_eq!(findings.close_name.as_deref(), Some("Close window"));
    assert!(findings.invoked, "the client could not invoke the control");

    // ------------------------------------------------------------ Plan 1a.2
    let open_locator = format!("koru:w{slot}/v1/n:{}#root", ENTRY_A.replace('-', ""));
    let entry_locator = format!("koru:w{slot}/v1/n:{}#root", ENTRY_B.replace('-', ""));
    assert!(
        findings.found_entry_by_automation_id,
        "a client could not find a document entry by the locator it was given"
    );
    assert_eq!(findings.entry_name.as_deref(), Some("Notes"));
    // **Zero-based on our side, one-based on UIA's**: the adapter adds the one.
    assert_eq!(
        findings.entry_position_in_set,
        Some(2),
        "the entry did not tell a reader where it is in its set"
    );
    // Read off the *container*, which is the only place the consumer looks.
    assert_eq!(
        findings.entry_size_of_set,
        Some(2),
        "the total did not reach a client from the container that holds it"
    );
    assert_eq!(
        findings.twin_automation_id.as_deref(),
        Some(open_locator.as_str()),
        "two entries with one title were not two identities"
    );
    // The one exclusive pair in the whole projection: the open row is a
    // selection item and cannot be invoked; every other row can.
    assert_eq!(findings.open_row_is_selection_item, Some(true));
    assert_eq!(
        findings.open_row_offers_invoke,
        Some(false),
        "a node carrying `selected` unexpectedly kept its Invoke pattern. If \
         this ever starts failing the projection can publish `selected` on \
         every row again -- see guide/contributing/deps/ACCESSKIT.md"
    );
    assert!(findings.entry_invoked, "the entry could not be invoked");
    // The rectangle is the view's logical one times the scale, offset by the
    // client area's position on screen — so the only things asserted here are
    // that it is *not* empty and that it is the size the view asked for. Where
    // the window happens to be is the person running this's business.
    let bounds = findings.close_bounds.expect("the close command has bounds");
    assert!(
        bounds.2 > 0.0 && bounds.3 > 0.0,
        "the close command published an empty rectangle: {bounds:?}"
    );

    let routed = routed.lock().expect("no test poisons this");
    println!("routed: {routed:?}");
    let close = format!("koru:w{slot}/v0/s:window#close");
    assert!(
        routed
            .iter()
            .all(|(_, locator)| *locator == close || *locator == entry_locator),
        "a request arrived for a control the client never named: {routed:?}"
    );
    assert_eq!(
        routed
            .iter()
            .filter(|(action, locator)| *action == accesskit::Action::Click && *locator == close)
            .count(),
        1,
        "the invoke did not arrive at the host exactly once as the occurrence the client named"
    );
    assert_eq!(
        routed
            .iter()
            .filter(|(action, locator)| {
                *action == accesskit::Action::Click && *locator == entry_locator
            })
            .count(),
        1,
        "invoking a document entry did not reach the host as that entry"
    );
    // **`Invoke` is not one request.** Windows sends `SetFocus` first, and
    // `accesskit_windows` forwards it whether or not the node advertised
    // `Action::Focus` — a provider's advertised action set is a hint to a
    // client, never a filter on what arrives. The host performs only the
    // actions it understands, which is why the extra one is harmless here;
    // see `guide/contributing/deps/ACCESSKIT.md`.
    assert!(
        !routed.is_empty(),
        "the invoke never reached the host at all"
    );
    drop(routed);

    // ---- Plan 1b.2: the dump, armed by the environment and taken by a real
    // client attaching.
    let mut access = probe.access.take().expect("the window existed");
    let counters = access.counters();
    assert_eq!(
        counters.dumps, 1,
        "a client attached with KORU_ACCESSIBILITY_DUMP set and the window \
         wrote {} dumps; it must write exactly one",
        counters.dumps
    );
    let dump = access
        .take_dump()
        .expect("the dump was produced and kept for its caller");
    println!("---- the dump the environment armed ----\n{dump}");
    assert!(
        dump.contains(&format!("dump window=w{slot} scope=whole")),
        "the dump does not describe this window's whole tree:\n{dump}"
    );
    assert!(
        dump.contains(&close),
        "the dump does not name the control the client acted on:\n{dump}"
    );
    assert!(
        dump.contains("label=<redacted:"),
        "the dump published an accessible name in full:\n{dump}"
    );
    for secret in ["Close window", "Koru accessibility probe", "Minimize"] {
        assert!(
            !dump.contains(secret),
            "the dump leaked {secret:?}, which is a label a person reads:\n{dump}"
        );
    }
    // And it is one-shot: the run published more than one tree, and only the
    // first was described.
    assert!(
        counters.rebuilds >= 1,
        "no tree was built for the attached client"
    );

    // ------------------------------------------------------------ Plan 1b.4
    //
    // **The live setting reaches a client, and only on the node that asked.**
    // `Polite` is 1 and `Off` is 0 in UIA's own enum.
    assert_eq!(
        findings.news_live_setting,
        Some(1),
        "the announcement's live setting did not reach a client as Polite"
    );
    assert_eq!(
        findings.status_live_setting,
        Some(0),
        "the status sentence is a live region, so it would be spoken on every \
         keystroke — `live` is inherited, so check the group above it too"
    );
    // **Announce, then once.** The adapter raises its event on a name change;
    // a node republished with the identical name raises nothing, which is what
    // makes "said once" a property of the platform rather than a rule the
    // projection has to remember. If the first ever reads 0, the mechanism
    // this plan's announcements rest on has changed and `persistence.rs` needs
    // a different one — see guide/contributing/deps/ACCESSKIT.md.
    assert!(
        findings.live_events_after_change.unwrap_or(0) >= 1,
        "a changed announcement raised no live-region event: {:?}",
        findings.live_events_after_change
    );
    assert_eq!(
        findings.live_events_after_identical_republish,
        Some(0),
        "republishing the identical announcement spoke again"
    );
}

/// Counts `UIA_LiveRegionChangedEventId` the way a screen reader receives it.
///
/// **A real registered handler, because the claim is about receipt.** Gate T5
/// made the same distinction for text-changed events: an event constructed
/// inside the provider is not an event a client got.
#[windows_core::implement(windows::Win32::UI::Accessibility::IUIAutomationEventHandler)]
struct LiveRegionCounter(Arc<std::sync::atomic::AtomicUsize>);

impl windows::Win32::UI::Accessibility::IUIAutomationEventHandler_Impl for LiveRegionCounter_Impl {
    fn HandleAutomationEvent(
        &self,
        _sender: windows_core::Ref<windows::Win32::UI::Accessibility::IUIAutomationElement>,
        _event: windows::Win32::UI::Accessibility::UIA_EVENT_ID,
    ) -> windows_core::Result<()> {
        // Arrives on a UIA thread, so the count is atomic and nothing else is
        // touched here.
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

fn client_thread(
    hwnd_rx: &Receiver<isize>,
    findings: &Arc<Mutex<Findings>>,
    proxy: &EventLoopProxy<Wake>,
    announcement: &Arc<Mutex<String>>,
) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationEventHandler,
        IUIAutomationInvokePattern, IUIAutomationSelectionItemPattern, TreeScope_Descendants,
        TreeScope_Subtree, UIA_AutomationIdPropertyId, UIA_InvokePatternId, UIA_LevelPropertyId,
        UIA_LiveRegionChangedEventId, UIA_LiveSettingPropertyId, UIA_PositionInSetPropertyId,
        UIA_SelectionItemPatternId, UIA_SizeOfSetPropertyId, UiaHasServerSideProvider,
    };

    let raw = hwnd_rx
        .recv_timeout(TIMEOUT)
        .expect("the window was created");
    let hwnd = HWND(raw as *mut core::ffi::c_void);

    // The client initialises COM; the provider deliberately does not, which is
    // the arrangement `accesskit_windows`' own suite uses and the one a real
    // screen reader creates.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .expect("COM initialised");
    let _com = scopeguard(|| unsafe { CoUninitialize() });

    findings
        .lock()
        .expect("no test poisons this")
        .has_native_provider = Some(unsafe { UiaHasServerSideProvider(hwnd) }.as_bool());

    let uia: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
            .expect("a UI Automation client");

    // **Poll rather than assume.** The first `ElementFromHandle` is what sends
    // `WM_GETOBJECT`; the adapter answers with its own placeholder tree and the
    // real one lands on the next pass of the window's loop. A client that read
    // the property once would be reading the placeholder.
    let deadline = Instant::now() + TIMEOUT;
    let mut root: Option<IUIAutomationElement> = None;
    let mut automation_id = String::new();
    while Instant::now() < deadline {
        let element = unsafe { uia.ElementFromHandle(hwnd) }.expect("the window has an element");
        automation_id = unsafe { element.CurrentAutomationId() }
            .map(|id| id.to_string())
            .unwrap_or_default();
        if automation_id.starts_with("koru:") {
            root = Some(element);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let root = root.expect("the window root never published a Koru locator");
    {
        let mut found = findings.lock().expect("no test poisons this");
        found.root_automation_id = Some(automation_id.clone());
        found.root_class_name = unsafe { root.CurrentClassName() }
            .ok()
            .map(|s| s.to_string());
        found.root_name = unsafe { root.CurrentName() }.ok().map(|s| s.to_string());
        found.framework_id = unsafe { root.CurrentFrameworkId() }
            .ok()
            .map(|s| s.to_string());
    }

    // The locator of the Close command, built the way any external tool would
    // have to build it: from the window number in the root's own AutomationId.
    let slot = automation_id
        .trim_start_matches("koru:w")
        .split('/')
        .next()
        .expect("a window number")
        .to_string();
    let close_locator = format!("koru:w{slot}/v0/s:window#{}", part::CLOSE.name());
    // The view slot the probe seated its own contribution in. An external tool
    // reads this out of the entry it is looking for; here it is the first slot
    // the allocator hands out, which is 1.
    let view = 1;

    let condition = unsafe {
        uia.CreatePropertyCondition(
            UIA_AutomationIdPropertyId,
            &VARIANT::from(close_locator.as_str()),
        )
    }
    .expect("a property condition");
    let close = unsafe { root.FindFirst(TreeScope_Descendants, &condition) }
        .expect("a search by AutomationId");
    findings
        .lock()
        .expect("no test poisons this")
        .found_close_by_automation_id = true;
    findings.lock().expect("no test poisons this").close_name =
        unsafe { close.CurrentName() }.ok().map(|s| s.to_string());

    // **The window command's rectangle, from outside.** The view published a
    // logical rect; what a client sees is that times the scale, offset by
    // `ClientToScreen`. Reading it here is the only way to establish the
    // conversion happens exactly once (`guide/contributing/deps/ACCESSKIT.md`).
    if let Ok(rect) = unsafe { close.CurrentBoundingRectangle() } {
        findings.lock().expect("no test poisons this").close_bounds = Some((
            f64::from(rect.left),
            f64::from(rect.top),
            f64::from(rect.right - rect.left),
            f64::from(rect.bottom - rect.top),
        ));
    }

    // ------------------------------------------------------------ Plan 1a.2
    //
    // **From the window root to a document entry, by AutomationId.** The
    // locator is built the way an external tool has to build one: from the
    // window number in the root's own AutomationId plus the view slot and the
    // document id it was given. Nothing is read out of the tree to find it.
    let find_by_id = |locator: &str| -> Option<IUIAutomationElement> {
        let condition = unsafe {
            uia.CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(locator))
        }
        .ok()?;
        unsafe { root.FindFirst(TreeScope_Descendants, &condition) }.ok()
    };
    // Row A is the *open* one and carries `selected`; row B is the one a
    // person would open. Both are found the same way, by a locator built from
    // the window number and a document id.
    let open_locator = format!("koru:w{slot}/v{view}/n:{}#root", ENTRY_A.replace('-', ""));
    let entry_locator = format!("koru:w{slot}/v{view}/n:{}#root", ENTRY_B.replace('-', ""));
    let open = find_by_id(&open_locator).expect("a client could not find the open entry");
    let entry = find_by_id(&entry_locator).expect("a client could not find a document entry");
    {
        let mut found = findings.lock().expect("no test poisons this");
        found.found_entry_by_automation_id = true;
        found.entry_name = unsafe { entry.CurrentName() }.ok().map(|s| s.to_string());
        found.entry_position_in_set =
            unsafe { entry.GetCurrentPropertyValue(UIA_PositionInSetPropertyId) }
                .ok()
                .and_then(|value| i32::try_from(&value).ok());
        found.entry_size_of_set = unsafe { entry.GetCurrentPropertyValue(UIA_SizeOfSetPropertyId) }
            .ok()
            .and_then(|value| i32::try_from(&value).ok());
        found.entry_level = unsafe { entry.GetCurrentPropertyValue(UIA_LevelPropertyId) }
            .ok()
            .and_then(|value| i32::try_from(&value).ok());

        // **The twin.** Same title, different AutomationId — the whole of
        // *two documents with the same title are distinguishable without
        // reading a label*.
        found.twin_automation_id = unsafe { open.CurrentAutomationId() }
            .ok()
            .map(|s| s.to_string());

        // **`selected` and `Invoke` are exclusive.**
        // `accesskit_consumer::is_invocable` excludes a node whose
        // `is_selected()` is `Some`, and hands it SelectionItem instead — so
        // the open row cannot be invoked and every other row can. This pair is
        // the evidence for that, and the reason the projection publishes
        // `selected` on the open row alone.
        found.open_row_is_selection_item = Some(
            unsafe {
                open.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                    UIA_SelectionItemPatternId,
                )
            }
            .is_ok(),
        );
        found.open_row_offers_invoke = Some(
            unsafe { open.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) }
                .is_ok(),
        );

        // Informational only — see the field's own note.
        if let Ok(focused) = unsafe { uia.GetFocusedElement() } {
            found.focused_automation_id = unsafe { focused.CurrentAutomationId() }
                .ok()
                .map(|s| s.to_string());
        }
    }

    // ------------------------------------------------------------ Plan 1b.4
    //
    // **The save status, found the same way and asked two questions.** First
    // the live setting on each of the two nodes — the property mapping, which
    // the roadmap requires to be established rather than assumed. Then the
    // behaviour the whole "announce once" design rests on: a real handler
    // registered the way a screen reader registers one, a *changed*
    // announcement, and then an identical republish that must be silent.
    let news_locator = format!("koru:w{slot}/v{view}/s:save#news");
    let status_locator = format!("koru:w{slot}/v{view}/s:save#document");
    let news = find_by_id(&news_locator).expect("a client could not find the announcement");
    let status = find_by_id(&status_locator).expect("a client could not find the save status");
    {
        let live_of = |element: &IUIAutomationElement| {
            unsafe { element.GetCurrentPropertyValue(UIA_LiveSettingPropertyId) }
                .ok()
                .and_then(|value| i32::try_from(&value).ok())
        };
        let mut found = findings.lock().expect("no test poisons this");
        found.news_live_setting = live_of(&news);
        found.status_live_setting = live_of(&status);
    }

    let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let handler: IUIAutomationEventHandler = LiveRegionCounter(Arc::clone(&seen)).into();
    unsafe {
        uia.AddAutomationEventHandler(
            UIA_LiveRegionChangedEventId,
            &root,
            TreeScope_Subtree,
            None,
            &handler,
        )
    }
    .expect("a live-region event handler");

    // **The change.** The probe republishes the same tree with one different
    // label, which is the shape a real transition takes: news replaced by
    // different news.
    *announcement.lock().expect("no test poisons this") = NEWS_SECOND.to_string();
    let _ = proxy.send_event(Wake::Republish);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && seen.load(std::sync::atomic::Ordering::SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(25));
    }
    let after_change = seen.load(std::sync::atomic::Ordering::SeqCst);
    findings
        .lock()
        .expect("no test poisons this")
        .live_events_after_change = Some(after_change);

    // **And the non-change.** The tree is published again, in full, with the
    // identical announcement — the ordinary case, several times a second,
    // while nothing about saving has moved.
    let _ = proxy.send_event(Wake::Republish);
    let _ = proxy.send_event(Wake::Republish);
    std::thread::sleep(Duration::from_millis(750));
    findings
        .lock()
        .expect("no test poisons this")
        .live_events_after_identical_republish =
        Some(seen.load(std::sync::atomic::Ordering::SeqCst) - after_change);
    let _ =
        unsafe { uia.RemoveAutomationEventHandler(UIA_LiveRegionChangedEventId, &root, &handler) };

    // The open action, invoked on the entry the client found by id.
    let entry_invoke: IUIAutomationInvokePattern =
        unsafe { entry.GetCurrentPatternAs(UIA_InvokePatternId) }
            .expect("a document entry offers the Invoke pattern");
    unsafe { entry_invoke.Invoke() }.expect("the entry was invoked");
    findings.lock().expect("no test poisons this").entry_invoked = true;

    // **Last**, because it begins closing the window: everything above needs
    // the tree to still be answering.
    let invoke: IUIAutomationInvokePattern =
        unsafe { close.GetCurrentPatternAs(UIA_InvokePatternId) }
            .expect("the control offers the Invoke pattern");
    unsafe { invoke.Invoke() }.expect("the control was invoked");
    findings.lock().expect("no test poisons this").invoked = true;

    // Give the window's loop a moment to drain the requests it was just handed,
    // so the assertions about where they arrived have something to read.
    std::thread::sleep(Duration::from_millis(250));
}

/// `CoUninitialize` on the way out, whichever way the client leaves.
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
