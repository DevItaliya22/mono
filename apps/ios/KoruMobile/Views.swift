// SPDX-License-Identifier: MPL-2.0

import SwiftUI

private enum Route: Hashable { case workspaces(ProfileID), documents(WorkspaceID), document(DocumentID), devices, settings, diagnostics }

struct RootView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.scenePhase) private var scenePhase
    @State private var path: [Route] = []
    @State private var creator: Creator?
    @State private var renamingProfile: Profile?
    @State private var profileName = ""

    enum Creator: Identifiable, Hashable { case profile, workspace, document; var id: Self { self } }

    var body: some View {
        NavigationStack(path: $path) {
            EntityList(
                title: "profiles", rows: model.profiles,
                primary: { $0.displayName },
                secondary: { Locale.current.localizedString(forIdentifier: $0.locale) ?? $0.locale },
                leading: { AnyView(ProfileBadge(profile: $0)) },
                open: { profile in
                    model.selectProfile(profile.id); path.append(.workspaces(profile.id))
                },
                menu: { profile in
                    // Seeded with the raw name, not the display label, so a
                    // leading emoji is part of what the person edits rather
                    // than something the rename silently drops.
                    Button("rename", systemImage: "pencil") {
                        profileName = profile.displayName
                        renamingProfile = profile
                    }
                }
            )
            // The release version on the first screen, quiet and out of the
            // way — the Android profiles list carries the same label in the
            // same corner. It never takes a touch, so the row underneath it
            // still opens.
            .overlay(alignment: .bottomTrailing) {
                Text(verbatim: BuildPayload.stampLabel())
                    .font(.caption)
                    .foregroundStyle(KoruColor.subtle)
                    .padding(KoruSpace.medium)
                    .allowsHitTesting(false)
            }
            .alert(
                "rename",
                isPresented: Binding(
                    get: { renamingProfile != nil }, set: { if !$0 { renamingProfile = nil } })
            ) {
                TextField("name_or_title", text: $profileName)
                Button("cancel", role: .cancel) { renamingProfile = nil }
                Button("rename") {
                    if let profile = renamingProfile {
                        Task { await model.renameProfile(profile.id, displayName: profileName) }
                    }
                    renamingProfile = nil
                }
            }
            .toolbar {
                ToolbarItem(placement: .topBarLeading) { Button { path.append(.diagnostics) } label: { Label("developer", systemImage: "wrench.and.screwdriver") } }
                ToolbarItemGroup(placement: .topBarTrailing) {
                    if model.syncStatus.syncing { ProgressView() }
                    Button { path.append(.devices) } label: { Label("devices", systemImage: "iphone.radiowaves.left.and.right") }
                    Button { path.append(.settings) } label: { Label("settings", systemImage: "gearshape") }
                    Button { creator = .profile } label: { Label("create", systemImage: "plus") }
                }
            }
            .navigationDestination(for: Route.self) { route in
                switch route {
                case let .workspaces(id): WorkspaceList(profileID: id, path: $path, creator: $creator)
                case let .documents(id): DocumentList(workspaceID: id, path: $path, creator: $creator)
                case let .document(id):
                    DocumentView(documentID: id, onOpenDocument: { target in
                        model.openDocument(target)
                        path.append(.document(target))
                    })
                case .devices: DevicesScreen()
                case .settings: SettingsScreen()
                case .diagnostics: DiagnosticsView()
                }
            }
        }
        .sheet(item: $creator) { kind in CreationSheet(kind: kind) { value, locale in
            Task {
                switch kind {
                case .profile:
                    if let id = await model.createProfile(name: value, locale: locale), let row = model.profiles.first(where: { $0.id == id }) { model.selectProfile(row.id); path.append(.workspaces(row.id)) }
                case .workspace:
                    if let id = await model.createWorkspace(title: value) { model.selectWorkspace(id); path.append(.documents(id)) }
                case .document:
                    if let id = await model.createDocument(title: value) { model.openDocument(id); path.append(.document(id)) }
                }
                creator = nil
            }
        } }
        .alert("error_title", isPresented: Binding(get: { model.failure != nil }, set: { if !$0 { model.failure = nil } })) {
            Button("ok") { model.failure = nil }
        } message: { Text(localizedError(model.failure?.code ?? .unknown)) }
        .overlay { if model.loading { ProgressView().padding().background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12)) } }
        // The app half of the share extension (ADR-137): each activation
        // drains the App Group inbox, and a committed entry asks for its
        // destination here — the extension's process may not open the Store,
        // so this is the earliest a destination *can* be chosen. Swiping the
        // sheet away postpones; the inbox keeps the entry for next time.
        .task { model.drainShareInbox() }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { model.drainShareInbox() }
        }
        .sheet(
            isPresented: Binding(
                get: { !model.pendingShares.isEmpty },
                set: { if !$0 { model.postponeShares() } })
        ) {
            if let entry = model.pendingShares.first {
                ShareDestinationSheet(entry: entry)
            }
        }
    }
}

