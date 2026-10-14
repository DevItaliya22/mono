# Regional bootstrap v1: frozen foundation vectors

Authority: regional-bootstrap.final.md,
signed under D201. This first immutable increment freezes only the independent
US-deployment, active-account lookup and exact migration-100011 expectations.
The adjacent SQL is byte-for-byte the final contract's complete SQL, with LF
line endings. The production migration belongs to the hosted Koru Cloud
service, which is not in this repository; its tests hold that migration to
this copy, so the file must stay byte-identical.

`foundation.json` is a test fixture, not a new application or persisted format.
Tests consume the deployment/read matrix directly. PostgreSQL verification must
exercise both fresh creation and the real 100010-to-100011 upgrade, with rows
seeded before the upgrade. It must preserve the existing objects and historical
checksums; an earlier binary must refuse the additional ledger entry without
changing it. Merely compiling the guarded PostgreSQL feature proves none of this.

No bootstrap route/worker vectors are claimed here. The final requires both a
mandatory route relay URL drawn from `relay_urls` and a complete two-worker
isolated override with an explicitly empty relay list. That contradiction needs
an approved representation before those vectors can freeze. No nullable field,
sentinel URL or implicit production relay is authorized by this fixture.

Production worker binding additionally remains gated on Plan 19's real registry
and ready pool. Configured addresses are not evidence that a worker is ready.
There is no transport prewarm, admission, local schema or credential change in
this foundation increment.
