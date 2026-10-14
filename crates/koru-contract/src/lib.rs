// SPDX-License-Identifier: MPL-2.0

//! Rust boundary for the language-neutral Koru v1 contract.
//!
//! Generated DTOs stay in [`proto`]. [`validation`] checks semantic constraints
//! before adapters map them into long-lived domain state.

pub mod generated {
    pub mod koru {
        pub mod api {
            pub mod v1 {
                include!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../contracts/gen/rust/koru.api.v1.rs"
                ));
            }
        }
        pub mod v1 {
            include!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../contracts/gen/rust/koru.v1.rs"
            ));
        }

        pub mod testing {
            pub mod v1 {
                include!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../contracts/gen/rust/koru.testing.v1.rs"
                ));
            }
        }
    }
}

pub use generated::koru::api::v1 as mobile_proto;
pub use generated::koru::testing::v1 as testing_proto;
pub use generated::koru::v1 as proto;

pub mod mobile_account;
mod validation;

pub use validation::{ValidationError, validate_command, validate_document};
