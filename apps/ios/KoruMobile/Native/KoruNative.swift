// SPDX-License-Identifier: MPL-2.0

import Foundation

/// App-facing typed facade over the generated Protobuf/C binding.
/// All calls are synchronous; `KoruStoreCore` serializes them off the main actor.
enum KoruNative {
    static let deviceName = "iPhone"

    /// The UserDefaults key holding the device-local relay on/off value
    /// (iroh-relay roadmap, decision №7: platform-local storage owns it,
    /// never synced). Absent means the shipped default: relays on.
    static let relayEnabledKey = "relay_enabled"

    static var relayEnabled: Bool {
        (UserDefaults.standard.object(forKey: relayEnabledKey) as? Bool) ?? true
    }

    private static func host(root: String, secrets: Data) -> KORAPIHostContext {
        var value = KORAPIHostContext()
        value.root = root
        value.secrets = secrets
        value.deviceName = deviceName
        // Read on every call, applied only by the call that opens the host;
        // a live change goes through `setRelay`.
        value.relayEnabled = relayEnabled
        return value
    }

    static func status(root: String, secrets: Data) throws -> KORAPIHostStatus {
        var request = KORAPIHostStatusRequest()
        request.host = host(root: root, secrets: secrets)
        return try KoruApiNative.status(request)
    }

    static func link(root: String, secrets: Data, code: String) throws -> KORAPILinkDeviceResult {
        var request = KORAPILinkDeviceRequest()
        request.host = host(root: root, secrets: secrets)
        request.code = code
        return try KoruApiNative.linkDevice(request)
    }

    static func inventory(root: String, secrets: Data) throws -> KORAPIInventory {
        var request = KORAPIInventoryRequest()
        request.host = host(root: root, secrets: secrets)
        return try KoruApiNative.inventory(request)
    }

    static func document(root: String, secrets: Data, documentID: String) throws -> KORAPIDocumentProjection {
        var request = KORAPIDocumentRequest()
        request.host = host(root: root, secrets: secrets)
        request.documentID = documentID
        return try KoruApiNative.document(request)
    }

    static func blob(root: String, secrets: Data, workspaceID: String, hash: String) throws -> KORAPIBlobBytes {
        var request = KORAPIBlobRequest()
        request.host = host(root: root, secrets: secrets)
        request.workspaceID = workspaceID
        request.hash = hash
        return try KoruApiNative.blob(request)
    }

    static func mutateProfile(root: String, secrets: Data, profileID: String, action: String, value: String) throws -> KORAPIMutationResult {
        var request = KORAPIProfileMutation()
        request.host = host(root: root, secrets: secrets)
        request.profileID = profileID
        request.action = action
        request.value = value
        return try KoruApiNative.mutateProfile(request)
    }

    static func mutateWorkspace(root: String, secrets: Data, workspaceID: String, action: String, title: String) throws -> KORAPIMutationResult {
        var request = KORAPIWorkspaceMutation()
        request.host = host(root: root, secrets: secrets)
        request.workspaceID = workspaceID
        request.action = action
        request.title = title
        return try KoruApiNative.mutateWorkspace(request)
    }

    static func mutateDocument(root: String, secrets: Data, documentID: String, action: String, title: String) throws -> KORAPIMutationResult {
        var request = KORAPIDocumentMutation()
        request.host = host(root: root, secrets: secrets)
        request.documentID = documentID
        request.action = action
        request.title = title
        return try KoruApiNative.mutateDocument(request)
    }

    static func importFile(root: String, secrets: Data, workspaceID: String, name: String, mime: String, path: String, note: String? = nil) throws -> KORAPIDocumentIdResult {
        var request = KORAPIImportFileRequest()
        request.host = host(root: root, secrets: secrets)
        request.workspaceID = workspaceID
        request.name = name
        request.mime = mime
        request.path = path
        if let note { request.note = note }
        return try KoruApiNative.importFile(request)
    }

    static func ensureFileNote(root: String, secrets: Data, documentID: String) throws -> KORAPIDocumentIdResult {
        var request = KORAPIFileNoteRequest()
        request.host = host(root: root, secrets: secrets)
        request.documentID = documentID
        return try KoruApiNative.ensureFileNote(request)
    }

    static func startPairing(root: String, secrets: Data) throws -> KORAPIPairingOffer {
        var request = KORAPIStartPairingRequest()
        request.host = host(root: root, secrets: secrets)
        return try KoruApiNative.startPairing(request)
    }

    static func cancelPairing(root: String, secrets: Data) throws -> KORAPIMutationResult {
        var request = KORAPICancelPairingRequest()
        request.host = host(root: root, secrets: secrets)
        return try KoruApiNative.cancelPairing(request)
    }

    static func devices(root: String, secrets: Data) throws -> KORAPIDeviceList {
        var request = KORAPIDevicesRequest()
        request.host = host(root: root, secrets: secrets)
        return try KoruApiNative.devices(request)
    }

    static func history(root: String, secrets: Data, deviceID: String, before: Int64?) throws -> KORAPIDeviceHistory {
        var request = KORAPIDeviceHistoryRequest()
        request.host = host(root: root, secrets: secrets)
        request.deviceID = deviceID
        if let before { request.before = before }
        return try KoruApiNative.deviceHistory(request)
    }

    static func unlink(root: String, secrets: Data, deviceID: String) throws -> KORAPIMutationResult {
        var request = KORAPIUnlinkDeviceRequest()
        request.host = host(root: root, secrets: secrets)
        request.deviceID = deviceID
        return try KoruApiNative.unlinkDevice(request)
    }

    static func rename(root: String, secrets: Data, name: String) throws -> KORAPIMutationResult {
        var request = KORAPIRenameDeviceRequest()
        request.host = host(root: root, secrets: secrets)
        request.name = name
        return try KoruApiNative.renameDevice(request)
    }

    static func resetIdentity(root: String, secrets: Data) throws -> KORAPIMutationResult {
        var request = KORAPIResetIdentityRequest()
        request.host = host(root: root, secrets: secrets)
        return try KoruApiNative.resetIdentity(request)
    }

    /// Apply the relay on/off change to the live host — no app restart
    /// (decision №6). The caller persists the value in UserDefaults under
    /// `relayEnabledKey`; the next open reads it back through the context.
    static func setRelay(root: String, secrets: Data, enabled: Bool) throws -> KORAPIMutationResult {
        var request = KORAPISetRelayRequest()
        request.host = host(root: root, secrets: secrets)
        request.enabled = enabled
        return try KoruApiNative.setRelay(request)
    }

    static func advertiseBuild(root: String, secrets: Data, payload: BuildPayload) throws -> KORAPIMutationResult {
        var request = KORAPIBuildPayload()
        request.host = host(root: root, secrets: secrets)
        request.appVersion = payload.appVersion
        request.channel = payload.channel
        if let branch = payload.branch { request.branch = branch }
        if let commit = payload.commit { request.commit = commit }
        if let os = payload.os { request.os = os }
        if let version = payload.osVersion { request.osVersion = version }
        return try KoruApiNative.advertiseBuild(request)
    }
}
