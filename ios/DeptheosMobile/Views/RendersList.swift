import SwiftUI
@preconcurrency import DeptheosEngine

/// The stack's renders, newest first: thumbnail, "#4 ★ · sharper",
/// "Depth · R8 S5 B3", and for an edited base the engine's count ("2
/// changes") and Edited. A tap shows that render, nothing else; the one
/// shown has the ring, and one on its way shows progress (a tap on another
/// meanwhile shows that one).
struct RendersList: View {
    static let title = LocalizedStringResource(
        "Renders", comment: "The side of the Frames | Renders switch at the head of the stack's sources that lists its renders.")

    @Environment(AppModel.self) private var model

    var body: some View {
        // Asked once for the list: the base's row says it whatever is shown.
        let base = model.showing(of: model.base)
        List {
            ForEach(model.stackRenders, id: \.id) { render in
                RenderRow(
                    render: render,
                    edits: render.id == model.base && base.edited ? base.baseTileLine ?? "" : nil)
            }
        }
        .listStyle(.insetGrouped)
        .disabled(model.held)
        // What was just deleted, with Undo, where the delete was asked.
        .safeAreaBar(edge: .bottom) {
            DeletedNoticeBar()
                .animation(.snappy, value: model.deleteUndo)
        }
        .overlay {
            if model.stackRenders.isEmpty {
                ContentUnavailableView {
                    Label(
                        LocalizedStringResource("No Renders", comment: "The Renders list's empty-state title: the stack was never stacked."),
                        systemImage: RenderInfo.symbol)
                } description: {
                    Text(
                        "Each Stack makes a render, kept here with its frames and settings.",
                        comment: "The Renders list's empty-state text.")
                }
            }
        }
    }
}

/// With its menu: Render Info, Compare with Result, Revert to #n, the
/// star, and deletes, at once into Recently Deleted with Undo (ADR-140).
private struct RenderRow: View {
    @Environment(AppModel.self) private var model
    let render: RenderInfo
    /// The edited base's count ("2 changes") and "• Edited"; `nil` for any
    /// other row.
    let edits: String?
    /// Asking to be confirmed.
    @State private var asking: RevertInfo?
    @State private var renaming = false
    @State private var draftName = ""

    var body: some View {
        let shown = model.result?.renderID == render.id
        Button {
            // On iPhone the list is in the Frames sheet: the photo shows
            // above it.
            model.revealViewer()
            Task { await model.open(render) }
        } label: {
            RenderCard(
                render: render, selected: shown,
                loading: model.opening?.target.render == render.id,
                badge: edits != nil ? AppModel.editedLabel : nil,
                // The engine's words.
                lines: [AppWords.words.algorithmSummary(algorithm: render.settings.algorithm)]
                    + (edits.map { [$0] } ?? []))
        }
        .buttonStyle(.plain)
        // The engine's words: "Render 4, Pyramid, starred".
        .accessibilityLabel(Text(verbatim: render.spokenLabel))
        .accessibilityValue(edits != nil ? AppModel.editedLabel : .empty)
        .accessibilityAddTraits(shown ? .isSelected : [])
        // As the desktop's: what to look at and Revert, the star, then
        // what deletes.
        .contextMenu {
            Button(RenderInfoView.title, systemImage: "info.circle") {
                model.present(.renderInfo(render.id))
            }
            if model.result?.display != nil, !shown {
                Button(
                    LocalizedStringResource(
                        "Compare with Result",
                        comment: "Render menu item, in the Renders list: lays the render out with the render shown, the result."),
                    systemImage: "square.split.2x1"
                ) {
                    model.revealViewer()
                    model.compare(withRender: render.id)
                }
                .disabled(model.busy)
            }
            // As the change line's Revert would, were this render shown.
            let revert = model.showing(of: render.id).revert
            Button(ChangeLine.revertTitle(render.number), systemImage: "arrow.counterclockwise") {
                guard let revert else { return }
                if revert.alert == nil {
                    Task { await model.revert(to: revert.render) }
                } else {
                    asking = revert
                }
            }
            .disabled(revert == nil || model.busy)
            Divider()
            Button(
                LocalizedStringResource("Rename Render…", comment: "Render menu item: asks for a custom name for this render."),
                systemImage: "pencil"
            ) {
                draftName = render.name ?? ""
                renaming = true
            }
            Button(
                render.picked
                    ? LocalizedStringResource("Unstar", comment: "Render menu item: takes the star off the render.")
                    : LocalizedStringResource("Star", comment: "Render menu item: marks the render to keep."),
                systemImage: render.picked ? "star.slash" : "star"
            ) {
                Task { await model.history.togglePicked(render) }
            }
            Divider()
            Button(
                LocalizedStringResource("Delete Render", comment: "Render menu item: moves the render to Recently Deleted."),
                systemImage: "trash", role: .destructive
            ) {
                let id = render.id
                Task { await model.deleteRenders([id]) }
            }
            .disabled(model.busy)
            // The cleanup once this is the result wanted; starred ones stay.
            Button(otherRendersItem(render), systemImage: "trash", role: .destructive) {
                let others = model.history.others(of: render)
                Task { await model.deleteRenders(others) }
            }
            .disabled(model.busy || model.history.others(of: render).isEmpty)
        }
        .revertDialog($asking)
        .alert(
            Text("Rename Render", comment: "Title of the alert that renames a render."),
            isPresented: $renaming
        ) {
            TextField(
                LocalizedStringResource("Name", comment: "Rename Render alert: placeholder of the name field."),
                text: $draftName)
            Button(LocalizedStringResource("Rename", comment: "Rename Render alert: gives the render the name typed.")) {
                let updated = draftName
                Task { await model.history.rename(render, to: updated) }
            }
            Button(LocalizedStringResource("Cancel", comment: "Rename Render alert: closes it without renaming."), role: .cancel) {}
        } message: {
            Text(
                "Give this render a short label to tell it apart in Renders and Search. Leave empty to clear.",
                comment: "Rename Render alert's text.")
        }
    }

