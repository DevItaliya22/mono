# Reddit connection and publication

The `koru-socials-reddit` adapter connects an installed application and submits
text/self posts. Live consent and publication still need acceptance with an
approved Reddit application. Implementing the adapter does not grant API access.

## Application setup

1. Obtain Reddit's approval for the intended application and use case. Its
   [Responsible Builder Policy](https://support.reddithelp.com/hc/en-us/articles/42728983564564-Responsible-Builder-Policy)
   requires explicit approval before API access. Follow the access request link
   in the [Data API Wiki](https://support.reddithelp.com/hc/en-us/articles/16160319875092-Reddit-Data-API-Wiki).
   Koru has not verified that approval has been granted.
2. Use an **installed app**, with the callback registered exactly as
   `http://localhost:48180/oauth/reddit/callback`.
3. Enter that application's public client ID in Koru. Installed apps have no
   client secret. Koru uses `identity submit` and permanent authorization to
   obtain a refresh token. Consent opens in the system browser and returns to
   the local callback; no hosted authentication service is needed for this
   installed-app flow. These protocol details follow Reddit's
   [OAuth documentation](https://github.com/reddit-archive/reddit/wiki/OAuth2).

The official Data API Wiki cautions that older API documentation can be out of
date. A successful build or simulated response cannot establish that Reddit
will issue credentials or approve Koru's use case.

## Native contract

Connection verifies `/api/v1/me`; its base36 `id` is the stored actor identity,
while the username is display metadata. A fresh random state and the shared
IPv4/IPv6 localhost listener bind consent to the pending request. The worker
waits for at most five minutes and supports cancellation. HTTP requests have
bounded timeouts, response sizes, and no automatic retries or redirects.

Only the refresh token and public client ID enter the OS vault. The credential
entry includes the selected root, release channel, acknowledged Agent Device
ID, provider origin and stable actor ID. Access tokens stay in memory. Local
disconnect clears this entry under the same lock used by publication; it does
not revoke Reddit's remote app authorization.

Before each submission, the adapter refreshes authorization, persists any
rotated refresh token, and verifies the same actor again. One preparation
permits one submission. Expiry, disconnect, reconnect or another credential
rotation invalidates preparation. The caller owns durable scheduling and
records its Sending attempt before calling the adapter.

## Submission and failure behavior

The adapter sends one `/api/submit` request with `kind=self`, subreddit, title,
body and optional flair template ID. The subreddit is entered without `r/`.
Titles are limited to 300 characters. Flair discovery, community rule discovery,
media, link submissions and moderation operations are separate work. The
supported parameters and submit scope are described in Reddit's
[API reference](https://www.reddit.com/dev/api/#POST_api_submit).

HTTP 200 is insufficient evidence of publication: Reddit's `json.errors` must
be empty and its returned post ID must be valid. Koru constructs the public
link from that ID and never opens an arbitrary provider-supplied URL.

| Outcome | Adapter result |
| --- | --- |
| Confirmed post ID | Published ID and canonical Reddit link |
| HTTP 401 | Needs connection |
| HTTP 429 | Safe retry after the longest provided delay |
| Explicit client refusal or valid nonempty `json.errors` | Rejected; user intervention |
| Lost response, HTTP 408/5xx, malformed success or conflicting evidence | Unconfirmed; no automatic resend |

Reddit's `x-ratelimit-reset` is a relative delay in seconds, unlike X's Unix
timestamp header. Fractional delays round up and large delays saturate rather
than wrap. A `RATELIMIT` JSON error without a machine-readable wait is a
rejection; the adapter does not parse localized messages or guess when to retry.
Raw provider errors and tokens are never included in the public error enum.

## Acceptance still needed

- With the approved installed app, connect and verify the expected username.
- Schedule a text post into a community where that account may post; verify
  title, body, subreddit, optional flair and the returned public link.
- Verify a community refusal leaves a visible failure rather than Published.
- Reopen the app and publish again to exercise the stored refresh credential.
- Disconnect before a due post and verify reconnection is required.

Do not perform these checks against a community that disallows test posts.
The adapter's deterministic tests inject transport, vault and locks; they do
not contact Reddit, open a browser, write files or touch the OS credential vault.
