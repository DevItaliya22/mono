# Frozen regional route and resolver vectors

Authority: regional-bootstrap.final.md and TECHNICAL.final.md section 3,
approved under D201/D203. This additive checkpoint precedes source. Historical
regional-bootstrap-v1 foundation vectors and SQL remain byte-identical.

cases.json contains complete raw JSON decoder cases (including escaped duplicate
keys) and complete trusted configuration/response/session-owner resolver cases.
The harness must pass raw JSON directly to production serde decoders; decoding
through a JSON Value first would erase duplicate keys. Null route relay is
structurally valid; only complete isolated empty-relay configuration admits it.

Resolver configurations are test injection values, never a new configuration
carrier. Missing fields model a host that cannot assemble the complete bundle.
Generation/account pairs model the existing owner at dispatch and completion;
the resolver creates no generation registry. Local-only preparation returns no
placement and has no effects. Available routes are candidates, not readiness,
authority, permission to dial, or Cloud intent. No public endpoint is enabled.

The production relay fixture uses the existing published-channel provider.
API HTTP uses the existing loopback-only account transport exception; local
relay HTTP uses the existing Iroh transport mode. Physical Android connectivity,
real bootstrap, restore, prewarm, toggle and dial isolation remain downstream.
