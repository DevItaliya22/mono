# Connect an X account

The desktop Socials connection uses OAuth 2.0 authorization code with PKCE
(`S256`). It asks X to identify the signed-in person and saves the refresh
credential in this installation's OS credential vault. It does not publish a
post. Scheduled publication and refresh rotation now run separately in the
assigned device's Agent; see [Socials](../USERGUIDE.md#socials-experimental-desktop-mvp). Live acceptance
is still required before enabling the next providers.

## Configure the X application

In the [X Developer Console](https://developer.x.com/), select the application
and enable OAuth 2.0 in User authentication settings:

If Keys & Tokens shows **OAuth 2.0 Keys → User authentication settings → Set up**,
choose **Set up** first. The Client ID appears after OAuth 2.0 is enabled. The
app-only Bearer Token and OAuth 1.0 Consumer Key are not the values this form
uses. The desktop opens a temporary local callback listener during connection;
a hosted OAuth service is not required for this native X flow.

1. Select **Native App**, the public client type.
2. Permit read and write access.
3. Register this exact callback, including the port and path:

   ```text
   http://localhost:48179/oauth/x/callback
   ```

4. Complete the website and other application details required by X.
5. Copy the OAuth 2.0 **Client ID** from Keys and Tokens.
6. In Koru, choose the Socials megaphone → Connect account, enter the Client ID,
   and choose Connect X account. Approve the account in the browser, then return
   to Koru.

The scopes are `tweet.read users.read tweet.write offline.access`. X issues a
refresh token when `offline.access` is granted. A Native App is a public client;
this flow requires no Client Secret. These settings and the exact callback
requirement follow [X's authorization-code documentation](https://docs.x.com/fundamentals/authentication/oauth-2-0/authorization-code).

Use the hostname `localhost` exactly. During acceptance on 2026-09-26, the
developer console rejected the earlier numeric `127.0.0.1` callback as an
invalid URL format. X's [official xurl CLI](https://github.com/xdevplatform/xurl)
documents localhost callbacks and listening on both loopback address families.
Koru follows that form; the portal's acceptance of the replacement and actual
browser consent still need to be confirmed.

The Client ID is public application configuration. Keep API secrets, access
tokens and refresh tokens out of chat, source files, documents and screenshots.
The UI accepts the public Client ID; the browser exchange obtains the tokens.

## What happens on Connect

`koru-socials-x` runs on the caller's background worker. It obtains the selected
root's acknowledged Device ID through the existing Agent greeting. It binds
IPv4 and IPv6 loopback at the registered port, then hands a generated X URL to the
host's existing browser opener. A random state and PKCE verifier are unique to
the attempt. The shared callback listener checks the exact callback path and
state; duplicate authorization codes are refused.

The browser wait has a five-minute deadline and polls cancellation every
100 ms. HTTP calls disable redirects, use a 10-second connect timeout and a
20-second total timeout, and limit successful response bodies to 64 KiB.
Cancellation is checked between these bounded requests. A browser page says
only that the callback arrived; it cannot assert that connection is complete.

The code is exchanged at `https://api.x.com/2/oauth2/token`; then
`https://api.x.com/2/users/me` establishes the actor ID, username and display
name. A channel is returned as connected only after identity validation and a
successful vault write. Provider errors become fixed typed error codes. Remote
response bodies and credentials never become user-facing error strings.

## Credential ownership

The vault key derives from release channel, selected root, Agent-acknowledged
Device ID, X service origin, and the authenticated X actor ID. The shared
`CredentialEntry::for_provider_actor` makes an opaque, bounded account name
separate from Koru's account UUID scope. It never uses a typed username as
identity and never falls back to a less-specific vault slot or a plaintext file.

The vault value is versioned JSON containing the public Client ID and refresh
token, stored together for a future refresh exchange. Access tokens live only
in worker memory for the identity request and are discarded afterward. Channel
documents contain the actor metadata, with no token fields. A successful local
credential-presence check does not establish that X still accepts the grant.

`has_credentials` reads only the local vault. `disconnect` clears only that
installation's credential; it does not remove a channel document or revoke the
remote application grant. Remote revocation is available in X's connected-app
settings. The caller owns any user-facing disconnect control.

Only one connection flow can bind this port at a time, including across Koru
roots. If another flow owns it, finish or cancel that flow and retry. There is
no fallback to a different callback port because X matches the registered URI.

## Verification and remaining acceptance

Unit tests use pure URL/form parsing and an injected in-memory credential
store. They cover requested scopes, missing/duplicate callback fields, malformed
or ambiguous provider identity/token responses, identity-before-vault ordering,
access-token exclusion, vault refusal, cancellation and disabled side effects
under tests. `no-network` also disables production entry points when the crate
is linked by another crate's tests. The shared loopback suite covers real local
socket callback/state handling; it never opens a browser or contacts X.

The user subsequently connected X and confirmed live single-post publication,
threads and cancellation on September 26, 2026. No live credential was read by
the implementation tools. Refresh/expiry and the failure cases below still need
native acceptance with the registered application.
Repeat with a wrong Client ID, denied consent, occupied callback port, closed
browser, cancellation and app restart. A synced channel on a second device
must require its own connection because credentials remain device-local.

Desktop connection is the present scope. Android needs its own native return
URI and credential-vault host integration. Apple desktop uses the shared
mechanisms but still requires platform acceptance. No result on Windows is
evidence for those platforms.
