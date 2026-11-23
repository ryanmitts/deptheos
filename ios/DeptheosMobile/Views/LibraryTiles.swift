import SwiftUI
import UniformTypeIdentifiers
@preconcurrency import DeptheosEngine

/// A stack: its newest (starred, if any) render's thumbnail, name and
/// render count. Tap opens it at its working version; it drags out as JPEG.
struct StackTile: View {
    @Environment(AppModel.self) private var model
    @Environment(\.libraryLocked) private var locked
    let tile: LibraryStack
    let cover: RenderInfo

    var body: some View {
        let open = model.stackID == tile.stack
        Button {
            Task { await model.openStack(tile.stack) }
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                Color.clear
                    .aspectRatio(4 / 3, contentMode: .fit)
                    .overlay {
                        if let image = model.history.thumbnails[cover.id] {
                            Image(decorative: image, scale: 1)
                                .resizable()
                                .scaledToFill()
                        } else {
                            Rectangle().fill(.quaternary)
                        }
                    }
                    .clipShape(RoundedRectangle(cornerRadius: 10))
                    .overlay(alignment: .bottomLeading) {
                        if open, model.isEdited { EditedBadge().padding(6) }
                    }
                    .overlay(alignment: .topTrailing) {
                        if let badge = tile.badge { CountBadge(text: badge).padding(6) }
                    }
                    .overlay {
                        if open {
                            RoundedRectangle(cornerRadius: 10)
                                .strokeBorder(Color.accentColor, lineWidth: 2)
                        }
                    }
                Text(verbatim: cover.shownStackName)
                    .font(.callout.weight(.medium))
                    .lineLimit(1)
                Text(caption)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .disabled(locked)
        .accessibilityLabel(accessibilityText)
        // The cover, in the engine's words: "Render 4, Pyramid, starred".
        .accessibilityValue(Text(verbatim: cover.spokenLabel))
        .accessibilityAddTraits(open ? .isSelected : [])
        .showsThumbnail(of: cover.id)
        .draggable(
            RenderFile(
                render: cover, model: model, session: model.paint.session,
                options: model.preferences.export))
    }

    /// "Depth · 24 of 30 frames" for an only render; "#4 · Depth" or "#4 ★ ·
    /// Depth" of several: the star goes on the cover's number, never the
    /// stack's name.
    private var caption: UserText {
        // The engine's name for the method.
        let method = UserText.verbatim(AppWords.words.algorithmSummary(algorithm: cover.settings.algorithm))
        if tile.total > 1 || cover.picked {
            let number = AppWords.words.renderName(number: cover.number, name: nil, picked: cover.picked)
            return .line([.verbatim(number), method])
        }
        let stacked = cover.included.filter { $0 }.count
        let total = cover.frames.count
        return .line([method, .app(stacked == total ? framesCount(total) : framesIncluded(stacked, of: total))])
    }

    private var accessibilityText: LocalizedStringResource {
        LocalizedStringResource(
            "\(cover.shownStackName), \(tile.total) renders",
            comment: "VoiceOver label of a Library tile. The placeholders are the stack's name and how many renders it has.")
    }
}

extension View {
    /// The thumbnail policy hears while it is in view
    /// (`HistoryModel.thumbnailAppeared`).
    func showsThumbnail(of id: String) -> some View {
        modifier(ShowsThumbnail(id: id))
    }
}

private struct ShowsThumbnail: ViewModifier {
    @Environment(AppModel.self) private var model
    let id: String

    func body(content: Content) -> some View {
        content
            .onAppear { model.history.thumbnailAppeared(id) }
            .onDisappear { model.history.thumbnailDisappeared(id) }
            .onChange(of: id) { old, new in
                model.history.thumbnailDisappeared(old)
                model.history.thumbnailAppeared(new)
            }
    }
}

/// "4", or "2 of 4" when a filter or search lets two through (the engine's
/// words).
private struct CountBadge: View {
    let text: String