private struct ProfileBadge: View {
    let profile: Profile
    @Environment(\.colorScheme) private var colorScheme

    private var label: String {
        if let icon = profile.icon ?? profile.leadingEmoji, !icon.isEmpty { return icon }
        let initials = profile.displayName
            .split(whereSeparator: { $0.isWhitespace })
            .prefix(2)
            .compactMap(\.first)
        return initials.isEmpty ? "?" : String(initials).uppercased()
    }

    var body: some View {
        Text(verbatim: label)
            .font(.system(size: 13, weight: .medium))
            .frame(width: 28, height: 28)
            .foregroundStyle(.primary)
            .background(
                KoruEntityColor.color(profile.color, scheme: colorScheme) ?? KoruColor.surface,
                in: RoundedRectangle(cornerRadius: 7))
    }
}

private struct WorkspaceList: View {
    @EnvironmentObject private var model: AppModel
    let profileID: ProfileID
    @Binding var path: [Route]
    @Binding var creator: RootView.Creator?
    @State private var renaming: Workspace?
    @State private var renameTitle = ""
    @State private var debugging: Workspace?

    var body: some View {
        EntityList(
            title: "workspaces", rows: model.visibleWorkspaces,
            primary: { $0.displayTitle },
            secondary: { row in
                if row.deleted { return String(localized: "workspace_deleted") }
                if row.archived { return String(localized: "workspace_archived") }
                return String(localized: "workspace_subtitle")
            },
            open: { row in
                model.selectWorkspace(row.id); path.append(.documents(row.id))
            },
            menu: { row in
                Button("rename", systemImage: "pencil") { renameTitle = row.title; renaming = row }
                Button("debug_info", systemImage: "ladybug") { debugging = row }
                if row.archived {
                    Button("unarchive", systemImage: "tray.and.arrow.up") { Task { await model.setWorkspaceArchived(row.id, archived: false) } }
                } else {
                    Button("archive", systemImage: "archivebox") { Task { await model.setWorkspaceArchived(row.id, archived: true) } }
                }
                if row.deleted {
                    Button("restore", systemImage: "arrow.uturn.backward") { Task { await model.restoreWorkspace(row.id) } }
                } else {
                    Button("delete", systemImage: "trash", role: .destructive) { Task { await model.deleteWorkspace(row.id) } }
                }
            }
        )
        .navigationTitle("workspaces")
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                Menu {
                    Toggle("show_archived", isOn: $model.showArchivedWorkspaces)
                    Toggle("show_deleted", isOn: $model.showDeletedWorkspaces)
                } label: {
                    Label("filters", systemImage: "line.3.horizontal.decrease.circle")
                }
                Button { creator = .workspace } label: { Label("create", systemImage: "plus") }
            }
        }
        .alert("rename", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("name_or_title", text: $renameTitle)
            Button("cancel", role: .cancel) { renaming = nil }
            Button("rename") {
                if let row = renaming { Task { await model.renameWorkspace(row.id, title: renameTitle) } }
                renaming = nil
            }
        }
        .sheet(item: $debugging) { workspace in
            WorkspaceDebugSheet(workspace: workspace)
        }
        .task { model.selectProfile(profileID) }
    }
}

