// SPDX-License-Identifier: MPL-2.0

//! Sign-in result delivery; socket listeners and native SDKs belong to hosts.
use crate::account::{Cancel, ServiceError};
use zeroize::Zeroize;

/// Decoded delivery fields preserve duplicates for strict protocol validation.
pub struct Callback(Vec<(String, String)>);
impl Callback {
    pub fn new(fields: Vec<(String, String)>) -> Self {
        Self(fields)
    }
    pub fn field_names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(name, _)| name.as_str())
    }
    pub fn values<'a>(&'a self, field: &'a str) -> impl Iterator<Item = &'a str> {
        self.0
            .iter()
            .filter(move |(name, _)| name == field)
            .map(|(_, value)| value.as_str())
    }
}

impl Drop for Callback {
    fn drop(&mut self) {
        zeroize_fields(&mut self.0);
    }
}

fn zeroize_fields(fields: &mut [(String, String)]) {
    for (name, value) in fields {
        name.zeroize();
        value.zeroize();
    }
}

pub trait SignInDelivery: Send + Sync {
    fn bind(&self) -> Result<Box<dyn SignInSession>, ServiceError>;

    fn supports_native_token(&self) -> bool {
        false
    }

    fn bind_native(&self) -> Result<Box<dyn NativeSignInSession>, ServiceError> {
        Err(ServiceError::terminal("native_sign_in_unavailable"))
    }
}

/// Private host delivery request. The host supplies its trusted web client ID;
/// neither the token nor a provider-selected client ID configures that value.
#[derive(Clone)]
pub struct NativeSignInRequest {
    pub generation: u64,
    pub request_id: u64,
    pub nonce: String,
    pub expires_at_millis: i64,
    pub reauthentication: bool,
}

impl std::fmt::Debug for NativeSignInRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NativeSignInRequest(<redacted>)")
    }
}

impl Drop for NativeSignInRequest {
    fn drop(&mut self) {
        self.nonce.zeroize();
    }
}

pub trait NativeSignInSession: Send {
    /// Publish one correlated prompt, then wait until its deadline or cancel.
    /// Hosts reject stale/duplicate results before returning the raw token.
    fn wait(
        &mut self,
        request: &NativeSignInRequest,
        cancel: &Cancel,
    ) -> Result<koru_account_wire::GoogleIdToken, ServiceError>;
}

pub trait SignInSession: Send {
    fn callback_port(&self) -> u16;
    /// A phone returns through the website's App Link instead of a loopback
    /// port; its `callback_port` is then zero and ignored.
    fn app_link_return(&self) -> bool {
        false
    }
    fn wait(&mut self, state: &str, cancel: &Cancel) -> Result<Callback, ServiceError>;
}

#[cfg(test)]
mod tests;
