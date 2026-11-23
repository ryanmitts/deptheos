import SwiftUI
@preconcurrency import DeptheosEngine

/// Render Info for `AppModel.renderInfo`: a sheet with Close at regular
/// width; at compact width pushed onto the sources' renders
/// (`SourcesSidebar`).
struct RenderInfoSheet: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if let render = model.renderInfoRender {
            NavigationStack {
                RenderInfoView(render: render)
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button(role: .close) { model.dismiss(.renderInfo(render.id)) }
                        }
                    }
            }
            .singleItemSheetBar()
            .presentationDetents([.medium, .large])
        }
    }
}

/// A render's record, read-only: thumbnail, stacked date, star and the
/// change line, then the engine's sections. A row the working version has
/// otherwise carries the edited dot and its value under it. Nothing here
/// changes anything: Revert is beside Stack.
struct RenderInfoView: View {
    static let title = LocalizedStringResource(
        "Render Info",
        comment: "Title menu, a render's menu in the Renders list and Stack menu item (⌘I): shows a render's frames and settings as stacked, read-only.")

    @Environment(AppModel.self) private var model
    let render: RenderInfo

    var body: some View {
        List {
            Section {
                head
            }
            ForEach(model.renderSetup(render.id), id: \.part) { section in
                Section {
                    ForEach(Array(section.rows.enumerated()), id: \.offset) { _, row in
                        SetupRowView(row: row)
                    }
                } header: {
                    // The engine's words.
                    Text(verbatim: section.title)
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle(Text(title))
        .navigationBarTitleDisplayMode(.inline)
        .showsThumbnail(of: render.id)
    }

    /// "#3", or "#3 · sharper" for a render with a name.
    private var title: UserText {
        render.nameLabel()
    }

    /// The thumbnail, "Stacked Nov 14, 2026 at 10:42" with its star, and the
    /// change line.
    private var head: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let image = model.history.thumbnails[render.id] {
                Image(decorative: image, scale: 1)
                    .resizable()
                    .scaledToFit()
                    .frame(maxWidth: .infinity, maxHeight: 180)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
            }
            HStack(spacing: 6) {
                // The engine's words, with the date in the system's style.
                Text(verbatim: AppWords.words.stackedAtLine(date: stacked))
                if render.picked {
                    Image(systemName: "star.fill")
                        .foregroundStyle(.yellow)
                        .accessibilityLabel(LocalizedStringResource(
                            "Starred", comment: "VoiceOver, in Render Info: the render is starred."))
                }
            }
            .font(.subheadline)
            if let line = model.showing(of: render.id).changeLine {
                // The engine's words.
                Text(verbatim: line)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.vertical, 4)
    }

    /// In the system's style.
    private var stacked: String {
        Date(timeIntervalSince1970: TimeInterval(render.createdMs) / 1000)
            .formatted(date: .abbreviated, time: .shortened)
    }
}

/// "Smoothness  4", with the edited dot and "Working version: 6" under it
/// when it differs; VoiceOver reads it as one.
private struct SetupRowView: View {
    let row: SetupRowInfo

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                Text(verbatim: row.label)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if row.differs {
                    EditedDot().accessibilityHidden(true)
                }
                Spacer(minLength: 8)
                Text(verbatim: row.value)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.trailing)
            }
            if let working = row.working {
                Text(verbatim: working)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(verbatim: spoken))
    }

    /// In order, the empty ones left out.
    private var spoken: String {
        [row.label, row.value, row.working ?? ""].filter { !$0.isEmpty }.joined(separator: ", ")
    }
}