/// What the desktop's workspace details panel shows, on the phone — the ids
/// and states a sync question needs, with the workspace id one tap from a
/// bug report. Mirrors Android's Debug info dialog.
private struct WorkspaceDebugSheet: View {
    let workspace: Workspace
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                debugRow("workspace_id", workspace.id.value)
                debugRow("profile_id", workspace.profileID.value)
                debugRow("archived", workspace.archived ? "yes" : "no")
                debugRow(
                    "deleted",
                    workspace.deletedAtMillis.map {
                        Date(timeIntervalSince1970: Double($0) / 1000)
                            .formatted(.relative(presentation: .named))
                    } ?? "no")
                debugRow(
                    "modified",
                    workspace.modifiedAtMillis > 0
                        ? Date(timeIntervalSince1970: Double(workspace.modifiedAtMillis) / 1000)
                            .formatted(.relative(presentation: .named))
                        : "—")
                debugRow("icon", workspace.icon ?? "—")
                debugRow("color", workspace.color ?? "—")
                Button("copy_workspace_id", systemImage: "doc.on.doc") {
                    UIPasteboard.general.string = workspace.id.value
                }
            }
            .navigationTitle(workspace.displayTitle)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("done") { dismiss() }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func debugRow(_ label: LocalizedStringKey, _ value: String) -> some View {
        HStack {
            Text(label).font(.caption).foregroundStyle(KoruColor.subtle)
            Spacer()
            Text(verbatim: value)
                .font(.system(.caption, design: .monospaced))
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
        }
    }
}

private struct DocumentList: View {
    @EnvironmentObject private var model: AppModel
    let workspaceID: WorkspaceID
    @Binding var path: [Route]
    @Binding var creator: RootView.Creator?
    @State private var renaming: DocumentSummary?
    @State private var renameTitle = ""

    var body: some View {
        VStack(spacing: 0) {
            // The Android screen's PrimaryTabRow: one list, partitioned on
            // `kind == "file"`. Files list and sync; they do not open here.
            Picker("documents", selection: $model.documentProjection) {
                Text("documents").tag(AppModel.DocumentProjection.documents)
                Text("files").tag(AppModel.DocumentProjection.files)
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, KoruSpace.medium)
            EntityList(
                title: "documents", rows: model.visibleDocuments,
                primary: { $0.displayTitle },
                secondary: { row in
                    row.kind == "file"
                        ? (row.mimeType ?? String(localized: "file_subtitle"))
                        : String(localized: "document_subtitle")
                },
                trailing: { row in
                    // A file's note opens like any other document; back returns
                    // here. Borderless so the tap is the icon's, not the row's.
                    guard let noteId = row.noteId else { return nil }
                    return AnyView(
                        Button {
                            model.openDocument(noteId); path.append(.document(noteId))
                        } label: {
                            Image(systemName: "note.text")
                        }
                        .buttonStyle(.borderless)
                        .accessibilityLabel(Text("open_note"))
                    )
                },
                open: { row in
                    guard model.canOpen(row) else { return }
                    model.openDocument(row.id); path.append(.document(row.id))
                },
                menu: { row in
                    Button("rename", systemImage: "pencil") { renameTitle = row.title; renaming = row }
                    if let noteId = row.noteId {
                        Button("open_note", systemImage: "note.text") {
                            model.openDocument(noteId); path.append(.document(noteId))
                        }
                    } else if row.kind == "file" {
                        Button("add_note", systemImage: "note.text.badge.plus") {
                            Task { await model.openNote(for: row) { path.append(.document($0)) } }
                        }
                    }
                    Button("move_to_trash", systemImage: "trash", role: .destructive) { Task { await model.trashDocument(row.id) } }
                }
            )
        }
        .navigationTitle("documents")
        .toolbar { Button { creator = .document } label: { Label("create", systemImage: "plus") } }
        .alert("rename", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("name_or_title", text: $renameTitle)
            Button("cancel", role: .cancel) { renaming = nil }
            Button("rename") {
                if let row = renaming { Task { await model.renameDocument(row.id, title: renameTitle) } }
                renaming = nil
            }
        }
        .task { model.selectWorkspace(workspaceID) }
    }
}

