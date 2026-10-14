# keyring

Koru pins `keyring` 4.1.6 in the workspace manifest. Native OAuth code reaches
it only through `koru-oauth-credentials::OsCredentialStore`; provider owners do
not select backends or use `keyring::v1` directly.

Refresh tokens are normally saved while access tokens remain in provider-worker
memory. LinkedIn native Socials grants instead save their access token and
expiry because renewal requires browser authorization; see
ADR-455. A missing OS vault has no
plaintext fallback. The vault service is
`do.koru.oauth.refresh`; its account is an opaque `v1` name derived from
the HTTPS service origin, release channel, stable selected-root fingerprint,
and Agent-acknowledged Device ID. It contains no path or email.

Tests inject the `CredentialStore` trait or use `SessionCredentialStore`. They
must never read, write, prompt, or delete a real OS credential.

Android does not link `keyring`. `WrappedAccountCredentialVault` uses Android
Keystore AES-GCM wrapping and encrypted values in `noBackupFilesDir`, preserving
the exact Rust entry after `do.koru.oauth.refresh-`. The entry is also
authenticated associated data. The current v1 maximum is 210 ASCII bytes, so
the direct filename plus encrypted/temporary suffixes fits one component. The
account-scoped v2 entry in Plan 1.10 must preserve the logical alias while
revisiting that physical filename bound; Kotlin must not derive account identity.

Missing keys beside ciphertext are errors. Clear does not need a working key;
it durably removes the encrypted value and temporary replacement. A retained
non-exportable wrapping key is not a retained refresh credential. JVM tests
inject ciphertext storage and key lookup while exercising real AES-GCM, so
neither disk nor a real Keystore is opened. Production uses Android's public
`Os.remove` and a known directory opened `O_RDONLY` for directory fsync;
`Os.unlink` and `O_DIRECTORY` are not exposed by the compiled Android SDK.
See [Android's Keystore guide](https://developer.android.com/privacy-and-security/keystore)
for non-exportable keys and off-main-thread use.
