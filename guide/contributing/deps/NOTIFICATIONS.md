# Native desktop notifications

Settings tone preview on Windows uses one process worker with a single pending
latest request. The UI queues a tone; WAV reads and `PlaySoundW` happen on that
worker. A later preview stops prior playback before replacing its retained
buffer (`SND_MEMORY | SND_ASYNC` borrows it after the native call returns).
The queue result means accepted, not successful playback. Other platforms
omit the preview control until an adapter exists; tone selection is still
available. Actual playback and packaging remain native acceptance checks.

The phase-one reminder delivery boundary is `koru-native-notifications`. It is
a vocabulary-only leaf: the Agent scheduler resolves an instant and supplies
display content on its worker; the adapter registers, replaces, withdraws, and
reports capability. The adapter owns no reminder rule, profile, target, clock,
or UI. Tests use a fake implementation of the same trait.

This page separates API evidence from runtime evidence. An API existing is not
proof that Koru's signed package shape activates the right executable.

## Standing contract

- `schedule` is replace-by-stable-registration-id, even where the native API
  needs an explicit remove followed by add.
- `withdraw` removes both pending and already-delivered notification-center
  state for that id.
- Every selected Agent root has a stable, content-free native namespace: a
  root path is hashed before it reaches an OS identifier. Exact withdrawal and
  corruption recovery therefore cannot touch another `--root` rehearsal.
- A missing or malformed ledger is not enough to name obsolete OS entries.
  Recovery clears that root's pending and delivered namespace before a full
  Store rebuild; a future ledger schema still fails closed without clearing or
  overwriting it.
- `capability` distinguishes current authorization from static support for
  actions, replacement, withdrawal, custom sound, and stopped activation.
- `delivery_support` reports only static package/platform facts and performs no
  notification-service call. Ledger recovery checks it before namespace reset,
  so the deliberately
  unsupported interim cannot turn an unregistered Windows AUMID into an idle
  retry loop; Settings still uses `capability` for truthful live inspection.
- `send_test` uses the real native path and writes no reminder/profile state.
- `Sound::Silent` is an explicit native instruction: Windows writes a silent
  audio element and macOS omits `UNNotificationSound`. `ActionSet::OpenOnly`
  distinguishes count-only missed summaries from ordinary reminder actions.
- A missed summary carries only its localized count. Its stable registration
  resolves to a profile in the content-free ledger; item titles, bodies, and
  targets never enter the summary request or diagnostics. The scheduler marks
  included occurrences summarized only after native registration succeeds.
- The activation envelope contains only its schema, registration id, and
  `open | snooze | handled`. Windows wraps that platform-neutral envelope with
  the selected root's 16-hex content-free namespace so the one channel broker
  can distinguish equal registration ids from two roots. The Agent resolves
  the `(namespace, registration)` pair through private device state and then
  reuses private `Router::open(Target)` navigation.
- Accepting an Open or Snooze gesture into the Agent's bounded activation queue
  transfers ownership of that gesture to Koru. A transient Store failure or an
  updating workspace projection retains it on the reminder worker and retries
  only after authoritative reconciliation succeeds; it is not relabelled as a
  stale notification or silently consumed. Definitively missing, handled,
  revised, or mismatched occurrences remain terminally stale. Handled is not
  replayed after an ambiguous Store mutation because retrying a mutation is a
  different idempotency contract.
- Native Open navigates without changing occurrence state. Native Snooze first
  navigates to the exact reminder and then opens Koru's own duration picker;
  the toast never guesses a duration or mutates the occurrence by itself. The
  Agent sends one durable Open followed by one semantic Snooze handoff. Koru
  retains that ordering without synthesizing a second navigation, because a
  navigation after the semantic handoff closes the picker it just opened.
  The desktop host's `ManagerView` wrapper explicitly forwards the semantic
  ask into its `MainView` container. A default `false` at that wrapper boundary
  silently consumes an otherwise valid Windows gesture, so a completeness
  regression covers every reminder-related `View` method implemented by the
  container.
- Native reminder titles are presentation text, not raw command state. The
  projection removes a bounded `/remind` command suffix that it can briefly
  observe between editor submission and exact-range cleanup, so implementation
  syntax never appears in a Windows or macOS notification.
- Scheduler work and the live event tail's shutdown wait use separate condition
  variables. A notification-action wake is therefore consumed only by the
  scheduler that owns the activation queue; it cannot be lost to the
  shutdown-only waiter and sleep behind the next reminder deadline.
- Native calls may block and therefore run on the Agent worker, never winit.
- Native triggers receive an absolute Unix-millisecond instant. Floating wall
  time is resolved and repaired by reconciliation, not delegated to an opaque
  calendar trigger.

