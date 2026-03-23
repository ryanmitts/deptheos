import SwiftUI
import DeptheosKit

struct ProjectListView: View {
    @Bindable var appState: AppState
    @State private var pendingDeletion: PendingDeletion?

    private enum PendingDeletion: Identifiable {
        case single(ProjectEntry)
        case all

        var id: String {
            switch self {
            case .single(let entry): "single-\(entry.id)"
            case .all: "all"
            }
        }
    }

    var body: some View {
        List(selection: $appState.sidebarSelection) {
            Label("New Stack", systemImage: "plus.square")
                .tag(SidebarSelection.newStack)

            Section("Projects") {
                if appState.projectStore.entries.isEmpty {
                    Text("No saved projects")
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .center)
                        .padding(.vertical, 20)
                } else {
                    ForEach(appState.projectStore.entries) { entry in
                        ProjectEntryRow(entry: entry)
                            .tag(SidebarSelection.project(entry.id))
                            .contextMenu {
                                Button("Delete", role: .destructive) {
                                    pendingDeletion = .single(entry)
                                }
                                Divider()
                                Button("Delete All Projects", role: .destructive) {
                                    pendingDeletion = .all
                                }
                            }
                    }
                    .onDelete { offsets in
                        let toDelete = offsets.map { appState.projectStore.entries[$0] }
                        for entry in toDelete {
                            if appState.selectedProjectID == entry.id {
                                appState.sidebarSelection = .newStack
                            }
                            appState.projectStore.deleteProject(entry)
                        }
                    }
                }
            }
        }
        .listStyle(.sidebar)
        .confirmationDialog(
            deletionTitle,
            isPresented: Binding(
                get: { pendingDeletion != nil },
                set: { if !$0 { pendingDeletion = nil } }
            ),
            titleVisibility: .visible
        ) {
            Button("Delete", role: .destructive) {
                guard let deletion = pendingDeletion else { return }
                switch deletion {
                case .single(let entry):
                    if appState.selectedProjectID == entry.id {
                        appState.sidebarSelection = .newStack
                    }
                    appState.projectStore.deleteProject(entry)
                case .all:
                    appState.sidebarSelection = .newStack
                    appState.projectStore.deleteAllProjects()
                }
            }
        }
        .onChange(of: appState.sidebarSelection) { _, newValue in
            switch newValue {
            case .newStack:
                appState.resetWorkspace()
            case .project(let url):
                if let entry = appState.projectStore.entries.first(where: { $0.id == url }) {
                    appState.loadProjectFromStore(entry)
                }
            default:
                break
            }
        }
        .task {
            await appState.projectStore.scanProjects()
        }
    }

    private var deletionTitle: String {
        switch pendingDeletion {
        case .single(let entry):
            "Delete \"\(entry.name)\"?"
        case .all:
            "Delete all projects?"
        case nil:
            ""
        }
    }
}

struct ProjectEntryRow: View {
    let entry: ProjectEntry

    var body: some View {
        HStack(spacing: 8) {
            if let thumb = entry.thumbnail {
                Image(decorative: thumb, scale: 1.0)
                    .resizable()
                    .aspectRatio(contentMode: .fill)
                    .frame(width: 48, height: 48)
                    .clipShape(RoundedRectangle(cornerRadius: 4))
            } else {
                RoundedRectangle(cornerRadius: 4)
                    .fill(.quaternary)
                    .frame(width: 48, height: 48)
                    .overlay(
                        Image(systemName: "photo.on.rectangle")
                            .foregroundStyle(.secondary)
                    )
            }

            VStack(alignment: .leading, spacing: 2) {
                Text("\(entry.config.algorithmName) · \(entry.config.settingsSummary)")
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .font(.body)

                Text("\(entry.imageCount) images")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 2)
    }
}
