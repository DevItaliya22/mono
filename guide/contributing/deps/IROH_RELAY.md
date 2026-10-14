# iroh-relay (the server library)

What the `iroh-relay` crate's *server* half actually provides, established for
Plan 1.1 of the relay roadmap. `apps/koru-relay`
wraps this library; `crates/koru-relay-test-support` starts it in-memory for
tests. Version: the workspace declares `iroh = "1.1"` / `iroh-relay = "1.1"`,
resolved by the lockfile to the 1.1.0 pair. The `server` feature gates the
whole `server` module (and pulls in metrics, TLS-ring, ACME, clap).

**Verified by experiment** (the tests in `koru-relay-test-support`,
`koru-relay`, and `koru-network-iroh` run these): plain-HTTP relaying on
localhost, `/healthz`, graceful shutdown, teardown-on-drop, and relay-only
endpoint connections reporting relay paths. **Read in source, not run**:
everything involving ACME/TLS, QUIC address discovery, metrics serving and
rate limits — repository tests do not touch TLS, UDP discovery, or the
network, so those sections cite the code they were read from
(`iroh-relay-1.1.0` and `tokio-rustls-acme-0.9.1` as resolved by our
lockfile).

## One server instance serves both hostnames (the maintainers-blocking question)

**Yes — one instance, one certificate, both names.** The chain, from source:

- The server config takes `CertConfig::LetsEncrypt { acme_config, .. }` with
  `AcmeConfig::domains(Vec<String>)` — a *list*. The shipped `iroh-relay`
  binary exposes the same as `tls.hostname`, deserialized string-or-sequence
  (`src/main.rs:409`).
- The list is passed whole to `tokio_rustls_acme::AcmeConfig::new_with_client_tls_config(domains, …)`
  (`iroh-relay/src/server.rs`, the `LetsEncrypt` arm). tokio-rustls-acme
  0.9.1 places **one ACME order naming every domain as an identifier** and
  builds the CSR with `rcgen::CertificateParams::new(config.domains)` — all
  names become subjectAltNames of a **single certificate**
  (`tokio-rustls-acme/src/state.rs:250-256`, `src/acme.rs:127-130`).
- SNI at serving time is then trivial: the resolver answers **every**
  non-challenge ClientHello with that one certificate, whatever the SNI says
  (`tokio-rustls-acme/src/resolver.rs` — `resolve()` returns `inner.cert`
  unconditionally outside the challenge path). Clients validate against the
  SAN list, so both `dev1.relay.koru.do` and `use1-1.relay.koru.do` are
  covered by the same handshake.

Operational consequences to respect:

- **Validation is TLS-ALPN-01 only** (`AcmeError::NoTlsAlpn01Challenge`
  otherwise). Each name is validated by a TLS handshake with ALPN
  `acme-tls/1` **to port 443 of the host that name resolves to**. Both DNS
  records must point at the relay host and 443 must be reachable before the
  certificate can ever be issued. No HTTP-01, no DNS-01. Challenge SNI is
  answered per-domain from `resolver.set_auth_key(domain, …)`.
- **The cache file is keyed by the hash of the whole domain list** (plus
  directory URL) — `cached_cert_{hash}` in the ACME cache directory
  (`tokio-rustls-acme/src/caches/dir.rs:51`). Adding, removing, or reordering
  a hostname makes the cached certificate invisible and orders a fresh one.
- Renewal is automatic at one third of validity remaining
  (`state.rs:214`), driven by the ACME event stream the server spawns; if
  that stream ever ends the whole server supervisor exits
  (`SupervisorError::AcmeEventStreamFinished`) — the process is expected to
  be restarted by its supervisor (compose restart policy), not to limp on
  without renewal.

## Health endpoint

Ships `/healthz`: `200 OK`, JSON `{"status":"ok","version":…,"git_hash":…}`
(the library reports `git_hash` as `"unknown"`; only n0's own build injects
one). Verified by experiment over plain HTTP
(`koru-relay-test-support/src/tests.rs`). Also served: `/` (index), `/ping`
(net-report probe), `/robots.txt` (disallow all).