## Windows evidence and constraints

Windows delivery uses the locked WinRT `Windows.UI.Notifications` surface. It
implements setting inspection, scheduled toast add/remove, delivered-history
removal, root-hash-namespaced tag/group replacement and namespace reset,
localized foreground Open/Snooze/Handled actions, and system or package-relative
sound. The release adds a signed, channel-specific package-with-external-location
identity rather than bringing the Windows App SDK runtime into the process.
The identity-only `koru-identity.msix`, the external `Assets/` logos, the Agent,
the one-shot `koru-com-notification.exe` broker, and the ten
`assets/reminders/` WAV files are updater-owned inventory.

Keep the Koru registration id separate from Windows' Tag and Group fields.
Koru's content-free registration id remains up to 64 ASCII characters in the
ledger and activation envelope. The Windows adapter derives a stable 16-hex
Tag from it and uses the selected root's existing 16-hex namespace as Group;
schedule, replacement, withdrawal, and test delivery must all use those same
helpers. A signed Windows 11 build rejected a 64-character scheduled-toast Tag
with `0x803E0116` even though the general Toast Tag documentation describes a
64-character limit on post-Creators-Update systems. Treat 16 as the reliable
scheduled-notification boundary rather than passing a domain id through
directly. The full registration id still travels only in the bounded opaque
activation envelope, so shortening Windows' lookup token does not change
ledger resolution or action routing.

The Agent registers or updates that MSIX for the current user with the stable
install directory as `-ExternalLocation`, then restarts once so `Package.Current`
is real for the new process. A content-free device-local stamp binds package
version to a hash of that canonical directory; a missing or mismatched stamp
forces registration again, including after a same-version portable move, and is
written only after registration succeeds. One manifest COM class belongs to a
dedicated, short-lived `koru-com-notification.exe`, never to the long-running
Agent. It handles both already-running and stopped activation.
The package gives that broker its own `KoruNotification` application identity,
separate from the long-running Agent's `KoruAgent` identity. Notifications are
created and withdrawn explicitly against the broker AUMID. A separate
executable with the Agent's AUMID is not a separate lifetime: Windows tears
down the existing application instance before COM activation reaches `main`,
leaving no broker log, Agent clean-stop, or crash record and resetting every
Store connection.
A private channel-local map resolves a `(namespace, registration)` pair to either
that Agent or an explicit `--root`; a rooted Agent never tries to register the
same COM class. A malformed rebuildable map refuses activations, but the next
route mutation copies it to a bounded quarantine and starts a fresh map; a future
schema remains fail-closed. The native toast contains only the bounded opaque
namespace/registration/action envelope, so no root, profile, entity target,
title, or authored content enters the toast or COM activation arguments. After
the broker's private lookup, an explicit root enters only the rooted Agent's
strict `--root <path> notification-activation <opaque-envelope>` spawn argv.

Runtime support is deliberately conjunctive. Actions and stopped activation are
advertised only when the process has package identity and the COM broker is
installed beside it;
custom sound additionally requires every packaged WAV beside the executable.
Replacement and withdrawal remain available through tag/group APIs. An ordinary
unsigned cargo build has no sparse identity and continues to fail closed rather
than claiming native delivery:

