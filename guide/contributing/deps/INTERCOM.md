# Intercom's Web Messenger — what its documentation does not say

The support chat is Intercom's Messenger loaded from the marketing site's
`/[locale]/support/desktop` page into a native WebView
(ADR-276; the page lives with the website,
not in this repository). Everything below was
found by experiment on 2026-09-05, with a configured Intercom workspace, widget
served from `https://widget.intercom.io/widget/<app id>`, in WebView2
152 and in Chrome driven over the debugging protocol. Each item cost time
nobody should spend twice.

## The workspace decides whether the Messenger draws at all

- **An origin not on the workspace's allowed domains gets a Messenger that
  boots and never draws.** The widget script loads, `boot` runs,
  `onUnreadCountChange` fires with the count, `Intercom.booted` is true —
  and the DOM holds only a 1 × 1 `intercom-frame`. The one warning, *"This
  domain is not allowed for the Intercom Messenger. Add your domain here:
  https://app.intercom.com/a/apps/_/messenger"*, is written **inside
  Intercom's own frame**, so a console hook on the page's top frame never
  sees it. Each development and production origin must be added to the
  workspace's allowed domains before testing the integration.
- **Headless Chrome is not a counter-check.** The widget does not boot under
  automation flags at all; the experiments used a headed Chrome positioned
  off-screen.
- **The Home space is whatever the workspace configures.** With no greeting
  or cards, Home is a blank panel with a single *Chat with us* button; that
  is not a layering problem on our side. Content for Home lives in Intercom
  under Messenger settings.

## Layout

- **The Messenger is a fixed 400 × 704 panel with 20 px margins and 24 px
  corners at any page width of 460 CSS px or more**, and switches to its
  full-viewport layout (inset 0, no radius) at 450 px and narrower. There is
  no setting for it, and `horizontal_padding` / `vertical_padding` only move
  the launcher. The page pins `.intercom-messenger-frame` to the viewport
  with one `!important` rule; that class is Intercom's, so a widget release
  that renames it brings the panel back without breaking the chat. The
  desktop window is 440 wide for the same reason: the Messenger's own width.

## The JavaScript object is replaced after boot

- **The `window.Intercom` captured when the widget script loaded still
  delivers callbacks but silently drops a later `show`.** The widget swaps
  the function after boot. Every call the page makes after boot goes through
  a fresh `window.Intercom` lookup; the page's re-show never worked until it
  did. Registering `onShow`, `onHide` and `onUnreadCountChange` through the
  early reference is fine.
- **A `show` that lands during the hide animation (~100–400 ms in) is
  ignored;** one at once or after it is honoured. The page calls `show`
  immediately on `onHide` and again after a second if still hidden.

## The header's X, and hiding

- **Nothing removes the X in the Messenger's own header.** The page treats
  `onHide` as "bring it back": the chat is on screen again within about
  30 ms, so the X is as good as disabled. Expect a blink.
- **A Messenger that Intercom believes is on screen marks every reply read
  the moment it arrives.** Inside a window that is hidden (not closed), the
  unread count therefore falls straight back to zero and any badge built on
  it vanishes. The page exposes `koruSupport.hide()` / `show()`, which call
  Intercom's `hide` / `show` and tell the `onHide` re-show to stand down;
  the native side calls them when the chat window hides behind the icon and
  when it is shown again. While hidden this way `onUnreadCountChange` still
  fires on each reply.

## What the page exposes to the native side

`window.koruSupport = { show, hide, shutdown }`; messages over
`window.ipc.postMessage` (in a plain browser, `console.info('[koru-support] …')`):
`{"ready":true}` from `onShow`, `{"unread":n}` from `onUnreadCountChange`,
`{"hidden":true}` from `onHide`, `{"error":"…"}` for a missing or failed
widget script. `shutdown` is for the signed-in phase's logout; End chat does
not call it, so the anonymous visitor and the history survive.

## What a visitor cannot do

Delete a conversation, or sign out of an anonymous one. Closing happens
from the inbox.