private struct EntityList<Item: Identifiable & Equatable, Content: StringProtocol, RowMenu: View>: View {
    let title: LocalizedStringKey
    let rows: [Item]
    let primary: (Item) -> Content
    let secondary: (Item) -> String
    let leading: (Item) -> AnyView?
    let trailing: (Item) -> AnyView?
    let open: (Item) -> Void
    /// The row's long-press actions — the Android screens' action sheet.
    @ViewBuilder let menu: (Item) -> RowMenu

    init(
        title: LocalizedStringKey, rows: [Item],
        primary: @escaping (Item) -> Content,
        secondary: @escaping (Item) -> String,
        leading: @escaping (Item) -> AnyView? = { _ in nil },
        trailing: @escaping (Item) -> AnyView? = { _ in nil },
        open: @escaping (Item) -> Void,
        @ViewBuilder menu: @escaping (Item) -> RowMenu
    ) {
        self.title = title
        self.rows = rows
        self.primary = primary
        self.secondary = secondary
        self.leading = leading
        self.trailing = trailing
        self.open = open
        self.menu = menu
    }

    var body: some View {
        Group {
            if rows.isEmpty { KoruEmptyState(title: "empty_state") }
            else {
                List(rows) { row in
                    Button { open(row) } label: {
                        KoruRow {
                            HStack(spacing: KoruSpace.small) {
                                if let leading = leading(row) { leading }
                                VStack(alignment: .leading, spacing: KoruSpace.xSmall) {
                                    Text(verbatim: String(primary(row))).font(KoruType.body).foregroundStyle(.primary)
                                    Text(verbatim: secondary(row)).font(.caption).foregroundStyle(KoruColor.subtle)
                                }
                                Spacer(minLength: KoruSpace.small)
                                if let trailing = trailing(row) { trailing }
                            }
                        }
                    }
                        .contextMenu { menu(row) }
                }
            }
        }
        .navigationTitle(title)
    }
}

extension EntityList where RowMenu == EmptyView {
    init(
        title: LocalizedStringKey, rows: [Item],
        primary: @escaping (Item) -> Content,
        secondary: @escaping (Item) -> String,
        leading: @escaping (Item) -> AnyView? = { _ in nil },
        trailing: @escaping (Item) -> AnyView? = { _ in nil },
        open: @escaping (Item) -> Void
    ) {
        self.init(
            title: title, rows: rows, primary: primary, secondary: secondary,
            leading: leading, trailing: trailing, open: open
        ) { _ in EmptyView() }
    }
}

private struct CreationSheet: View {
    let kind: RootView.Creator
    let create: (String, String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var value = ""
    @State private var locale = Locale.current.identifier.replacingOccurrences(of: "_", with: "-")
    var body: some View {
        NavigationStack {
            Form {
                TextField("name_or_title", text: $value)
                if kind == .profile { TextField("locale", text: $locale).textInputAutocapitalization(.never) }
            }
            .navigationTitle("create")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button("create") { create(value, locale) }.disabled(value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty) }
            }
        }
    }
}

