// SPDX-License-Identifier: MPL-2.0

//! iOS's canonical Protobuf C ABI over `koru-mobile-host`.
//!
//! Each catalog operation accepts one generated request and returns one
//! generated response. The shim owns no product behavior and no JSON shape.

use std::panic::{AssertUnwindSafe, catch_unwind};

use koru_contract::mobile_proto as pb;
use prost::Message;

#[repr(C)]
pub struct KoruBuffer {
    pub ptr: *mut u8,
    pub len: usize,
}

fn empty_buffer() -> KoruBuffer {
    KoruBuffer {
        ptr: std::ptr::null_mut(),
        len: 0,
    }
}

fn into_buffer(bytes: Vec<u8>) -> KoruBuffer {
    let mut bytes = bytes.into_boxed_slice();
    let buffer = KoruBuffer {
        ptr: bytes.as_mut_ptr(),
        len: bytes.len(),
    };
    std::mem::forget(bytes);
    buffer
}

unsafe fn protobuf_call<Request, Response>(
    ptr: *const u8,
    len: usize,
    invoke: fn(Request) -> Response,
) -> KoruBuffer
where
    Request: Message + Default,
    Response: Message,
{
    let result = catch_unwind(AssertUnwindSafe(|| {
        let bytes = if ptr.is_null() {
            &[]
        } else {
            // SAFETY: the C ABI requires a readable pointer for `len` bytes.
            unsafe { std::slice::from_raw_parts(ptr, len) }
        };
        Request::decode(bytes)
            .map(|request| into_buffer(invoke(request).encode_to_vec()))
            .unwrap_or_else(|_| empty_buffer())
    }));
    result.unwrap_or_else(|_| empty_buffer())
}

macro_rules! protobuf_export {
    ($symbol:ident, $request:ty, $response:ty, $invoke:path) => {
        /// One catalog operation over the C ABI: a serialized request in, a
        /// serialized response out.
        ///
        /// # Safety
        ///
        /// `ptr` is null or readable for `len` bytes for the duration of the
        /// call; the returned buffer is freed exactly once with
        /// `koru_buffer_free`.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $symbol(ptr: *const u8, len: usize) -> KoruBuffer {
            if stringify!($symbol).starts_with("koru_api_account_") && len > 96 * 1024 {
                return empty_buffer();
            }
            // SAFETY: forwarded under this exported function's pointer contract.
            unsafe { protobuf_call::<$request, $response>(ptr, len, $invoke) }
        }
    };
}

