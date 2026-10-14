// SPDX-License-Identifier: MPL-2.0

//! Causal comparison shared by scoped peer and Cloud reconciliation.
//! Callers establish authority and decode their own protocol's vectors.
use loro::VersionVector;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Missing {
    pub download: bool,
    pub upload: bool,
}

pub fn missing(local: &VersionVector, remote: &VersionVector) -> Missing {
    Missing {
        download: !local.includes_vv(remote),
        upload: !remote.includes_vv(local),
    }
}

#[cfg(test)]
mod tests;