    /// "Delete 3 Other Renders": the stack's unstarred renders but `render`.
    private func otherRendersItem(_ render: RenderInfo) -> LocalizedStringResource {
        LocalizedStringResource(
            "Delete \(model.history.others(of: render).count) Other Renders",
            comment: "Menu item on a render in the Renders list: moves the stack's other unstarred renders to Recently Deleted. The placeholder is how many; zero reads 'Delete Other Renders'.")
    }
}

/// Its thumbnail (ringed when `selected`, with `badge`, progress while
/// `loading`), "#4 ★ · sharper" over `lines`, and `trailing`.
struct RenderCard<Trailing: View>: View {
    @Environment(AppModel.self) private var model
    let render: RenderInfo
    let selected: Bool
    var loading = false
    var badge: LocalizedStringResource?
    let lines: [String]
    @ViewBuilder var trailing: () -> Trailing

    var body: some View {
        HStack(spacing: 12) {
            TileImage(
                image: model.history.thumbnails[render.id], selected: selected,
                loading: loading, badge: badge)
            VStack(alignment: .leading, spacing: 2) {
                TileTitle(render: render)
                ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                    Text(verbatim: line)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 8)
            trailing()
        }
        .padding(.vertical, 2)
        .contentShape(.rect)
        .showsThumbnail(of: render.id)
    }
}

extension RenderCard where Trailing == EmptyView {
    init(render: RenderInfo, selected: Bool, loading: Bool = false, badge: LocalizedStringResource? = nil, lines: [String]) {
        self.init(render: render, selected: selected, loading: loading, badge: badge, lines: lines) { EmptyView() }
    }
}

/// The render glyph, then the engine's "#4 ★ · sharper", the star drawn as
/// `star.fill`.
private struct TileTitle: View {
    let render: RenderInfo

    var body: some View {
        let parts = render.nameLabel(starred: true).string
            .split(separator: "★", maxSplits: 1, omittingEmptySubsequences: false)
            .map { $0.trimmingCharacters(in: .whitespaces) }
        HStack(spacing: 3) {
            Image(systemName: RenderInfo.symbol).font(.caption)
            Text(verbatim: parts[0])
            if parts.count > 1 {
                Image(systemName: "star.fill").font(.caption).foregroundStyle(.yellow)
                if !parts[1].isEmpty {
                    Text(verbatim: parts[1]).lineLimit(1)
                }
            }
        }
        .font(.callout.weight(.medium))
        .monospacedDigit()
        .lineLimit(1)
    }
}

/// Its picture, ring when selected, a badge, and progress while on its way
/// to the viewer.
private struct TileImage: View {
    let image: CGImage?
    let selected: Bool
    var loading = false
    var badge: LocalizedStringResource?

    var body: some View {
        ZStack(alignment: .bottomLeading) {
            Group {
                if let image {
                    Image(decorative: image, scale: 1).resizable().scaledToFill()
                } else {
                    Rectangle().fill(.white.opacity(0.08))
                }
            }
            .frame(width: 96, height: 54)
            .clipShape(RoundedRectangle(cornerRadius: 6))
            if let badge {
                HStack(spacing: 4) {
                    EditedDot().accessibilityHidden(true)
                    Text(badge)
                }
                .font(.caption2.weight(.semibold))
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .hudPill()
                .padding(4)
            }
        }
        .overlay {
            if loading {
                ProgressView()
                    .padding(6)
                    .hudPill()
            }
        }
        .overlay(
            RoundedRectangle(cornerRadius: 7)
                .strokeBorder(selected ? Color.accentColor : .clear, lineWidth: 2)
                .padding(-2))
    }
}