struct DocumentView: View {
    @EnvironmentObject private var model: AppModel
    let documentID: DocumentID
    /// How this surface opens *another* document — the host owns navigation
    /// (RootView appends a route; the device-history sheet passes nothing and
    /// its pills stay inert). Same split as the desktop's search adapter.
    var onOpenDocument: ((DocumentID) -> Void)? = nil
    var body: some View {
        Group {
            if let document = model.snapshot, document.id == documentID {
                // A list container is structure, not a row: its style paints
                // its children's markers instead — the Android viewer filters
                // the same way.
                let visible = document.blocks.filter {
                    if case .list = $0.content { return false } else { return true }
                }
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: KoruSpace.medium) {
                        Text(document.title).font(KoruType.title).frame(maxWidth: .infinity, alignment: .leading)
                        ForEach(visible) { block in
                            BlockView(
                                block: block,
                                depth: depth(of: block, in: document.blocks),
                                listStyle: parentListStyle(of: block, in: document.blocks),
                                ordinal: ordinal(of: block, in: document.blocks),
                                follow: follow
                            )
                            .padding(.leading, CGFloat(depth(of: block, in: document.blocks)) * KoruSpace.medium)
                            // The heading ramp's space above (28/24/20/16/14/12)
                            // is the *whole* gap the desktop puts over a
                            // heading; the stack already lays every row out on
                            // one, so only the difference is added and a body
                            // row keeps the plain gap. There is none below —
                            // the next row brings its own line box.
                            .padding(
                                .top,
                                max(
                                    0,
                                    documentHeadingSpaceAbove(block.properties.headingLevel)
                                        - KoruSpace.medium))
                        }
                        addControls
                    }.padding(KoruSpace.medium)
                }
            } else { ProgressView() }
        }
        .navigationTitle(model.snapshot?.title ?? String(localized: "document_subtitle"))
        .navigationBarTitleDisplayMode(.inline)
        .task { model.openDocument(documentID) }
    }

    private func depth(of block: Block, in blocks: [Block]) -> Int {
        var result = 0; var parent = block.parentID; var visited: Set<BlockID> = []
        while let id = parent, visited.insert(id).inserted, let row = blocks.first(where: { $0.id == id }) { result += 1; parent = row.parentID }
        return result
    }

    /// What tapping a pill does. A reference to another document navigates
    /// through the host's closure; a URL goes to the browser (http/https only —
    /// a URL out of a document is remote input, the editor's allow-list rule);
    /// anything else stays inert for now.
    private func follow(_ reference: EntityReference) {
        switch reference {
        case let .document(target):
            if target != documentID { onOpenDocument?(target) }
        case let .block(_, documentID: target):
            if let target, target != documentID { onOpenDocument?(target) }
        case let .url(value):
            if let url = URL(string: value), ["http", "https"].contains(url.scheme?.lowercased()) {
                UIApplication.shared.open(url)
            }
        case .user, .label: break
        }
    }

    private func parentListStyle(of block: Block, in blocks: [Block]) -> KoruListStyle? {
        guard let parentID = block.parentID,
              let parent = blocks.first(where: { $0.id == parentID }),
              case let .list(style) = parent.content else { return nil }
        return style
    }

    private func ordinal(of block: Block, in blocks: [Block]) -> Int {
        let siblings = blocks.filter { $0.parentID == block.parentID }
        return (siblings.firstIndex { $0.id == block.id } ?? 0) + 1
    }

    private var addControls: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack {
                Button("add_paragraph", systemImage: "text.alignleft") { model.addBlock(.paragraph(RichText(text: ""))) }
                Button("add_task", systemImage: "checkmark.square") { model.addBlock(.task(RichText(text: ""))) }
                Button("add_list", systemImage: "list.bullet") { model.addBlock(.list(.bullet)) }
                Button("add_image", systemImage: "photo") { model.addImage() }
            }.buttonStyle(.bordered)
        }
    }
}

