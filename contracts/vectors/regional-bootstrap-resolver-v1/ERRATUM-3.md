# Scope complete worker configuration to isolated roots

Authority: regional-bootstrap.final.md, New response and Portable resolver,
versus its Complete isolated-root override section; TECHNICAL.final.md section 3.
This corrects implementation expectations under the existing signed contract.
It changes no wire version, persisted format, account behavior or deployment.

The original fixture supplied the same complete worker pool for every mode and
treated a changed worker/address as hostile even in production. Consequently it
could not detect that production account entry unnecessarily required worker
metadata, or that authenticated server discovery was being checked against a
client worker registry the contract only requires for isolated roots.

Apply erratum-3 after errata 1 and 2; preserve every earlier byte. The two named
production substitutions now return available after normal strict validation.
Historical production configurations are projected by the test adapter to the
account-only region/API/relay inputs; sync_pool is not passed to production.
Isolated fixtures still use the full two-worker RegionalEndpoints contract.

New cases prove preparation without a pool before any response, authenticated
candidate discovery with valid new keys/addresses, unavailable without a pool,
both compiled relay channels, and unchanged API/relay/key/address/account/owner
refusals. Paired isolated substitutions still refuse exact membership mismatch.
Local-only preparation remains effect-free and accepts no account placement.

Production candidates must name a non-null member of the trusted channel HTTPS
relay list. Production discovery remains routing metadata, never readiness proof,
permission to dial, a signed assignment or dataset admission. No HTTP handler,
transport call, host startup, prewarm or Cloud ALPN is enabled by this correction.
