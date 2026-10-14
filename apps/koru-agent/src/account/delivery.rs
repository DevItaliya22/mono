// SPDX-License-Identifier: MPL-2.0

//! Desktop loopback delivery. Only the bounded account worker waits here.
use koru_account_runtime::account::{Cancel, ServiceError};
use koru_account_runtime::delivery::{Callback, SignInDelivery, SignInSession};
use koru_oauth_loopback::{CallbackListener, TwoStageCallbackListener, WaitOptions};
use std::time::Duration;
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
pub struct LoopbackDelivery;
struct Session {
    listener: TwoStageCallbackListener,
    port: u16,
}
impl SignInDelivery for LoopbackDelivery {
    fn bind(&self) -> Result<Box<dyn SignInSession>, ServiceError> {
        let listener = CallbackListener::bind(koru_account_wire::NATIVE_LOOPBACK_CALLBACK_PATH)
            .map_err(|_| ServiceError::terminal("callback_unavailable"))?;
        let port = listener.local_addr().port();
        Ok(Box::new(Session {
            listener: listener.into_two_stage(),
            port,
        }))
    }
}
impl SignInSession for Session {
    fn callback_port(&self) -> u16 {
        self.port
    }
    fn wait(&mut self, state: &str, cancel: &Cancel) -> Result<Callback, ServiceError> {
        self.wait_with(state, &|| cancel.cancelled())
    }
}
impl Session {
    fn wait_with(
        &mut self,
        state: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Callback, ServiceError> {
        let callback = self
            .listener
            .wait_stage(&wait_options(state), cancelled)
            .map_err(map_callback_error)?;
        Ok(Callback::new(
            callback
                .fields()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
        ))
    }
}
const CALLBACK_SUCCESS_HTML: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Return to Koru</title></head><body><p>Sign-in was returned to Koru. You can close this tab.</p><script>window.close();</script></body></html>"#;
const CALLBACK_ERROR_HTML: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Return to Koru</title></head><body><p>Koru could not accept this sign-in. Return to Koru and try again.</p></body></html>"#;
fn wait_options(state: &str) -> WaitOptions<'_> {
    WaitOptions {
        expected_state: state,
        timeout: CALLBACK_TIMEOUT,
        poll: koru_oauth_loopback::DEFAULT_POLL,
        max_request_bytes: koru_oauth_loopback::DEFAULT_MAX_REQUEST_BYTES,
        success_html: CALLBACK_SUCCESS_HTML,
        error_html: CALLBACK_ERROR_HTML,
    }
}
fn map_callback_error(error: koru_oauth_loopback::Error) -> ServiceError {
    match error.kind() {
        koru_oauth_loopback::ErrorKind::Cancelled => ServiceError::terminal("cancelled"),
        koru_oauth_loopback::ErrorKind::TimedOut => ServiceError::terminal("callback_timeout"),
        _ => ServiceError::terminal("callback_invalid"),
    }
}

#[cfg(test)]
mod tests;