    var body: some View {
        HStack(spacing: 3) {
            Image(systemName: RenderInfo.symbol)
            Text(verbatim: text).monospacedDigit()
        }
        .font(.caption2.weight(.semibold))
        .padding(.horizontal, 7)
        .padding(.vertical, 3)
        .hudPill()
        .accessibilityHidden(true)
    }
}

/// "• Edited": the working version changed since its last render.
private struct EditedBadge: View {
    var body: some View {
        HStack(spacing: 4) {
            EditedDot().accessibilityHidden(true)
            Text(AppModel.editedLabel)
        }
        .font(.caption2.weight(.semibold))
        .padding(.horizontal, 7)
        .padding(.vertical, 3)
        .hudPill()
    }
}

/// A failed batch stack. Tap loads its frames and settings to retry.
struct FailureTile: View {
    @Environment(AppModel.self) private var model
    @Environment(\.libraryLocked) private var locked
    let failure: FailureInfo

    var body: some View {
        Button {
            model.setScreen(.stacking)
            Task { await model.retry(failure) }
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                RoundedRectangle(cornerRadius: 10)
                    .fill(.quaternary)
                    .aspectRatio(4 / 3, contentMode: .fit)
                    .overlay {
                        VStack(spacing: 4) {
                            Text("Failed", comment: "On a Library tile: the batch stack didn't finish.").font(.callout).foregroundStyle(.red)
                            // The engine's words.
                            Text(verbatim: failure.error)
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                                .lineLimit(2)
                                .multilineTextAlignment(.center)
                        }
                        .padding(8)
                    }
                Text(verbatim: failure.label).font(.callout.weight(.medium)).lineLimit(1)
                Text(
                    "Tap to open and retry", comment: "Under a failed Library tile: a tap opens its frames and settings to stack again."
                ).font(.caption).foregroundStyle(.secondary)
            }
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .disabled(locked)
        .accessibilityHint(LocalizedStringResource(
            "Opens its frames and settings in Stacking to try again.", comment: "VoiceOver hint of a failed Library tile."))
    }
}

/// Frames kept and no render yet: its name and frame count.
struct UnstackedTile: View {
    @Environment(AppModel.self) private var model
    @Environment(\.libraryLocked) private var locked
    let stack: UnstackedInfo

    var body: some View {
        let open = model.stackID == stack.stack
        Button {
            Task { await model.openStack(stack.stack) }
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                RoundedRectangle(cornerRadius: 10)
                    .fill(.quaternary)
                    .aspectRatio(4 / 3, contentMode: .fit)
                    .overlay {
                        VStack(spacing: 4) {
                            Image(systemName: "square.3.layers.3d")
                            Text(AppModel.notStackedLabel).font(.callout)
                        }
                        .foregroundStyle(.secondary)
                        .padding(8)
                    }
                    .overlay {
                        if open {
                            RoundedRectangle(cornerRadius: 10)
                                .strokeBorder(Color.accentColor, lineWidth: 2)
                        }
                    }
                Text(verbatim: stack.shownName).font(.callout.weight(.medium)).lineLimit(1)
                Text(framesCount(Int(stack.frames)))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .disabled(locked)
        .accessibilityLabel(LocalizedStringResource(
            "\(stack.shownName), not stacked yet",
            comment: "VoiceOver label of a Library tile of a stack with no render yet. The placeholder is the stack's name."))
        .accessibilityHint(LocalizedStringResource(
            "Opens its frames and settings in Stacking", comment: "VoiceOver hint of a Library tile of a stack with no render yet."))
        .accessibilityAddTraits(open ? .isSelected : [])
    }
}

/// Written as a JPEG with its paint when dropped, with the export
/// preferences' quality and name, its frames' scopes held meanwhile (their
/// metadata goes in).
struct RenderFile: Transferable {
    let render: RenderInfo
    let model: AppModel
    let session: PaintSession?
    let options: ExportPreferences

    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(exportedContentType: .jpeg) { file in
            let (lease, engine) = await (file.model.access(file.render.frames), file.model.engine)
            defer { withExtendedLifetime(lease) {} }
            return SentTransferredFile(try await file.write(engine))
        }
    }

    private func write(_ engine: Engine?) async throws -> URL {
        guard let engine else { throw CocoaError(.fileWriteUnknown) }
        let directory = try AppFiles.newTemporaryFolder(prefix: "Drag")
        let id = render.id
        let result = try await Request().run { try await engine.loadRenderPixels(request: $0, id: id) }
        let name = AppModel.exportName(result, render: render, pattern: options.fileNamePattern)
        let path = try await model.runExport(name: name) { request in
            try await PaintModel.export(
                session, request: request, result: result, renderID: render.id,
                directory: directory.path, name: name, format: .jpeg, options: options)
        }
        return URL(fileURLWithPath: path)
    }
}