protobuf_export!(
    koru_api_status,
    pb::HostStatusRequest,
    pb::HostStatus,
    koru_mobile_host::protobuf::status
);
protobuf_export!(
    koru_api_link_device,
    pb::LinkDeviceRequest,
    pb::LinkDeviceResult,
    koru_mobile_host::protobuf::link_device
);
protobuf_export!(
    koru_api_advertise_build,
    pb::BuildPayload,
    pb::MutationResult,
    koru_mobile_host::protobuf::advertise_build
);
protobuf_export!(
    koru_api_store_inventory,
    pb::InventoryRequest,
    pb::Inventory,
    koru_mobile_host::protobuf::inventory
);
protobuf_export!(
    koru_api_store_document,
    pb::DocumentRequest,
    pb::DocumentProjection,
    koru_mobile_host::protobuf::document
);
protobuf_export!(
    koru_api_store_blob,
    pb::BlobRequest,
    pb::BlobBytes,
    koru_mobile_host::protobuf::blob
);
// `cloud-blob-object-v1` §10 at the C edge. Marshalling only, exactly as
// every export above: the engine is composed by the Store worker this host
// embeds, and the iOS **surfaces** that would read these are the mac branch's
// catch-up (docs/features/macos/CATCHUP.md).
protobuf_export!(
    koru_api_store_blob_status,
    pb::BlobStatusRequest,
    pb::BlobTransferStatus,
    koru_mobile_host::protobuf::blob_status
);
// The Cloud sync progress screen's two reads. Marshalling only; the iOS
// surface that would draw them is the mac branch's catch-up.
protobuf_export!(
    koru_api_store_cloud_sync_status,
    pb::CloudSyncStatusRequest,
    pb::CloudSyncStatus,
    koru_mobile_host::protobuf::cloud_sync_status
);
protobuf_export!(
    koru_api_store_cloud_waiting,
    pb::CloudWaitingRequest,
    pb::CloudWaitingDocuments,
    koru_mobile_host::protobuf::cloud_waiting
);
protobuf_export!(
    koru_api_store_set_blob_policy,
    pb::SetBlobPolicyRequest,
    pb::MutationResult,
    koru_mobile_host::protobuf::set_blob_policy
);
protobuf_export!(
    koru_api_store_request_blob,
    pb::RequestBlobRequest,
    pb::MutationResult,
    koru_mobile_host::protobuf::request_blob
);
protobuf_export!(
    koru_api_store_file_blob,
    pb::FileBlobRequest,
    pb::FileBlob,
    koru_mobile_host::protobuf::file_blob
);
protobuf_export!(
    koru_api_store_export_blob,
    pb::ExportBlobRequest,
    pb::ExportedBlob,
    koru_mobile_host::protobuf::export_blob
);
protobuf_export!(
    koru_api_store_release_blobs,
    pb::ReleaseBlobsRequest,
    pb::ReleasedBlobs,
    koru_mobile_host::protobuf::release_blobs
);
protobuf_export!(
    koru_api_store_recover_blobs,
    pb::BlobRecoverRequest,
    pb::MutationResult,
    koru_mobile_host::protobuf::recover_blobs
);
protobuf_export!(
    koru_api_store_blob_arrivals,
    pb::BlobArrivalsRequest,
    pb::BlobArrivals,
    koru_mobile_host::protobuf::blob_arrivals
);
protobuf_export!(
    koru_api_store_run,
    pb::RunRequest,
    pb::RunView,
    koru_mobile_host::protobuf::run
);
protobuf_export!(
    koru_api_store_run_timeline,
    pb::RunTimelineRequest,
    pb::RunTimelineWindow,
    koru_mobile_host::protobuf::run_timeline
);
protobuf_export!(
    koru_api_store_mutate_workspace,
    pb::WorkspaceMutation,
    pb::MutationResult,
    koru_mobile_host::protobuf::mutate_workspace
);
protobuf_export!(
    koru_api_store_mutate_profile,
    pb::ProfileMutation,
    pb::MutationResult,
    koru_mobile_host::protobuf::mutate_profile
);
protobuf_export!(
    koru_api_store_mutate_document,
    pb::DocumentMutation,
    pb::MutationResult,
    koru_mobile_host::protobuf::mutate_document
);
protobuf_export!(
    koru_api_store_import_file,
    pb::ImportFileRequest,
    pb::DocumentIdResult,
    koru_mobile_host::protobuf::import_file
);
protobuf_export!(
    koru_api_store_ensure_file_note,
    pb::FileNoteRequest,
    pb::DocumentIdResult,
    koru_mobile_host::protobuf::file_note
);
protobuf_export!(
    koru_api_start_pairing,
    pb::StartPairingRequest,
    pb::PairingOffer,
    koru_mobile_host::protobuf::start_pairing
);
protobuf_export!(
    koru_api_cancel_pairing,
    pb::CancelPairingRequest,
    pb::MutationResult,
    koru_mobile_host::protobuf::cancel_pairing
);
protobuf_export!(
    koru_api_devices,
    pb::DevicesRequest,
    pb::DeviceList,
    koru_mobile_host::protobuf::devices
);
protobuf_export!(
    koru_api_device_history,
    pb::DeviceHistoryRequest,
    pb::DeviceHistory,
    koru_mobile_host::protobuf::device_history
);
protobuf_export!(
    koru_api_unlink_device,
    pb::UnlinkDeviceRequest,
    pb::MutationResult,
    koru_mobile_host::protobuf::unlink_device
);
protobuf_export!(
    koru_api_rename_device,
    pb::RenameDeviceRequest,
    pb::MutationResult,
    koru_mobile_host::protobuf::rename_device
);
protobuf_export!(
    koru_api_reset_identity,
    pb::ResetIdentityRequest,
    pb::MutationResult,
    koru_mobile_host::protobuf::reset_identity
);
protobuf_export!(
    koru_api_set_relay,
    pb::SetRelayRequest,
    pb::MutationResult,
    koru_mobile_host::protobuf::set_relay
);

