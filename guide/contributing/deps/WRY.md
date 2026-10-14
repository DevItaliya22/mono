# wry — a native WebView as a child of our window

`wry 0.56.1`, pinned exactly (the crate calls its API unstable), linked by
`crates/koru-webview` and by nothing else. The probe that found what follows,
`tools/webview-spike`, was deleted when the tree was prepared for open source;
the macOS findings are measured in the product's own support window. What it is *for* is
ADR-276; this page is what it does that its
documentation does not say, found by the spike of 2026-09-05 (its findings
were folded here and into [INTERCOM.md](INTERCOM.md) when the plan closed).
The macOS run appends its own section here.

## The shape

`WebViewBuilder::new_with_web_context(&mut WebContext::new(Some(dir)))
.build_as_child(&window)` takes anything that implements
`raw_window_handle::HasWindowHandle` — our `winit 0.30` window, no adapter —
and creates a child HWND (Windows) or an `NSView` subview (macOS). Bounds are
`Rect { position, size }` in `dpi::Position`/`dpi::Size`; the product hands
physical pixels and lets the host do the one conversion.

## What the documentation does not say

- **Creation blocks the calling thread.** On Windows `build_as_child`
  creates the WebView2 environment and controller through COM callbacks and
  pumps messages until each returns: 558 ms cold (fresh data directory),
  262–321 ms warm, measured on the UI thread with a watchdog thread
  confirming the stall from outside. Nothing after creation stalls — page
  load and first paint ran with no late wake. Draw a frame first.
- **`WebContext`'s data directory is honoured on Windows** (`dir/EBWebView/`)
  and nothing lands in `%LOCALAPPDATA%`. On macOS WKWebView ignores it;
  the data store is chosen by `with_data_store_identifier` (macOS 14+) or is
  the default per-bundle store — the companion plan's M4 proves which of
  the `--root` promises that keeps.
- **`PageLoadEvent` is `Started` and `Finished` only.** There is no error
  variant; a page that never loads produces nothing, so readiness has to be
  a deadline plus a signal the page itself sends.
- **IPC carries provenance.** `with_ipc_handler` receives a `Request<String>`
  whose URI is the sending frame's; the product believes a message only from
  the support page's origin.
- **The navigation handler sees top-level navigations.** Frames the
  Messenger embeds and the resources it fetches are not navigations; the
  three are different lists.
- **`clear_all_browsing_data` returns `Ok` when the clear has *started*** on
  Windows; the completion result is discarded inside wry. A caller that needs
  the data gone drops the surface and removes the directory.
- **`reparent` exists** on both platforms (`WebViewExtWindows`,
  `WebViewExtMacOS`) but the product does not use it: the chat never moves
  between windows.
- **`set_event_handler` on muda** — the popup menu library wry's sibling
  project ships — replaces muda's receiver; see `menus.rs`.
- **The parent may be reported unfocused** by winit the instant the child is
  created (2 of 5 spike runs), and focused again seconds later with no
  input. Focus is a query the product answers from the platform
  (`GetFocus`/`IsChild`; the macOS first responder), never from visibility.
- **Devtools** are compile-time on macOS release builds (`devtools`
  feature); on Windows they follow debug assertions.

## New transitive dependencies

`webview2-com 0.38`, `webview2-com-sys`, `webview2-com-macros`,
`windows 0.61`, `windows-core 0.61` and their helpers on Windows;
`objc2-web-kit 0.3` on macOS. None were in the lock file before.