**Trap for the container healthcheck (read in source):** these handlers are
registered on the *relay* server. Without TLS that is the plain HTTP socket;
**with TLS they move to the HTTPS bind**, and the plain-HTTP socket serves a
captive-portal service that knows only `/generate_204` and answers 404 to
everything else (`server.rs`, `run_captive_portal_service`). A deployment
health probe must speak HTTPS or use `/generate_204` on port 80.

## File descriptors and the accept loop

The HTTP serve task (`relay-http-serve` in `http_server.rs`) and the
captive-portal task on port 80 (`run_captive_portal_service` in
`server.rs`) both `listener.accept()` in a `select!` and, on error, **log
and immediately retry**. When the process hits its open-file ceiling
(`Too many open files` / `os error 24` / EMFILE), accept fails instantly,
the listening socket stays readable, and the loop floods the log at
line-rate. Closing the accepted sockets is not something a wrapper can
do: we never see them.

Two properties of 1.1.0 make the ceiling reachable on a public host:

- **Port 80 has no idle timeout.** Captive-portal connections are handed
  to hyper http1 with default keep-alive and no `header_read_timeout`.
  Internet scanners that complete a TCP handshake and stall hold an fd
  until kernel TCP keepalive (hours, unless `SO_KEEPALIVE` is set — it is
  not). Port 443 is better: a 30 s `establish_timeout` aborts connections
  that never finish TLS + the relay WebSocket upgrade.
