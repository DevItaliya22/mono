// SPDX-License-Identifier: MPL-2.0

import Foundation

// IDs are String-backed, the way Android's are: the wire — protobuf and the
// native JSON alike — carries UUID strings, and a UUID round-trip both
// uppercases and rejects the non-UUID ids a blob hash already proved possible.
struct ProfileID: Hashable, Codable, Sendable {
    let value: String
    init(_ value: String = UUID().uuidString.lowercased()) { self.value = value }
}
struct WorkspaceID: Hashable, Codable, Sendable {
    let value: String
    init(_ value: String = UUID().uuidString.lowercased()) { self.value = value }
}
struct DocumentID: Hashable, Codable, Sendable {
    let value: String
    init(_ value: String = UUID().uuidString.lowercased()) { self.value = value }
}
struct BlockID: Hashable, Codable, Sendable {
    let value: String
    init(_ value: String = UUID().uuidString.lowercased()) { self.value = value }
}
struct InlineObjectID: Hashable, Codable, Sendable {
    let value: String
    init(_ value: String = UUID().uuidString.lowercased()) { self.value = value }
}
struct BlobID: Hashable, Codable, Sendable { let value: String }

struct Profile: Identifiable, Equatable, Sendable {
    let id: ProfileID
    let displayName: String
    let locale: String
    var icon: String? = nil
    var leadingEmoji: String? = nil
    var displayLabel: String = ""
    var color: String? = nil

    init(
        id: ProfileID, displayName: String, locale: String,
        icon: String? = nil, leadingEmoji: String? = nil,
        displayLabel: String? = nil, color: String? = nil
    ) {
        self.id = id
        self.displayName = displayName
        self.locale = locale
        self.icon = icon
        self.leadingEmoji = leadingEmoji
        self.displayLabel = displayLabel ?? displayName
        self.color = color
    }
}

struct Workspace: Identifiable, Equatable, Sendable {
    let id: WorkspaceID
    let profileID: ProfileID
    let title: String
    var archived = false
    var deletedAtMillis: Int64? = nil
    var modifiedAtMillis: Int64 = 0
    var icon: String? = nil
    var color: String? = nil
    var leadingEmoji: String? = nil
    var displayTitle: String = ""

    init(
        id: WorkspaceID, profileID: ProfileID, title: String,
        archived: Bool = false, deletedAtMillis: Int64? = nil, modifiedAtMillis: Int64 = 0,
        icon: String? = nil, color: String? = nil, leadingEmoji: String? = nil,
        displayTitle: String? = nil
    ) {
        self.id = id
        self.profileID = profileID
        self.title = title
        self.archived = archived
        self.deletedAtMillis = deletedAtMillis
        self.modifiedAtMillis = modifiedAtMillis
        self.icon = icon
        self.color = color
        self.leadingEmoji = leadingEmoji
        self.displayTitle = displayTitle ?? title
    }

    var deleted: Bool { deletedAtMillis != nil }
}

struct DocumentSummary: Identifiable, Equatable, Sendable {
    let id: DocumentID
    let workspaceID: WorkspaceID
    let title: String
    var displayTitle: String = ""
    var leadingEmoji: String? = nil
    /// `"file"` rows list in the Files tab and do not open; everything else is
    /// a document. The partition the Documents screen keys on.
    var kind: String = "document"
    var mimeType: String? = nil
    /// The file's one note, when it has one. Notes open through the file's
    /// row, never as a row of their own.
    var noteId: DocumentID? = nil

    init(
        id: DocumentID, workspaceID: WorkspaceID, title: String,
        displayTitle: String? = nil, leadingEmoji: String? = nil,
        kind: String = "document", mimeType: String? = nil,
        noteId: DocumentID? = nil
    ) {
        self.id = id
        self.workspaceID = workspaceID
        self.title = title
        self.displayTitle = displayTitle ?? title
        self.leadingEmoji = leadingEmoji
        self.kind = kind
        self.mimeType = mimeType
        self.noteId = noteId
    }
}

struct BlobReference: Equatable, Sendable {
    let id: BlobID
    let mimeType: String
    let size: UInt64
    var width: UInt32? = nil
    var height: UInt32? = nil
    /// The workspace whose peers can hydrate these bytes; loading needs it.
    var workspaceID: WorkspaceID? = nil
}