private struct BlockView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.layoutDirection) private var layoutDirection
    // Read, not merely declared: the ramp is scaled through `UIFontMetrics`,
    // which asks UIKit rather than this environment, so reading the size here
    // is what makes SwiftUI rebuild the block when somebody moves the slider.
    @Environment(\.dynamicTypeSize) private var typeSize
    let block: Block
    let depth: Int
    var listStyle: KoruListStyle?
    var ordinal: Int = 1
    var follow: (EntityReference) -> Void = { _ in }

    /// The block's own face, at the reader's text size.
    private var baseFont: UIFont {
        documentBlockFont(
            headingLevel: block.properties.headingLevel,
            sizeCategory: UIContentSizeCategory(typeSize))
    }

    private var textAlignment: NSTextAlignment {
        documentTextAlignment(block.properties.alignment, layoutDirection: layoutDirection)
    }

    var body: some View {
        HStack(alignment: .top, spacing: KoruSpace.small) {
            marker
            content
        }
        .contextMenu {
            Button("indent", systemImage: "increase.indent") { model.execute(.indentBlock(block.id)) }
            Button("outdent", systemImage: "decrease.indent") { model.execute(.outdentBlock(block.id)) }
            Button("delete", systemImage: "trash", role: .destructive) { model.execute(.deleteBlock(block.id)) }
        }
    }

    private var isTask: Bool { if case .task = block.content { return true } else { return false } }

    // Every marker hangs in one lane of `documentMarkerLane`, whatever its
    // style, so the text column starts in the same place on a bullet row, a
    // numbered row and a checkbox row of the same list — and a long ordinal
    // (`mcmxciv.`) no longer shoves its own row's text sideways. Markers stay
    // body size under a heading: DESIGN says the ordinals are body-size text.
    @ViewBuilder private var marker: some View {
        if isTask || listStyle == .checkbox {
            Button { model.execute(.toggleTask(block.id, !block.properties.done)) } label: { Image(systemName: block.properties.done ? "checkmark.square.fill" : "square") }
                .frame(width: documentMarkerLane, alignment: .center)
        } else if listStyle == .bullet {
            Text("•").frame(width: documentMarkerLane, alignment: .center)
        } else if let style = listStyle {
            Text(listMarkerLabel(style, ordinal: ordinal))
                .monospacedDigit()
                .lineLimit(1)
                .frame(width: documentMarkerLane, alignment: .trailing)
                .foregroundStyle(KoruColor.subtle)
        } else if case .quote = block.content {
            Rectangle().fill(KoruColor.subtle).frame(width: 3).padding(.vertical, 2)
        }
        // A plain row has no marker lane at all — an empty spacer here pushed
        // every paragraph 18pt right of the title (Android gives plain rows
        // no lane either; markers indent their own rows).
    }

    @ViewBuilder private var content: some View {
        switch block.content {
        case let .paragraph(rich), let .task(rich):
            richText(rich, font: baseFont)
        // A quote is prose with a semantic frame (DESIGN): it keeps the block's
        // own type and adds the start-edge rule the marker lane draws, so it
        // goes down the ordinary paragraph path — which is also how it keeps
        // its marks, its pills and its alignment instead of arriving flat.
        case let .quote(rich):
            richText(rich, font: italicized(baseFont))
        case let .code(rich):
            VStack(alignment: .leading, spacing: KoruSpace.xSmall) {
                if let language = block.properties.language,
                    !language.trimmingCharacters(in: .whitespaces).isEmpty
                {
                    Text(verbatim: language).font(.caption2).foregroundStyle(KoruColor.accent)
                }
                Text(rich.text).font(.system(.body, design: .monospaced))
            }
            .padding(KoruSpace.small).frame(maxWidth: .infinity, alignment: .leading)
            .background(KoruColor.surface, in: RoundedRectangle(cornerRadius: 6))
        case let .table(rich): TableBlock(source: rich.text)
        case let .contact(name): Label(name.text, systemImage: "person.crop.circle")
        case .divider:
            // SwiftUI's `Divider` turns vertical inside an HStack — this row is
            // one, for the marker lane — which left document dividers invisible.
            // The rule is drawn explicitly instead.
            Rectangle().fill(KoruColor.subtle.opacity(0.4))
                .frame(height: 1).frame(maxWidth: .infinity)
                .padding(.vertical, KoruSpace.small)
        case let .embed(reference): Label(embedLabel(reference), systemImage: "link").foregroundStyle(KoruColor.subtle).onTapGesture { follow(reference) }
        case .list: EmptyView() // containers are filtered out of the visible rows
        case let .image(blob), let .canvas(blob): BlobImage(reference: blob)
        case let .unsupported(kind, _, _): Label(String(format: String(localized: "unsupported_block"), locale: .current, kind), systemImage: "questionmark.diamond").foregroundStyle(KoruColor.subtle)
        }
    }

    /// The one rich-text path: paragraph, task and quote differ by their face,
    /// never by whether marks, pills and alignment survive.
    private func richText(_ rich: RichText, font: UIFont) -> some View {
        NativeRichTextEditor(
            richText: rich, baseFont: font, alignment: textAlignment, onFollow: follow
        ) { edit in
            switch edit {
            case let .insert(offset, text): model.execute(.insertText(block.id, atUTF8: offset, text: text))
            case let .delete(range): model.execute(.deleteText(block.id, range: range))
            }
        }.frame(minHeight: 28)
    }

    private func embedLabel(_ reference: EntityReference) -> String {
        switch reference {
        case let .url(value): value
        case let .label(value): value
        case .document, .block, .user: String(localized: "document_subtitle")
        }
    }
}

/// A table, drawn as a table. The source is the Markdown a table block stores;
/// `parseMarkdownTable` is Android's reader, ported, so both phones cut the
/// same string into the same cells. A source that is not a table parses to
/// nothing and is shown as its own monospaced text rather than as an error —
/// the same fallback Android takes.
private struct TableBlock: View {
    let source: String

