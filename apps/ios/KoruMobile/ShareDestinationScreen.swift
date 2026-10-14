// SPDX-License-Identifier: MPL-2.0

import SwiftUI

/// Where a share lands: the recent destinations first (one tap saves), then
/// profile first, then that profile's workspaces — the same two-step
/// Android's share activity shows, arriving here on the app's next activation
/// instead because the extension may not open the Store (ADR-137). Each staged
/// file's name is editable before saving.
struct ShareDestinationSheet: View {
    @EnvironmentObject private var model: AppModel
    let entry: ShareInbox.Entry
    @State private var profileID: ProfileID?
    @State private var workspaceID: WorkspaceID?
    @State private var saving = false
    /// The name each staged file saves under, seeded with the staged names.
    @State private var names: [String]
    /// The note each staged file saves with, seeded with any staged note;
    /// empty means no note.
    @State private var notes: [String]
    /// The recorded destinations that still exist, most recent first.
    @State private var recents: [RecentShareDestination] = []

    init(entry: ShareInbox.Entry) {
        self.entry = entry
        _names = State(initialValue: entry.manifest.files.map(\.name))
        _notes = State(initialValue: entry.manifest.files.map { $0.note ?? "" })
    }

    var body: some View {
        NavigationStack {
            List {
                if !recents.isEmpty {
                    Section("share_recent") {
                        ForEach(recents) { recent in
                            Button {
                                save(into: recent.workspace.id)
                            } label: {
                                VStack(alignment: .leading, spacing: KoruSpace.xSmall) {
                                    Text(verbatim: recent.workspace.displayTitle)
                                        .font(KoruType.body).foregroundStyle(.primary)
                                    if let profile = recent.profile {
                                        Text(verbatim: profile.displayLabel)
                                            .font(.caption).foregroundStyle(KoruColor.subtle)
                                    }
                                }
                            }
                            .disabled(saving)
                        }
                    }
                }
                Section("share_incoming") {
                    ForEach(Array(entry.manifest.files.enumerated()), id: \.element.file) { index, file in
                        VStack(alignment: .leading, spacing: KoruSpace.xSmall) {
                            TextField("name_or_title", text: nameBinding(at: index, fallback: file.name))
                            TextField("note_optional", text: noteBinding(at: index, fallback: file.note ?? ""), axis: .vertical)
                                .lineLimit(1...4)
                            Text(file.mimeType).font(.caption).foregroundStyle(KoruColor.subtle)
                        }
                    }
                }
                Section("profiles") {
                    ForEach(model.profiles) { profile in
                        selectableRow(profile.displayLabel, selected: profileID == profile.id) {
                            profileID = profile.id
                            workspaceID = nil
                            model.selectProfile(profile.id)
                        }
                    }
                }
                if profileID != nil {
                    Section("workspaces") {
                        ForEach(model.visibleWorkspaces.filter { !$0.archived && !$0.deleted }) { workspace in
                            selectableRow(workspace.displayTitle, selected: workspaceID == workspace.id) {
                                workspaceID = workspace.id
                            }
                        }
                    }
                }
            }
            .navigationTitle("share_destination")
            .navigationBarTitleDisplayMode(.inline)
            .task { recents = await model.recentShareDestinations() }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("share_discard", role: .destructive) {
                        model.discardShare(entry)
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if saving {
                        ProgressView()
                    } else {
                        Button("share_save") {
                            guard let workspaceID else { return }
                            save(into: workspaceID)
                        }
                        .disabled(workspaceID == nil)
                    }
                }
            }
        }
    }

    private func save(into workspace: WorkspaceID) {
        saving = true
        Task {
            await model.importShare(entry, into: workspace, names: names, notes: notes)
            saving = false
        }
    }

    private func nameBinding(at index: Int, fallback: String) -> Binding<String> {
        Binding(
            get: { index < names.count ? names[index] : fallback },
            set: { if index < names.count { names[index] = $0 } })
    }

    private func noteBinding(at index: Int, fallback: String) -> Binding<String> {
        Binding(
            get: { index < notes.count ? notes[index] : fallback },
            set: { if index < notes.count { notes[index] = $0 } })
    }

    private func selectableRow(
        _ label: String, selected: Bool, choose: @escaping () -> Void
    ) -> some View {
        Button(action: choose) {
            HStack {
                Text(verbatim: label).foregroundStyle(.primary)
                Spacer()
                if selected { Image(systemName: "checkmark").foregroundStyle(KoruColor.accent) }
            }
        }
    }
}