struct UTF8Range: Equatable, Sendable { let start: Int; let end: Int }

enum MarkKind: String, Sendable {
    case bold, italic, underline, strike, code, superscript, `subscript`, size, highlight, unknown
}

struct TextMark: Equatable, Sendable {
    let range: UTF8Range
    let kind: MarkKind
    let wireKind: String
    let value: Data

    init(range: UTF8Range, kind: MarkKind, wireKind: String? = nil, value: Data = Data()) {
        self.range = range
        self.kind = kind
        self.wireKind = wireKind ?? kind.rawValue
        self.value = value
    }
}

enum EntityReference: Equatable, Sendable {
    case document(DocumentID)
    case block(BlockID, documentID: DocumentID? = nil)
    case user(String)
    case url(String)
    case label(String)
}

struct InlineObject: Identifiable, Equatable, Sendable {
    let id: InlineObjectID
    let anchorUTF8: Int
    let role: String
    let reference: EntityReference
    let label: String?
}

struct RichText: Equatable, Sendable {
    var text: String
    var marks: [TextMark] = []
    var inlineObjects: [InlineObject] = []
}

enum KoruAlignment: Sendable { case start, center, end, justify }
enum KoruListStyle: Sendable { case bullet, checkbox, numbered, letter, roman }

struct BlockProperties: Equatable, Sendable {
    var name: String?
    var icon: String?
    var color: String?
    var done = false
    var alignment: KoruAlignment = .start
    var headingLevel: UInt32?
    var listStyle: KoruListStyle?
    var imageWidth: UInt32?
    var imageHeight: UInt32?
    var language: String?
}

enum BlockContent: Equatable, Sendable {
    case paragraph(RichText)
    case quote(RichText)
    case task(RichText)
    case list(KoruListStyle)
    case image(BlobReference)
    case canvas(BlobReference)
    case code(RichText)
    case table(RichText)
    case divider
    case embed(EntityReference)
    case contact(RichText)
    case unsupported(kindCode: Int, typeURL: String, payload: Data)
}

struct Block: Identifiable, Equatable, Sendable {
    let id: BlockID
    var parentID: BlockID?
    var properties: BlockProperties = BlockProperties()
    var content: BlockContent

    init(
        id: BlockID, parentID: BlockID? = nil,
        properties: BlockProperties = BlockProperties(), content: BlockContent
    ) {
        self.id = id
        self.parentID = parentID
        self.properties = properties
        self.content = content
    }
}

struct DocumentSnapshot: Equatable, Sendable {
    let id: DocumentID
    let workspaceID: WorkspaceID
    var title: String
    var revision: UInt64
    var blocks: [Block]
}

enum AppErrorCode: String, Sendable { case notFound, permissionDenied, unavailable, invalidData, unsupported, unknown }
struct AppFailure: LocalizedError, Equatable, Sendable {
    let code: AppErrorCode
    let detail: String
    var errorDescription: String? { detail }
}

enum DocumentCommand: Equatable, Sendable {
    case createBlock(Block, siblingIndex: Int)
    case deleteBlock(BlockID)
    case moveBlock(BlockID, parentID: BlockID?, siblingIndex: Int)
    case indentBlock(BlockID)
    case outdentBlock(BlockID)
    case insertText(BlockID, atUTF8: Int, text: String)
    case deleteText(BlockID, range: UTF8Range)
    case addMark(BlockID, TextMark)
    case removeMark(BlockID, UTF8Range, MarkKind)
    case toggleTask(BlockID, Bool)
    case insertInlineObject(BlockID, InlineObject)
    case removeInlineObject(BlockID, InlineObjectID)
    case attachBlob(BlockID, BlobReference)
    case renameDocument(String)
}

enum DocumentEvent: Equatable, Sendable {
    case changed(revision: UInt64, blocks: Set<BlockID>)
    case renamed(String)
    case blobAvailable(BlobID)
    case blobUnavailable(BlobID)
    case sessionError(AppFailure)
    case unknown(typeURL: String, payload: Data)
}