protobuf_export!(
    koru_api_account_command,
    pb::AccountCommandRequest,
    pb::AccountCommandReply,
    koru_mobile_host::protobuf::account_command
);
#[unsafe(no_mangle)]
pub unsafe extern "C" fn koru_api_account_poll(ptr: *const u8, len: usize) -> KoruBuffer {
    if len > 96 * 1024 {
        return empty_buffer();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let bytes = if ptr.is_null() {
            &[]
        } else {
            // SAFETY: caller supplies readable request bytes for len.
            unsafe { std::slice::from_raw_parts(ptr, len) }
        };
        let Ok(request) = pb::AccountPollRequest::decode(bytes) else {
            return empty_buffer();
        };
        let mut response = koru_mobile_host::protobuf::account_poll(request);
        let encoded = response.encode_to_vec();
        koru_mobile_host::protobuf::zeroize_account_poll(&mut response);
        into_buffer(encoded)
    }))
    .unwrap_or_else(|_| empty_buffer())
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn koru_api_account_platform(ptr: *const u8, len: usize) -> KoruBuffer {
    if len > 96 * 1024 {
        return empty_buffer();
    }
    catch_unwind(AssertUnwindSafe(|| {
        let bytes = if ptr.is_null() {
            &[]
        } else {
            // SAFETY: caller supplies readable request bytes for len.
            unsafe { std::slice::from_raw_parts(ptr, len) }
        };
        let Ok(request) = koru_mobile_host::protobuf::decode_account_platform(bytes) else {
            return empty_buffer();
        };
        into_buffer(koru_mobile_host::protobuf::account_platform(request).encode_to_vec())
    }))
    .unwrap_or_else(|_| empty_buffer())
}

/// Frees a response returned by any `koru_api_*` operation.
///
/// # Safety
/// `buffer` must be an untouched value returned by this library and must be
/// freed at most once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn koru_buffer_free(buffer: KoruBuffer) {
    if buffer.ptr.is_null() {
        return;
    }
    // SAFETY: `into_buffer` allocated exactly this pointer and length.
    let mut bytes =
        unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(buffer.ptr, buffer.len)) };
    zeroize::Zeroize::zeroize(bytes.as_mut());
    drop(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str, pattern: &str) -> std::collections::BTreeSet<String> {
        text.match_indices(pattern)
            .map(|(at, _)| {
                let rest = &text[at + pattern.len()..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                format!("koru_{name}")
            })
            .collect()
    }

    #[test]
    fn header_names_every_export() {
        let header = include_str!("../include/koru_mobile.h");
        let source = include_str!("lib.rs");
        let declared = names(header, "KoruBuffer koru_")
            .into_iter()
            .chain(names(header, "void koru_"))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(declared.len(), 35, "34 catalog calls plus buffer_free");
        for symbol in declared {
            assert!(source.contains(&symbol), "{symbol} has no Rust export");
        }
    }

    #[test]
    fn malformed_protobuf_returns_no_bytes() {
        let buffer = unsafe { koru_api_status([0xff].as_ptr(), 1) };
        assert!(buffer.ptr.is_null());
        assert_eq!(buffer.len, 0);
    }

    #[test]
    fn free_tolerates_nothing() {
        unsafe { koru_buffer_free(empty_buffer()) };
    }

    #[test]
    fn an_empty_success_is_not_the_failure_sentinel() {
        let buffer = into_buffer(Vec::new());
        assert!(!buffer.ptr.is_null());
        assert_eq!(buffer.len, 0);
        unsafe { koru_buffer_free(buffer) };
    }
}
