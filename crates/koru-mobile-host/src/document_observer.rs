// SPDX-License-Identifier: MPL-2.0

//! Lifetime-owned, content-free document invalidation for native viewers.
//! Reuses the commit hub that wakes Cloud; never polls or drains projection.
use koru_store_api::cloud_changes::Subscription;

pub struct DocumentObserver(tokio::task::JoinHandle<()>);

impl Drop for DocumentObserver {
    fn drop(&mut self) {
        // Only notification interest is canceled. No Store write is owned by
        // this task, and dropping it cannot cancel an accepted mutation.
        self.0.abort();
    }
}

impl DocumentObserver {
    pub(crate) fn start(
        runtime: &tokio::runtime::Handle,
        mut watch: Subscription,
        mut manifest: Option<Subscription>,
        notify: impl Fn() -> bool + Send + 'static,
    ) -> Self {
        Self(runtime.spawn(async move {
            loop {
                tokio::select! {
                    _ = watch.changed() => {},
                    _ = async {
                        match &mut manifest {
                            Some(watch) => watch.changed().await,
                            None => std::future::pending().await,
                        }
                    } => {},
                }
                // Clear before delivery, so a commit racing the caller's
                // subsequent read remains pending and produces another wake.
                watch.begin_pass();
                if let Some(manifest) = &mut manifest {
                    manifest.begin_pass();
                }
                if !notify() {
                    tracing::warn!("document observer: notification delivery failed");
                    break;
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests;
