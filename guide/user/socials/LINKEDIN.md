# LinkedIn connection and publication

## Application setup

Native authorization is gated: ask your LinkedIn contact to enable PKCE for the
application before testing. Koru uses the public Client ID, system browser,
S256 challenge and `http://127.0.0.1:48181/oauth/linkedin/callback` loopback.
Do not paste a Client Secret into Koru. Renew expired access through Connect;
native renewal repeats browser authorization. See LinkedIn's
[native authorization guide](https://learn.microsoft.com/en-us/linkedin/shared/authentication/authorization-code-flow-native).

Enable Sign In with LinkedIn using OpenID Connect and Share on LinkedIn for a
personal connection (`openid profile w_member_social`). A Page connection uses
`openid profile w_organization_social r_organization_admin`; Community Management
access must make those permissions and the organization APIs available to your
app. The numeric Page ID alone does not authorize publication. Koru verifies the
signed-in member, an approved administrator/content administrator ACL for that
exact Page. Other roles and pending requests fail closed. The channel displays
the verified numeric Page ID: organization-name lookup would add a broader
write-admin permission just for a label.
Role checks stop after five result pages or a 60-second deadline checked between
bounded requests. A Page outside that result window cannot connect; this limit
prevents a large account catalog from occupying the publishing worker indefinitely.
See [organization access control](https://learn.microsoft.com/en-us/linkedin/marketing/community-management/organizations/organization-access-control-by-role?view=li-lms-2026-09).

Profile identity comes from the authenticated
[UserInfo endpoint](https://learn.microsoft.com/en-us/linkedin/consumer/integrations/self-serve/sign-in-with-linkedin-v2).

## Runtime behavior

`koru-socials-linkedin` implements connection, local credential availability,
disconnect and one prepared text publication. Member URNs and Page URNs are
different actors. A Page credential retains its authorizing member separately.
The OS vault holds the access token and absolute expiration under the selected
root/channel/device/actor namespace. There is no token in the Socials document.

Before each send, the adapter rechecks identity and Page authorization, then arms
one request for at most 60 seconds. It requires at least 60 seconds remaining on
the token at preparation. Disconnect or reconnect invalidates the armed grant;
the shared credential lock covers the final check and request. An expired token
or 401 asks for reconnection.

Publication uses `/rest/posts`, `LinkedIn-Version: 202609` and Rest.li 2.0.0,
with public main-feed visibility. Plain text is escaped for LinkedIn's `little`
grammar. HTTP 201 with a valid `x-restli-id` share/ugcPost URN is the receipt;
the resulting link is built under `https://www.linkedin.com/feed/update/`.
Transport failures, 5xx and a missing/malformed receipt remain Unconfirmed;
they never trigger an automatic second POST. A 429 preserves the provider's
Retry-After delay. The caller owns the durable attempt journal and schedule.
See [Posts API](https://learn.microsoft.com/en-us/linkedin/marketing/community-management/shares/posts-api?view=li-lms-2026-09),
[text grammar](https://learn.microsoft.com/en-us/linkedin/marketing/community-management/shares/little-text-format?view=li-lms-2026-09),
and [supported versions](https://learn.microsoft.com/en-us/linkedin/marketing/integrations/recent-changes?view=li-lms-2026-09).

## Verification

Adapter tests inject transport, credentials and credential locks; they do not
open a browser, vault, Store or network connection. Production entry points are
disabled by the `no-network` feature. Live native consent and real personal/Page
publication still need acceptance with an approved LinkedIn application.

For live acceptance: connect the intended member, verify the shown identity,
schedule a short test and confirm its public link. Repeat for a Page using an
approved administrator. Then disconnect before a due send and verify that Koru
asks for connection. Reconnect the same actor and explicitly retry. An account
without the Page role must not connect that Page or publish as it.
