// SPDX-License-Identifier: MPL-2.0

import Foundation
import SwiftProtobuf

private func invalid(_ detail: String) -> AppFailure { AppFailure(code: .invalidData, detail: detail) }

enum ContractMapper {
    static func document(_ value: KORDocumentProjection) throws -> DocumentSnapshot {
        guard value.hasID, value.hasWorkspaceID else { throw invalid("document_identity") }
        let documentID = try uuid(value.id.value, "document_id")
        let workspaceID = try uuid(value.workspaceID.value, "workspace_id")
        guard !value.title.isEmpty else { throw invalid("document_title") }

        let ids = try value.blocks.map { block -> String in
            guard block.hasID else { throw invalid("block_id") }
            return try uuid(block.id.value, "block_id")
        }
        guard Set(ids).count == ids.count else { throw invalid("duplicate_block") }
        var emitted: Set<String> = []
        for block in value.blocks {
            if block.hasParentID {
                let parent = try uuid(block.parentID.value, "parent_id")
                guard ids.contains(parent) else { throw invalid("missing_parent") }
                guard emitted.contains(parent) else { throw invalid("parent_after_child") }
            }
            emitted.insert(try uuid(block.id.value, "block_id"))
        }

        return DocumentSnapshot(
            id: DocumentID(documentID),
            workspaceID: WorkspaceID(workspaceID),
            title: value.title,
            revision: value.revision,
            blocks: try value.blocks.map(block)
        )
    }

    private static func block(_ value: KORBlockEnvelope) throws -> Block {
        guard value.hasPayload, !value.payload.typeURL.isEmpty else { throw invalid("block_payload") }
        let properties = value.properties
        let mappedProperties = BlockProperties(
            name: properties.hasName ? properties.name : nil,
            icon: properties.hasIcon ? properties.icon : nil,
            color: properties.hasColor ? properties.color : nil,
            done: properties.done,
            alignment: alignment(properties.alignment),
            headingLevel: properties.hasHeadingLevel ? properties.headingLevel : nil,
            listStyle: listStyle(properties.listStyle),
            imageWidth: properties.hasImageSize ? properties.imageSize.width : nil,
            imageHeight: properties.hasImageSize ? properties.imageSize.height : nil,
            language: properties.hasLanguage ? properties.language : nil
        )
        let payload = value.payload
        let content: BlockContent
        switch value.kind {
        case .paragraph:
            try requireType(payload.typeURL, "koru.v1.ParagraphPayload")
            let decoded = try KORParagraphPayload(serializedBytes: payload.value)
            guard decoded.hasRichText else { throw invalid("rich_text") }
            content = .paragraph(try richText(decoded.richText))
        case .task:
            try requireType(payload.typeURL, "koru.v1.TaskPayload")
            let decoded = try KORTaskPayload(serializedBytes: payload.value)
            guard decoded.hasRichText else { throw invalid("rich_text") }
            content = .task(try richText(decoded.richText))
        case .list:
            try requireType(payload.typeURL, "koru.v1.ListPayload")
            _ = try KORListPayload(serializedBytes: payload.value)
            content = .list(mappedProperties.listStyle ?? .bullet)
        case .image:
            try requireType(payload.typeURL, "koru.v1.ImagePayload")
            let decoded = try KORImagePayload(serializedBytes: payload.value)
            guard decoded.hasBlob else { throw invalid("blob") }
            content = .image(try blob(decoded.blob))
        default:
            content = .unsupported(kindCode: value.kind.rawValue, typeURL: payload.typeURL, payload: payload.value)
        }
        return Block(
            id: BlockID(try uuid(value.id.value, "block_id")),
            parentID: value.hasParentID ? BlockID(try uuid(value.parentID.value, "parent_id")) : nil,
            properties: mappedProperties,
            content: content
        )
    }

    private static func richText(_ value: KORRichText) throws -> RichText {
        var anchors: Set<Int> = []
        for index in value.text.indices where value.text[index] == "\u{FFFC}" {
            anchors.insert(value.text[..<index].utf8.count)
        }
        let objects = try value.inlineObjects.map { object -> InlineObject in
            guard object.hasID, object.hasReference, !object.role.isEmpty else { throw invalid("inline_object") }
            return InlineObject(
                id: InlineObjectID(try uuid(object.id.value, "inline_object_id")),
                anchorUTF8: Int(object.anchorUtf8),
                role: object.role,
                reference: try reference(object.reference),
                label: object.hasLabel ? object.label : nil
            )
        }
        guard Set(objects.map(\.anchorUTF8)) == anchors, objects.count == anchors.count else { throw invalid("inline_anchor") }
        let marks = try value.marks.map { mark -> TextMark in
            guard mark.hasRange, !mark.kind.isEmpty else { throw invalid("text_mark") }
            let range = UTF8Range(start: Int(mark.range.startUtf8), end: Int(mark.range.endUtf8))
            _ = try stringRange(range, in: value.text)
            let kind = MarkKind(rawValue: mark.kind) ?? .unknown
            return TextMark(range: range, kind: kind, wireKind: mark.kind, value: mark.value)
        }
        return RichText(text: value.text, marks: marks, inlineObjects: objects)
    }

    private static func reference(_ value: KOREntityReference) throws -> EntityReference {
        switch value.target {
        case let .documentID(id): .document(DocumentID(try uuid(id.value, "document_id")))
        case let .blockID(id): .block(BlockID(try uuid(id.value, "block_id")))
        case let .userID(id): .user(try uuid(id.value, "user_id"))
        case let .url(url): .url(try required(url, "url"))
        case nil: throw invalid("entity_reference")
        }
    }

    private static func blob(_ value: KORBlobReference) throws -> BlobReference {
        guard value.hasID else { throw invalid("blob_id") }
        return BlobReference(id: BlobID(value: try required(value.id.value, "blob_id")), mimeType: try required(value.mimeType, "blob_mime_type"), size: value.size, width: value.hasWidth ? value.width : nil, height: value.hasHeight ? value.height : nil)
    }

    private static func alignment(_ value: KORAlignment) -> KoruAlignment {
        switch value { case .center: .center; case .end: .end; case .justify: .justify; default: .start }
    }
    private static func listStyle(_ value: KORListStyle) -> KoruListStyle? {
        switch value { case .bullet: .bullet; case .checkbox: .checkbox; case .numbered: .numbered; case .letter: .letter; case .roman: .roman; default: nil }
    }
    private static func stringRange(_ range: UTF8Range, in text: String) throws -> Range<String.Index> {
        guard range.start >= 0, range.start <= range.end, range.end <= text.utf8.count else { throw invalid("text_range") }
        let lowerUTF8 = text.utf8.index(text.utf8.startIndex, offsetBy: range.start)
        let upperUTF8 = text.utf8.index(text.utf8.startIndex, offsetBy: range.end)
        guard let lower = String.Index(lowerUTF8, within: text), let upper = String.Index(upperUTF8, within: text) else { throw invalid("text_range") }
        return lower..<upper
    }
    private static func requireType(_ typeURL: String, _ suffix: String) throws { guard typeURL.hasSuffix(suffix) else { throw invalid("block_payload_type") } }
    // Validates the shape and keeps the original spelling: a UUID round-trip
    // uppercases, and the wire id is the identity.
    private static func uuid(_ value: String, _ field: String) throws -> String { guard UUID(uuidString: value) != nil else { throw invalid(field) }; return value }
    private static func required(_ value: String, _ field: String) throws -> String { guard !value.isEmpty else { throw invalid(field) }; return value }
}
