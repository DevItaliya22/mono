// SPDX-License-Identifier: MPL-2.0

//! Argon2id, and the fixed verifier that makes an unknown address cost the
//! same as a known one.
//!
//! This is the only place in the workspace that hashes a password. It sits
//! beside the other cryptography adapters rather than in `koru-account-core`
//! so `argon2` is linked by the hosted account service (which is not in this
//! repository) and by nothing else — the mobile host, the agent and the
//! account client all depend on core and none of them ever checks a password.

use argon2::password_hash::{PasswordHash, PasswordHasher as _, PasswordVerifier as _, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use koru_account_core::{
    AcceptedPassword, CryptoError, PasswordCheck, PasswordHashing, PasswordVerifier,
};

/// The Argon2id parameters, and the reason for each number.
///
/// Memory is OWASP's baseline; iterations deliberately are not.
///
/// **19 MiB of memory** is the parameter that does the work. Argon2id is
/// memory-*hard*, which is what separates it from bcrypt: an attacker with a
/// GPU farm has thousands of cores and nothing like thousands of independent
/// 19 MiB working sets, so the memory cost is what their hardware advantage
/// runs into.
///
/// **Ten iterations**, not OWASP's companion figure of two. That pairing is a
/// published *floor* for constrained hardware rather than a target, and on this
/// hardware it measured 9.8 ms per verification where the field's usual target
/// is 50-100 ms -- roughly a fivefold gift to anyone attacking a stolen
/// verifier table. Ten measures 43.7 ms here. The maintainers took this decision on
/// 2026-09-16, before any account existed and so before any hash had to be
/// rewritten.
///
/// Iterations rather than memory, deliberately: memory is what a GPU farm runs
/// into, and it is also what bounds how many verifications may run at once, so
/// raising it would move the concurrency ceiling in 5.2 with it. Passes over
/// the same 19 MiB cost an attacker the same multiple and cost the server
/// nothing but time it already has.
///
/// **One lane**, because parallelism inside a single hash buys the defender
/// nothing on a server that is already handling requests concurrently — it
/// would take more cores per sign-in to finish one sign-in sooner.
///
/// **Measured, not assumed**, by `measure_verification_cost` at release
/// codegen. Re-measure there rather than trusting this comment after changing
/// a number; the plan's own prediction of ~50 ms was wrong by 5x at the
/// original settings, which is how this decision came to be taken at all.
///
/// Raising these later needs no migration. The parameters travel inside each
/// stored verifier, so an existing verifier keeps verifying at its own cost
/// and is rewritten at the new one on that person's next successful sign-in --
/// which is what `PasswordCheck::rehash` is for.
const MEMORY_KIB: u32 = 19 * 1024;
const ITERATIONS: u32 = 10;
const LANES: u32 = 1;

/// A password that no person can have, hashed once at startup.
///
/// Used when no account exists at the address, so that path spends the same
/// work as the path that finds one. It is a constant rather than a random
/// value because nothing about it needs to be secret: it never authenticates
/// anything, and its only job is to cost the same.
const ABSENT_PASSWORD: &str = "koru-absent-account-placeholder";

pub struct Argon2PasswordHashing {
    argon2: Argon2<'static>,
    /// Hashed once, at construction, with the parameters above — so it stays
    /// the same cost as a real verifier when those parameters are raised.
    absent: PasswordVerifier,
}

fn parameters() -> Result<Params, CryptoError> {
    Params::new(MEMORY_KIB, ITERATIONS, LANES, None).map_err(|_| CryptoError::InvalidInput)
}

fn salt() -> SaltString {
    SaltString::generate(&mut argon2::password_hash::rand_core::OsRng)
}

impl Argon2PasswordHashing {
    pub fn new() -> Result<Self, CryptoError> {
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, parameters()?);
        let absent = argon2
            .hash_password(ABSENT_PASSWORD.as_bytes(), &salt())
            .map(|hash| PasswordVerifier(hash.to_string()))
            .map_err(|_| CryptoError::KeyUnavailable)?;
        Ok(Self { argon2, absent })
    }
}

impl PasswordHashing for Argon2PasswordHashing {
    fn hash(&self, password: &AcceptedPassword) -> Result<PasswordVerifier, CryptoError> {
        self.argon2
            .hash_password(password.expose().as_bytes(), &salt())
            .map(|hash| PasswordVerifier(hash.to_string()))
            .map_err(|_| CryptoError::KeyUnavailable)
    }

    fn verify(
        &self,
        password: &str,
        stored: Option<&PasswordVerifier>,
    ) -> Result<PasswordCheck, CryptoError> {
        // No early return on `None`. The whole point of the port taking an
        // `Option` is that this function always does the work; returning
        // `Rejected` here without hashing would put back the timing oracle it
        // exists to remove.
        let absent = stored.is_none();
        let verifier = stored.unwrap_or(&self.absent);
        let parsed = PasswordHash::new(&verifier.0).map_err(|_| CryptoError::VerificationFailed)?;

        // The password is normalized on the way in from `check_password` at
        // sign-up; a sign-in normalizes at the call site for the same reason,
        // so both spellings of an accented password reach the same comparison.
        let matched = self
            .argon2
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();

        if absent || !matched {
            return Ok(PasswordCheck::Rejected);
        }
        Ok(PasswordCheck::Accepted {
            rehash: !current_parameters(&parsed),
        })
    }
}

/// Whether a stored hash was made with the parameters above.
///
/// Compared field by field rather than by re-serializing, because a PHC string
/// may legitimately differ in salt length or encoding without differing in
/// cost, and only cost is the question here.
fn current_parameters(parsed: &PasswordHash<'_>) -> bool {
    if parsed.algorithm.as_str() != Algorithm::Argon2id.ident().as_str() {
        return false;
    }
    let Ok(params) = Params::try_from(parsed) else {
        return false;
    };
    params.m_cost() == MEMORY_KIB && params.t_cost() == ITERATIONS && params.p_cost() == LANES
}

#[cfg(test)]
mod tests;
