# Android Credential Manager

The Android adapter pins `credentials` and `credentials-play-services-auth`
1.6.0, the stable release; the documentation's dependency example currently
uses a 1.7 alpha. Google ID support is `googleid` 1.2.0. Versions were checked
against the [AndroidX release notes](https://developer.android.com/jetpack/androidx/releases/credentials)
and [Google release notes](https://developers.google.com/identity/android-credential-manager/releases).

`GetSignInWithGoogleOption` is the explicit button flow. Koru supplies the hosted
nonce and web client ID, including for collision reauthentication; it never
uses automatic selection as a substitute for that second chooser. The SDK's
outer credential type remains `TYPE_GOOGLE_ID_TOKEN_CREDENTIAL`; the explicit
flow has its subtype inside the bundle. Only `idToken` leaves this adapter.
See the [implementation guide](https://developer.android.com/identity/sign-in/credential-manager-siwg-implementation)
and [option reference](https://developers.google.com/identity/android-credential-manager/android/reference/com/google/android/libraries/identity/googleid/GetSignInWithGoogleOption).

Koru updates its Activity-based `MutableContextWrapper` explicitly on main-thread
lifecycle callbacks and retains the operation in application scope. Do not
assume stable 1.6.0 includes the automatic context-swapping fix listed under
1.7.0-alpha03. Actual rotation through Google's UI still needs device evidence.
Cancellation can race delivery across `withContext`; the adapter therefore
keeps a wiping owner for created byte arrays even if the receiving coroutine
never sees the result. Immutable SDK/JVM strings cannot offer deterministic
zeroization. Exceptions and credential bundles must not be logged.

`clearCredentialState` clears provider session preference independently of
erasing Koru's refresh credential. Koru bounds this call to thirty seconds.
A failed clear remains visible, cannot recreate the erased credential, and
does not prevent the explicit chooser after that bound. A new explicit sign-in
retries a failed clear once before launching its sheet; no retry timer is added.

The pure Android effect adapter tests non-consuming polls, lost acknowledgements,
old correlations lingering after acknowledgement, expiry and changed prompts,
late token disposal, background/rotation, vault failures, and provider clearing.
Shared Rust fixtures retain responsibility for exact hosted exchange replay and
the remote-commit/vault-acknowledgement boundary.