- [desktop activation and unpackaged COM registration](https://learn.microsoft.com/en-us/windows/apps/develop/notifications/app-notifications/app-notifications-dotnet)
- [scheduling and cancellation](https://learn.microsoft.com/en-us/windows/apps/develop/notifications/app-notifications/app-notifications-scheduled)
- [tag/group removal](https://learn.microsoft.com/en-us/windows/apps/develop/notifications/app-notifications/manage-app-notifications)
- [permission/block setting](https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.windows.appnotifications.appnotificationmanager.setting)
- [custom audio](https://learn.microsoft.com/en-us/windows/apps/design/shell/tiles-and-notifications/custom-audio-on-toasts)

The scheduled-toast constructor takes a `DateTimeOffset`, so Koru registers an
instant. A device-zone change does not reinterpret that native value. Microsoft
also documents that Windows drops a scheduled notification when the device is
offline for more than five minutes around its delivery time; native scheduling
is therefore a wake/delivery aid, not Koru's durable due-state authority. Agent
startup and resume reconciliation applies the reminder's agreed missed-delivery
policy and replaces or delivers work from Koru state after that native window.

Koru still wakes and reconciles on `WM_TIMECHANGE`, because a floating reminder
must be replaced with a newly resolved instant. Resume is `WM_POWERBROADCAST` /
`PBT_APMRESUMEAUTOMATIC`; cold start and resume re-read the zone fingerprint so
correctness never depends on receiving either message. Microsoft's resume
contract is documented at
[PBT_APMRESUMEAUTOMATIC](https://learn.microsoft.com/en-us/windows/win32/power/pbt-apmresumeautomatic).

Custom audio uses `ms-appx:///assets/reminders/<inventory filename>`; an absolute
drive path never enters toast XML. MakeAppx has one sparse-package trap: normal
semantic validation insists that every manifest reference be present in its
input, while the final identity-only package must omit precisely those external
files. Packaging therefore first runs MakeAppx without `/nv` against a temporary
mirror of the real external COM broker and logo layout. Only after that succeeds does
it pack the manifest-only sparse MSIX with `/nv` and sign it. `/nv` is an encoding
necessity for the final external-location package, not the validation evidence.
The corresponding explicit SDK test is ignored by the default suite because it
writes a temporary package and requires MakeAppx.

The OS-launched COM broker registers one class factory, accepts one callback,
forwards the opaque activation over the selected Agent's private rendezvous, and
exits. If no Agent is listening, it starts the sibling `koru-agent.exe` with the
same strict `notification-activation <opaque-envelope>` bootstrap (and `--root`
only for an explicitly routed root); that Agent then becomes the ordinary
long-lived scheduler. Windows may therefore end the COM process after the
callback without ending the Agent, tray, or scheduler. It never starts a second
profile. Only the content-free namespace/registration/action envelope may cross
native activation; the entity target still enters the existing private router
and never appears in argv.

An Open or Snooze click also gives the one-shot COM broker the foreground
capability associated with that user gesture. Before forwarding the activation,
the broker passes that short-lived permission on with
`AllowSetForegroundWindow`; the Agent then uses the existing authenticated-shell
route to foreground the exact Koru process. A background Agent cannot grant
that privilege to itself after the broker exits. The broker's
`foreground_granted` field records whether Windows accepted the handoff.

### Activation trace

The native Open/Snooze/Handled path is observable end to end without recording
reminder text. Every line uses the content-free registration id until Agent
resolves it, then the stable profile/workspace/document/node ids used by the
existing router:

1. Agent logs when Windows accepts the scheduled delivery registration.
2. The COM broker logs when Windows invokes its callback, the decoded action,
   route-map result, forward attempts, and whether it reached a running or newly
   started Agent.
3. Agent logs queue acceptance, worker resolution start, transient deferral,
   authoritative retry, the resolved target, and the winit routing handoff.
4. `agent/routing.log` records target selection, launch/deliver choice, window
   foreground state, and whether the Koru IPC push was queued.
5. Koru logs IPC receipt, window-loop and host drains, Store location and
   document-load time, pane presentation, deferred-document state, and the final
   caret row and byte offset.

`koru-agent.log.<date>`, `koru.log.<date>`, and `agent/routing.log` live under the
selected root used by a rooted rehearsal. `koru-com-notification.log.<date>`
lives under the channel Store log directory because the one COM executable has
not learned which content-free namespace/root Windows supplied until after its
logger must be initialized. Timestamps and the registration/target ids join the
files without copying authored content into diagnostics.

Windows does not expose a callback when a scheduled toast merely becomes
visible. `Windows accepted reminder delivery registration` is therefore the
last authoritative Koru-side line before display; `Windows invoked the
notification COM callback` is the first authoritative line for a person's
click. The interval between those two includes the intended reminder wait and
must not be interpreted as navigation latency.

`open_settings` launches `ms-settings:notifications`; Microsoft documents that
URI under [Windows Settings launch URIs](https://learn.microsoft.com/en-us/windows/apps/develop/launch/launch-settings).

## macOS evidence and constraints

The supported direction is UserNotifications through
`objc2-user-notifications`, which is the focused generated binding for the
framework Koru already reaches through the objc2 family. Apple documents:

- local delivery while the app is not running, actions, and pending-request
  removal in [Scheduling a notification locally](https://developer.apple.com/documentation/UserNotifications/scheduling-a-notification-locally-from-your-app)
- same-identifier replacement in
  [`UNNotificationRequest.identifier`](https://developer.apple.com/documentation/usernotifications/unnotificationrequest/identifier)
- authorization inspection through
  [`UNNotificationSettings`](https://developer.apple.com/documentation/usernotifications/unnotificationsettings)
- system and named custom sounds in the main bundle, WAV support, and the
  under-30-second limit in
  [`UNNotificationSound`](https://developer.apple.com/documentation/usernotifications/unnotificationsound)
- the zone-change wake in
  [`NSSystemTimeZoneDidChangeNotification`](https://developer.apple.com/documentation/foundation/nsnotification/name-swift.struct/nssystemtimezonedidchange)

Koru should use `UNTimeIntervalNotificationTrigger` computed from the resolved
instant rather than `UNCalendarNotificationTrigger`. The latter would hand
floating/travel semantics to the OS; the former makes replacement after a
fingerprint change explicit and matches Windows. Workspace session-active and
wake notifications are wake-ups only; cold start/resume still reconcile.

Process identity is now explicit in the package:
`Koru.app/Contents/Library/LoginItems/Koru Agent.app` is a separately
identified, background-only notification owner with bundle id
`do.koru.desktop.agent.notifications`. The packager builds its narrow binary,
stages its own `Info.plist` and ten sounds, signs the executable and nested app
before sealing the outer app, and requires the nested identity files in release
inventory v5. All ten nested tone paths are mandatory in that inventory too,
so an incomplete update cannot validate by omitting a tone from its manifest.
This establishes the package mechanism; a stopped-action launch
still needs the signed macOS matrix in Q-P0-2 and is not proved by the Windows
cross-compile.

The macOS adapter schedules, replaces, withdraws, and sends silent, system, or
bundled sound only when authorization is already allowed; none of those operations
prompts. A separate typed worker `RequestAuthorization` operation is the sole
prompt seam (Q-P5-1). In unpackaged builds, actions and stopped activation
remain false. A packaged Agent advertises them only when the
exact nested owner executable, its `Info.plist`, and the outer Agent are all
present; live capability also requires the owner to answer the bounded
inspection request. Its request identifiers carry the same content-free root
hash used by Windows; corrupt-ledger recovery enumerates and removes only that
prefix. The
locked `objc2-user-notifications` 0.2.2 binding requires the older
objc2 0.5/Foundation 0.2 family; this duplicates the current objc2 0.6 family
already in the graph and should be collapsed when a compatible generated
binding is available.

Agent-to-owner operations are one bounded (16 KiB) JSON request and response
over the helper's private stdin/stdout. The schedule request carries the
serialized `Sound` and `ActionSet`: ordinary reminders select the localized
Open/Snooze/Handled category, while quiet summaries select a distinct localized
Open-only category and omit sound. Unknown fields and modes are refused;
typed permission/platform failures come back as protocol values rather than
being flattened into process-exit failure. Scheduling writes a private
`Default | Root(UTF-8 path)` namespace route under the person's Application
Support directory with directory/file modes `0700`/`0600`. The native
notification carries only its strict `koru.<root hash>.<RegistrationId>`
identifier and category: no root is placed in UserNotifications `userInfo` or
the OS launch argv. On activation the owner requires that route to exist and
decode; missing or corrupt state fails closed instead of launching the default
Agent. It then starts `koru-agent notification-activation <opaque> --root
<root>`; the Agent's existing strict bootstrap forwards to the selected
singleton or becomes it, and the entity target remains ledger-resolved rather
than entering argv.

## Tone evidence

`assets/reminders/` is canonical source. The explicit
`BUNDLED_TONES` inventory gives the model and packager one stable list, and its
tests compile all ten source files and verify PCM WAV plus the shared Apple
duration constraint. Piece 5 stages the exact names under
`assets/reminders/` on Windows and `Contents/Resources/assets/reminders/` on
macOS, preserving Koru's external-before-compiled asset lookup rule.

## Evidence status after Piece 5 implementation

| Evidence | Windows host | signed macOS host |
| --- | --- | --- |
| API capability and trigger semantics | official API contracts inspected | official API contracts inspected |
| adapter contract / activation privacy | compiled and unit-tested, including exact foreground envelopes and private rooted routing; signed runtime unproved | adapter cross-compiled; signed callback path unproved |
| ten source WAV encoding/duration | compiled and unit-tested | same source proof |
| replace and withdraw | implemented; manual packaged/unpackaged run still required | implemented; manual signed run still required |
| actions and stopped activation | identity package and dedicated one-shot COM broker implemented; signed running/stopped matrix not run | nested owner, localized categories, private routing, and singleton forwarding implemented; signed stopped/running matrix not yet run |
| system/custom tone audibility | manual run still required | manual signed run still required |
| zone-change and resume repair | winit message hook compiled; manual matrix outstanding | Foundation/AppKit observers compiled and cross-checked; manual matrix outstanding |

Implemented rows awaiting a manual run are not silently promoted to “proved.”
Windows actions remain capability-gated. macOS package capability is gated on
the exact nested-app geometry and a live owner reply; that says the implementation
is installed, not that the outstanding signed runtime matrix has passed.
The manual evidence is the explicit Piece 5 release matrix in the runbook.