- **A reconnect does not close the previous socket.** When the same
  endpoint id connects again, the old connection is moved to an
  `inactive` list and kept open (`clients.rs`, iroh#3921) until the
  client drops it or a ping times out. A flapping client therefore holds
  more than one fd.

`apps/koru-relay` raises `RLIMIT_NOFILE` at start; in a container the hard
ceiling must be raised too (`ulimits.nofile` 65535, say — Docker's default is
1024), or the raise has nowhere to go. That stops the outage; it does not close
iroh's sockets. Reclaiming leaked fds still means restarting the
container. The accept-loop backoff belongs in iroh-relay, not a fork.

## Prometheus metrics

`ServerConfig::metrics_addr: Option<SocketAddr>` spawns a **separate** HTTP
server (iroh-metrics `MetricsServer`) on that socket — so binding it to
`127.0.0.1:9090` keeps it private without firewall help; upstream's default
port is also 9090. It answers **any** path (the handler ignores the URI) with
the OpenMetrics/Prometheus text encoding. Counters cover relayed
bytes/packets sent/received/dropped, accepts/disconnects, unique client
keys, rate-limited bytes/connections, QAD connections, and HTTP upgrade
counts (`server/metrics.rs`). The Milestone 6 `RelayMetricsSink` question is
separate: these are host metrics, not product events.

## The auth seam (where a token verifier attaches later)

`RelayConfig::access: Arc<dyn DynAccessControl>` — default `AllowAll`. The
trait to implement is `AccessControl`:

- `async fn on_connect(&self, request: &ClientRequest) -> Access` — called
  once per incoming relay client, *before* the connection is registered.
  Return `Access::Allow` or `Access::Deny { reason: Option<String> }` (the
  reason is sent to the client). It is async, so a verifier may do real work.
- `ClientRequest` supplies exactly what Milestone 6.b needs: `endpoint_id()`
  — **already authenticated by the relay handshake** (the client proves
  possession of the key, the hook never needs to), `auth_token()` — the
  first `Authorization: Bearer` header, falling back to the `?token=` URL
  query parameter (how browser/Wasm clients send it) — plus raw headers,
  URI, and a per-connection `ConnectionId`.
- `fn on_disconnect(&self, endpoint_id, connection_id)` — sync, called
  exactly once per admitted connection (enforced by `OnDisconnectGuard`,
  a drop guard), so per-device connection accounting cannot leak.

Client side, for when tokens exist: `RelayMap::with_auth_token` /
`RelayConfig::with_auth_token` attach the token per relay entry; iroh sends
it as `Authorization: Bearer` on native targets.

## Rate limits

`RelayConfig::limits: Limits`. What works: `client_rx:
Option<ClientRateLimit>` — per-client-connection read throttling,
`bytes_per_second` (NonZeroU32) plus optional `max_burst_bytes`. **What does
not: `accept_conn_limit` and `accept_conn_burst` exist as fields and are
documented in-source as "Not currently implemented, setting this has no
effect"** — do not design an accept-rate defence around them. V1 ships no
limits (decision №1), but the seam is the `Limits` struct plus the access
hook above.

## Graceful shutdown

`Server::shutdown(self)` asks the relay HTTP server, QUIC server, and
metrics server to stop and awaits the supervisor; `Server::join()` waits
without requesting anything. **Dropping the `Server` aborts every task** —
the module is written so tasks cannot outlive their handle. Both paths are
verified by experiment in `koru-relay-test-support` (port stops accepting
after `shutdown().await` and after a bare drop). Signal handling is the
caller's job; `apps/koru-relay` maps Ctrl-C to `shutdown()`.

## QUIC address discovery (QAD)

`ServerConfig::quic: Option<QuicConfig>` — a UDP listener, default port
**7842** (`defaults::DEFAULT_RELAY_QUIC_PORT`), ALPN `/iroh-qad/0`, through
which clients learn their reflexive address. It **requires TLS 1.3**: it
uses `QuicConfig::server_config` if set, otherwise inherits the relay's TLS
config — including the ACME resolver, so the same two-hostname certificate
serves QAD. With no TLS anywhere, spawning fails
(`QuicSpawnError::TlsNotConfigured`); consequently the plain-HTTP test
relay and the current `koru-relay` local mode run without QAD.

## Running it in-memory for tests

Plain HTTP is a first-class mode: `RelayConfig::new(addr)` with `tls: None`
serves the relay over `ws://`, and iroh endpoints accept an `http://` relay
URL directly — no certificates, no `insecure_skip_verify`, nothing touching
disk or DNS. That is `koru-relay-test-support`'s whole configuration, and it
is what lets the relay-path integration tests run offline. iroh's own test
helper (`iroh/src/test_utils.rs`) instead uses a self-signed HTTPS config —
copying that would have needed cert-verification bypasses in every client
under test.

## The client side: what an endpoint knows about relay reachability (Plan 2.2)

Established for the relay-failure diagnostics
(`koru-network-iroh::dial_failure_code`). The question was: after
`Endpoint::connect` fails, can the client tell a dead relay from anything
else? What iroh 1.1.0 actually exposes:

- **The dial error never says.** `ConnectError` bottoms out in
  `SelfConnect` / `NoAddress` / `Noq(QuicConnectError)` / `LocallyRejected` /
  `EndpointClosed` / `InvalidAlpn` plus handshake errors. A relay-only dial
  with the relay down is a generic QUIC timeout; a dial with no addressing
  is `NoAddress`. Nothing in the chain names the relay.
- **`Endpoint::home_relay_status()` is the only stable relay-connection
  state**, and it tracks the *selected home relay only*: at most one entry,
  `Connecting` / `Connected` / `Disconnected { last_error }`.
  `RelayStatus::last_error()` carries the most recent relay connection
  error. (`Watcher::get` takes `&mut self` — bind the watcher mutably or
  call `.get()` on the temporary.)
- **The state enum itself is crate-private** (`RelayConnectionState`,
  `socket/transports/relay/actor.rs`); the public `RelayStatus` exposes only
  `url()`, `is_connected()` and `last_error()`. So *Connecting* cannot be
  told apart from *Disconnected before any attempt has errored* — a consumer
  maps not-connected-with-an-error to disconnected and
  not-connected-without-one to connecting, which is what
  `koru-network-iroh`'s relay status watch does. Exercised by
  `a_relay_that_dies_mid_session_reads_disconnected`: after a mid-session
  relay death the watcher does reach `Disconnected` with a recorded error
  within a couple of seconds.
- **The surprise (read in `iroh-1.1.0` source): a relay that was never
  reachable is never selected as home**, so it never appears in that status
  at all. Home selection is `net_report`'s `preferred_relay`, which is set
  only from *measured* probe latency (`net_report.rs`,
  `add_report_history_and_set_preferred_relay`); a dead relay yields no
  latency, the status list stays empty, and `Disconnected { last_error }`
  only ever appears for a relay that died *after* connecting. Corollary:
  `Endpoint::online()` awaits a connected home relay and therefore hangs
  forever when the only configured relay is down.
- Dial-time connections to a *peer's* (non-home) relay URL never publish
  status either: `ActiveRelayActor` writes through
  `HomeRelayWatch::set_status`, which drops writes for any URL that is not
  the current home relay (`socket/transports/relay/actor.rs`).
- A per-relay probe report (`relay_latency` for every configured relay)
  does exist on `Endpoint::net_report()`, but the whole watcher is gated
  behind the `unstable-net-report` feature with no semver guarantee — not
  used.

Consequence for our diagnostics: the honest, evidence-backed statement a
client can make is "configured to use relays, and holding no live relay
connection when the attempt failed" — which covers both the
never-reachable relay (empty status; that it reads as relay-unreachable is
verified by experiment in
`a_dead_relay_surfaces_the_stable_relay_unreachable_code`) and the
died-after-connecting relay (`Disconnected { last_error }`). That is what
the stable `relay_unreachable` code means; the dial error is deliberately
never the evidence.

## Live relay reconfiguration on the client `Endpoint` (Plan 2.1, revised)

iroh 1.1 grew `Endpoint::insert_relay` / `Endpoint::remove_relay`
(`iroh-1.1.0/src/endpoint.rs:984,998`). Plan 2.1's first implementation
rejected them and **rebound the endpoint** on every switch — which turned
"relay off" into "every connection drops", the maintainers-reported defect: a
desktop and phone connected over a *direct* path lost that connection the
moment relays were toggled off. The revised switch
(`RuntimeState::replace_relay_provider`) edits the live relay map and only
rebinds where iroh leaves no alternative. What iroh 1.1.0's map edits
actually do, verified by experiment (the `remove_relay_*` tests in
`koru-network-iroh`) and by source:

- **`remove_relay` does not drop an established relay-routed connection.**
  The map edit only removes the entry and pokes a net-report re-run
  (`socket.rs`, `handle_relay_map_change` → `re_stun`); the
  `ActiveRelayActor` holding the relay server connection keeps running and
  keeps forwarding — an echo still round-trips seconds later. Off therefore
  requires the runtime to close relay-only connections explicitly.
- **`remove_relay` does not gate new dials either — the surprise.** Datagram
  routing never consults the relay map: sending to a peer whose addressing
  names a relay URL spins up an `ActiveRelayActor` for that URL on demand
  (`socket/transports/relay/actor.rs`,
  `active_relay_handle_for_endpoint`; the map feeds net-report probing and
  per-relay auth tokens only). A fresh dial naming a removed relay URL still
  connects straight through that relay. Off therefore also requires the
  runtime to strip relay addresses from its own dials and to refuse
  relay-only connections at establishment (inbound included: a peer with
  relays still on can reach the endpoint through its old relay
  registration).
- **The home-relay reading goes stale, not away.** With the map emptied, the
  direct-address updater skips net-report entirely (`relay_map.is_empty()`
  short-circuit) and only a `Some(report)` ever reaches the relay actor's
  `on_network_change` — so `home_relay_status` keeps reporting the old
  relay, and the endpoint keeps its (genuinely live) registration to it
  until a real network change. The runtime kills its status watch on the
  off switch and pins `Off`; the lingering relay-server registration is an
  iroh 1.1.0 limitation the map edit cannot reach — no *peer traffic*
  crosses it, but the client stays connected to the relay server itself.
- **`insert_relay` is symmetric and sufficient for on**: entries built
  through the same `RelayMap::try_from_iter` as a fresh bind, inserted with
  `insert_relay(url, config)`, trigger the map-change re-probe; home
  selection and the relay connection follow, no rebind and no connection
  touched.
- The one rebind left: an endpoint built with `RelayMode::Disabled` has
  **no relay transport at all** — `From<RelayMode> for
  Option<TransportConfig>` maps `Disabled` to `None`
  (`endpoint.rs:153-156`) — so there is nothing to `insert_relay` into.
  A runtime that *started* with relays off still rebinds (same
  `SecretKey`, identity unchanged) when they are switched on; whether the
  bound endpoint has relay transport is recorded at bind time in
  `RuntimeState::endpoint_has_relay_transport`, never inferred.

Net effect of the revised switch: off removes the relays from the map,
closes exactly the connections whose **live** `Connection::paths()` are
relay-only (paths migrate after establishment, so the recorded
establishment path never decides), and leaves every direct-pathed
connection untouched; on re-inserts the relays and wakes only the peers
without a live connection. `DefaultNetwork` (never produced by Koru's
configuration rows) and custom-list changes keep the rebind fallback.
