// SPDX-License-Identifier: MPL-2.0

//! The Cloud card needs a purchase-management decision, not subscription IDs.
use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct Page {
    subscriptions: Vec<Entry>,
    #[serde(deserialize_with = "required_cursor")]
    next_cursor: Option<String>,
}
fn required_cursor<'de, D: serde::Deserializer<'de>>(value: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(value)
}
#[derive(Deserialize)]
struct Entry {
    status: Status,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Incomplete,
    IncompleteExpired,
    Trialing,
    Active,
    PastDue,
    Canceled,
    Unpaid,
    Paused,
}

impl AccountClient {
    /// Existing nonterminal purchases go to management, even when access is
    /// unpaid or pending cancellation. Only a complete traversal proves absence.
    /// Call on the account worker; cancellation is checked between bounded HTTP
    /// requests. Exhaustion, malformed/repeating cursors and outages stay unknown.
    pub fn has_cloud_subscription(
        &self,
        authorization: &AccountAuthorization<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, ClientError> {
        let started = std::time::Instant::now();
        let mut cursor: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..32 {
            if cancelled() || started.elapsed() >= REQUEST_TIMEOUT {
                return Err(ClientError::Transport(TransportError::Timeout));
            }
            let mut path = "/v1/account/subscription?limit=25".to_owned();
            if let Some(cursor) = &cursor {
                path.push_str("&cursor=");
                path.push_str(cursor);
            }
            // This separately composed billing read shares the ordinary account
            // GET's auth and bounded transport policy, including diagnostics.
            let page: Page = self.execute(
                RouteId::Account,
                vec![],
                Some(authorization),
                false,
                Some(path),
            )?;
            if cancelled() || started.elapsed() >= REQUEST_TIMEOUT {
                return Err(ClientError::Transport(TransportError::Timeout));
            }
            if page.subscriptions.len() > 25 {
                return Err(ClientError::InvalidResponse);
            }
            if page
                .subscriptions
                .iter()
                .any(|entry| !matches!(entry.status, Status::Canceled | Status::IncompleteExpired))
            {
                return Ok(true);
            }
            let Some(next) = page.next_cursor else {
                return Ok(false);
            };
            if next.is_empty()
                || next.len() > 2048
                || !next
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || !seen.insert(next.clone())
            {
                return Err(ClientError::InvalidResponse);
            }
            cursor = Some(next);
        }
        Err(ClientError::InvalidResponse)
    }
}

#[cfg(test)]
mod tests;
