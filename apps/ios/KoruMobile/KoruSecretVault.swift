// SPDX-License-Identifier: MPL-2.0

import Foundation
import Security

/// The iOS half of the seed contract `AndroidSecretVault` defines: 64 bytes —
/// 32 of Ed25519 device seed, 32 of iroh endpoint seed — minted once and
/// handed to the native host on every call.
///
/// The bytes live in the Keychain as a generic password:
/// `AfterFirstUnlockThisDeviceOnly` because sync may run in the background but
/// the seeds must never leave the device, and `kSecAttrSynchronizable = false`
/// because a seed that roamed through iCloud Keychain would be two devices
/// claiming one identity. The Store root itself is excluded from backup — the
/// mirror of Android's `noBackupFilesDir` — so a restored backup cannot carry
/// replica databases whose seeds died with the old device's Keychain item.
struct KoruSecretVault: Sendable {
    private static let service = "koru-mobile-device-seeds-v1"
    private static let account = "device"

    enum VaultError: Error { case status(OSStatus), corrupt }

    /// The stored seeds, minted on first ask. Never regenerates over an
    /// existing item — a re-mint is a new device identity.
    func loadOrCreate() throws -> Data {
        if let existing = try load() {
            guard existing.count == 64 else { throw VaultError.corrupt }
            return existing
        }
        var bytes = Data(count: 64)
        let status = bytes.withUnsafeMutableBytes { buffer in
            SecRandomCopyBytes(kSecRandomDefault, 64, buffer.baseAddress!)
        }
        guard status == errSecSuccess else { throw VaultError.status(status) }
        try store(bytes)
        return bytes
    }

    private func load() throws -> Data? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.service,
            kSecAttrAccount as String: Self.account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        switch status {
        case errSecSuccess: return item as? Data
        case errSecItemNotFound: return nil
        default: throw VaultError.status(status)
        }
    }

    private func store(_ bytes: Data) throws {
        let attributes: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.service,
            kSecAttrAccount as String: Self.account,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            kSecAttrSynchronizable as String: false,
            kSecValueData as String: bytes,
        ]
        let status = SecItemAdd(attributes as CFDictionary, nil)
        // A concurrent first launch may have won the race; the stored item is
        // the identity then, not ours.
        guard status == errSecSuccess || status == errSecDuplicateItem else {
            throw VaultError.status(status)
        }
    }

    /// Where the embedded Store lives: Application Support, excluded from
    /// backup so replica databases cannot outlive the this-device-only seeds.
    static func storeRoot() throws -> URL {
        let support = try FileManager.default.url(
            for: .applicationSupportDirectory, in: .userDomainMask,
            appropriateFor: nil, create: true)
        // Data identity is not the desktop host's executable name. Keep the
        // existing directory so the host rename cannot strand an iOS Store.
        var root = support.appendingPathComponent("koru-store", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try root.setResourceValues(values)
        return root
    }
}