    var body: some View {
        let rows = parseMarkdownTable(source)
        if rows.isEmpty {
            Text(verbatim: source).font(.system(.body, design: .monospaced))
        } else {
            ScrollView(.horizontal, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(rows.enumerated()), id: \.offset) { index, cells in
                        HStack(alignment: .top, spacing: 0) {
                            ForEach(Array(cells.enumerated()), id: \.offset) { _, cell in
                                Text(verbatim: cell)
                                    .font(KoruType.body)
                                    .fontWeight(index == 0 ? .semibold : .regular)
                                    .frame(minWidth: 112, alignment: .leading)
                                    .padding(KoruSpace.small)
                            }
                        }
                        if index == 0 {
                            Rectangle().fill(KoruColor.subtle.opacity(0.4)).frame(height: 1)
                        }
                    }
                }
            }
            .background(KoruColor.surface, in: RoundedRectangle(cornerRadius: 6))
        }
    }
}

/// The six hydration states, drawn — ADR-128's contract: an image whose bytes
/// have not arrived is a placeholder, never an error, and retry is a button
/// rather than a timer. The Android mirror is `BlobImage` in
/// `DocumentEditor.kt`.
private struct BlobImage: View {
    @EnvironmentObject private var model: AppModel
    let reference: BlobReference
    @State private var state: BlobLoadState = .checkingLocal
    @State private var attempt = 0

    var body: some View {
        Group {
            switch state {
            case let .available(bytes):
                if let image = UIImage(data: bytes) {
                    Image(uiImage: image).resizable().scaledToFit().frame(maxHeight: 220)
                } else {
                    Label("document_image", systemImage: "photo").foregroundStyle(KoruColor.subtle)
                }
            case .checkingLocal, .fetching:
                HStack(spacing: KoruSpace.small) { ProgressView(); Text("image_downloading").font(.caption).foregroundStyle(KoruColor.subtle) }
            case .noLinkedDevices:
                Label("image_link_device", systemImage: "iphone.slash").font(.caption).foregroundStyle(KoruColor.subtle)
            case let .waitingForPeer(name):
                Label(String(format: String(localized: "image_waiting_for_device"), locale: .current, name ?? "-"), systemImage: "wifi.slash").font(.caption).foregroundStyle(KoruColor.subtle)
            case let .failed(_, retryable):
                HStack(spacing: KoruSpace.small) {
                    Label("image_unavailable", systemImage: "photo.badge.exclamationmark").font(.caption).foregroundStyle(KoruColor.subtle)
                    if retryable { Button("retry") { attempt += 1 }.font(.caption) }
                }
            }
        }
        .task(id: attempt) {
            for await value in model.services.blobs.observeLoad(reference) { state = value }
        }
    }
}

private struct DiagnosticsView: View {
    @EnvironmentObject private var model: AppModel
    var body: some View {
        List {
            diagnostic("platform", "iOS")
            diagnostic("app_version", Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "-")
            diagnostic("contract_version", "koru.v1")
            diagnostic("active_profile", model.activeProfileID?.value ?? "-")
            diagnostic("active_workspace", model.activeWorkspaceID?.value ?? "-")
            diagnostic("active_document", model.activeDocumentID?.value ?? "-")
            diagnostic("locale", Locale.current.identifier.replacingOccurrences(of: "_", with: "-"))
            diagnostic("service_implementation", model.services.implementation)
        }.navigationTitle("developer")
    }
    private func diagnostic(_ key: LocalizedStringKey, _ value: String) -> some View { VStack(alignment: .leading) { Text(key).font(.caption).foregroundStyle(KoruColor.subtle); Text(value).font(.system(.body, design: .monospaced)).textSelection(.enabled) } }
}

private func localizedError(_ code: AppErrorCode) -> LocalizedStringKey {
    switch code {
    case .notFound: "error_not_found"
    case .permissionDenied: "error_permission"
    case .unavailable: "error_unavailable"
    case .invalidData: "error_invalid"
    case .unsupported: "error_unsupported"
    case .unknown: "error_unknown"
    }
}
