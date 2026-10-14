// SPDX-License-Identifier: MPL-2.0

import Foundation
import KoruMobileNative
import SwiftProtobuf

/// Canonical generated-message binding. The C ABI has one symbol per public
/// catalog operation and moves only serialized Protobuf bytes.
enum KoruApiNative {
    enum Failure: Error { case invalidResponse }

    private static func call<Request: SwiftProtobuf.Message, Response: SwiftProtobuf.Message>(
        _ request: Request,
        _ invoke: (UnsafePointer<UInt8>?, Int) -> KoruBuffer,
        as: Response.Type
    ) throws -> Response {
        let request = try request.serializedData()
        let buffer = request.withUnsafeBytes { raw in
            invoke(raw.bindMemory(to: UInt8.self).baseAddress, raw.count)
        }
        defer { koru_buffer_free(buffer) }
        guard let ptr = buffer.ptr else { throw Failure.invalidResponse }
        if buffer.len == 0 { return try Response(serializedBytes: Data()) }
        return try Response(serializedBytes: Data(bytes: ptr, count: buffer.len))
    }

    static func status(_ r: KORAPIHostStatusRequest) throws -> KORAPIHostStatus { try call(r, koru_api_status, as: KORAPIHostStatus.self) }
    static func linkDevice(_ r: KORAPILinkDeviceRequest) throws -> KORAPILinkDeviceResult { try call(r, koru_api_link_device, as: KORAPILinkDeviceResult.self) }
    static func advertiseBuild(_ r: KORAPIBuildPayload) throws -> KORAPIMutationResult { try call(r, koru_api_advertise_build, as: KORAPIMutationResult.self) }
    static func inventory(_ r: KORAPIInventoryRequest) throws -> KORAPIInventory { try call(r, koru_api_store_inventory, as: KORAPIInventory.self) }
    static func document(_ r: KORAPIDocumentRequest) throws -> KORAPIDocumentProjection { try call(r, koru_api_store_document, as: KORAPIDocumentProjection.self) }
    static func blob(_ r: KORAPIBlobRequest) throws -> KORAPIBlobBytes { try call(r, koru_api_store_blob, as: KORAPIBlobBytes.self) }
    static func blobStatus(_ r: KORAPIBlobStatusRequest) throws -> KORAPIBlobTransferStatus { try call(r, koru_api_store_blob_status, as: KORAPIBlobTransferStatus.self) }
    static func cloudSyncStatus(_ r: KORAPICloudSyncStatusRequest) throws -> KORAPICloudSyncStatus { try call(r, koru_api_store_cloud_sync_status, as: KORAPICloudSyncStatus.self) }
    static func cloudWaiting(_ r: KORAPICloudWaitingRequest) throws -> KORAPICloudWaitingDocuments { try call(r, koru_api_store_cloud_waiting, as: KORAPICloudWaitingDocuments.self) }
    static func setBlobPolicy(_ r: KORAPISetBlobPolicyRequest) throws -> KORAPIMutationResult { try call(r, koru_api_store_set_blob_policy, as: KORAPIMutationResult.self) }
    static func requestBlob(_ r: KORAPIRequestBlobRequest) throws -> KORAPIMutationResult { try call(r, koru_api_store_request_blob, as: KORAPIMutationResult.self) }
    static func fileBlob(_ r: KORAPIFileBlobRequest) throws -> KORAPIFileBlob { try call(r, koru_api_store_file_blob, as: KORAPIFileBlob.self) }
    static func exportBlob(_ r: KORAPIExportBlobRequest) throws -> KORAPIExportedBlob { try call(r, koru_api_store_export_blob, as: KORAPIExportedBlob.self) }
    static func releaseBlobs(_ r: KORAPIReleaseBlobsRequest) throws -> KORAPIReleasedBlobs { try call(r, koru_api_store_release_blobs, as: KORAPIReleasedBlobs.self) }
    static func recoverBlobs(_ r: KORAPIBlobRecoverRequest) throws -> KORAPIMutationResult { try call(r, koru_api_store_recover_blobs, as: KORAPIMutationResult.self) }
    static func blobArrivals(_ r: KORAPIBlobArrivalsRequest) throws -> KORAPIBlobArrivals { try call(r, koru_api_store_blob_arrivals, as: KORAPIBlobArrivals.self) }
    static func run(_ r: KORAPIRunRequest) throws -> KORAPIRunView { try call(r, koru_api_store_run, as: KORAPIRunView.self) }
    static func runTimeline(_ r: KORAPIRunTimelineRequest) throws -> KORAPIRunTimelineWindow { try call(r, koru_api_store_run_timeline, as: KORAPIRunTimelineWindow.self) }
    static func mutateWorkspace(_ r: KORAPIWorkspaceMutation) throws -> KORAPIMutationResult { try call(r, koru_api_store_mutate_workspace, as: KORAPIMutationResult.self) }
    static func mutateProfile(_ r: KORAPIProfileMutation) throws -> KORAPIMutationResult { try call(r, koru_api_store_mutate_profile, as: KORAPIMutationResult.self) }
    static func mutateDocument(_ r: KORAPIDocumentMutation) throws -> KORAPIMutationResult { try call(r, koru_api_store_mutate_document, as: KORAPIMutationResult.self) }
    static func importFile(_ r: KORAPIImportFileRequest) throws -> KORAPIDocumentIdResult { try call(r, koru_api_store_import_file, as: KORAPIDocumentIdResult.self) }
    static func ensureFileNote(_ r: KORAPIFileNoteRequest) throws -> KORAPIDocumentIdResult { try call(r, koru_api_store_ensure_file_note, as: KORAPIDocumentIdResult.self) }
    static func startPairing(_ r: KORAPIStartPairingRequest) throws -> KORAPIPairingOffer { try call(r, koru_api_start_pairing, as: KORAPIPairingOffer.self) }
    static func cancelPairing(_ r: KORAPICancelPairingRequest) throws -> KORAPIMutationResult { try call(r, koru_api_cancel_pairing, as: KORAPIMutationResult.self) }
    static func devices(_ r: KORAPIDevicesRequest) throws -> KORAPIDeviceList { try call(r, koru_api_devices, as: KORAPIDeviceList.self) }
    static func deviceHistory(_ r: KORAPIDeviceHistoryRequest) throws -> KORAPIDeviceHistory { try call(r, koru_api_device_history, as: KORAPIDeviceHistory.self) }
    static func unlinkDevice(_ r: KORAPIUnlinkDeviceRequest) throws -> KORAPIMutationResult { try call(r, koru_api_unlink_device, as: KORAPIMutationResult.self) }
    static func renameDevice(_ r: KORAPIRenameDeviceRequest) throws -> KORAPIMutationResult { try call(r, koru_api_rename_device, as: KORAPIMutationResult.self) }
    static func resetIdentity(_ r: KORAPIResetIdentityRequest) throws -> KORAPIMutationResult { try call(r, koru_api_reset_identity, as: KORAPIMutationResult.self) }
    static func setRelay(_ r: KORAPISetRelayRequest) throws -> KORAPIMutationResult { try call(r, koru_api_set_relay, as: KORAPIMutationResult.self) }
    static func accountCommand(_ r: KORAPIAccountCommandRequest) throws -> KORAPIAccountCommandReply { try call(r, koru_api_account_command, as: KORAPIAccountCommandReply.self) }
    static func accountPoll(_ r: KORAPIAccountPollRequest) throws -> KORAPIAccountPollReply { try call(r, koru_api_account_poll, as: KORAPIAccountPollReply.self) }
    static func accountPlatform(_ r: KORAPIAccountPlatformRequest) throws -> KORAPIMutationResult { try call(r, koru_api_account_platform, as: KORAPIMutationResult.self) }
}
