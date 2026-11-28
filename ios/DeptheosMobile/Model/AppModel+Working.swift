import Foundation
import SwiftUI
@preconcurrency import DeptheosEngine

private let infoLog = AppLog(category: "view")

/// The working version: every stack keeps the list and settings being
/// edited, which Frames and Stack Settings always show. A render is shown,
/// never opened; Revert is the one way its frames and settings come back,
/// and Stack the one way a render is made.
extension AppModel {
    /// The working version's base, while it is in the history.
    var baseRender: RenderInfo? {
        guard let base else { return nil }
        return history.renders.first { $0.id == base }
    }

    /// While it is in the history.
    var shownRender: RenderInfo? {
        guard let shown = result?.renderID else { return nil }
        return history.renders.first { $0.id == shown }
    }

    /// How the working version stands against the render on screen and
    /// each render of its stack, in the engine's words: derived, never
    /// state. Asked on the pool only when its key changes (`showingsMemo`),
    /// since views ask several times a redraw and each sends the whole
    /// list; the last answer stands in meanwhile.
    var showings: ShowingsInfo {
        guard let engine else { return .nothing }
        let key = ShowingsKey(working: working, shown: result?.renderID, history: history.revision)
        let renders = { [history, stackID] in history.renders.filter { $0.stack == stackID }.map(\.id) }
        let answer = showingsMemo.answer(for: key) { key in
            let renders = renders()
            do {
                return try await Request().run {
                    try await engine.showings(
                        request: $0, working: key.working, shown: key.shown, renders: renders)
                }
            } catch {
                Self.unanswered("showings", error)
                return nil
            }
        }
        return answer ?? .nothing
    }

    var showing: ShowingInfo { showings.shown }

    /// Against render `id`: what Revert to it would do; with the base,
    /// whether the working version is edited.
    func showing(of id: String?) -> ShowingInfo {
        guard let id else { return .nothing }
        return showings.renders[id] ?? .nothing
    }

    /// "• Edited".
    var isEdited: Bool { base != nil && showing(of: base).edited }

    /// After the render shown's name and on the base's tile.
    nonisolated static let editedLabel = LocalizedStringResource(
        "Edited",
        comment: "Under the stack's name after the number of the render shown, on the base render's tile, and on the stack's Library tile: the stack's working version has changed since its base render.")

    /// On a render-less stack's tile and under its name.
    nonisolated static let notStackedLabel = LocalizedStringResource(
        "Not stacked yet",
        comment: "On a Library tile of a stack with frames and no render yet, and under the stack's name while it has none.")

    /// Its name, else its first frame's folder.
    var stackName: String? {
        if let render = history.renders.last(where: { $0.stack == stackID }) {
            return render.shownStackName
        }
        guard let first = working.list.files.first else { return nil }
        return ((first as NSString).deletingLastPathComponent as NSString).lastPathComponent
    }

    /// Only a stack with a render has a name to change.
    var canRenameStack: Bool {
        history.renders.contains { $0.stack == stackID }
    }

    /// Newest first: the Renders row and the title's menu.
    var stackRenders: [RenderInfo] {
        history.renders.filter { $0.stack == stackID }.reversed()
    }

    // MARK: Changing it

    /// A blank name goes back to its folder's.
    func renameStack(_ stack: String? = nil, to name: String) async {
        let (stack, trimmed) = (stack ?? stackID, name.trimmingCharacters(in: .whitespaces))
        guard await ask({ engine, _ in try await engine.renameStack(stack: stack, name: trimmed.isEmpty ? nil : trimmed) })
            != nil
        else { return }
        await history.refresh()
    }

    /// Only These.
    func includeOnly(_ files: Set<String>) {
        guard !held, !files.isEmpty else { return }
        let others = rows.map(\.file).filter { !files.contains($0) }
        var changed = listSetIncluded(list: list, files: others, included: false)
        changed = listSetIncluded(list: changed, files: Array(files), included: true)
        setList(changed)
    }

    func setIncluded(_ included: Bool, files: [String]) {
        guard !held, !files.isEmpty else { return }
        setList(listSetIncluded(list: list, files: files, included: included))
    }

    func toggleIncluded(_ row: ListRow) {
        setIncluded(!row.included, files: [row.file])
    }

    func setAllIncluded(_ included: Bool) {
        setIncluded(included, files: rows.map(\.file))
    }

    func remove(_ files: [String]) {
        guard !held else { return }
        addedToExisting = nil
        let left = listRemove(list: list, files: files)
        guard !left.files.isEmpty else { return clearList() }
        setList(left)
    }

    /// Remove All Frames, in the same stack; frames being read in drop.
    /// With a base, its result stays and Revert brings the frames back.
    func clearList() {
        guard !held else { return }
        addedToExisting = nil
        if adding.isReading { setAdding(.none) }
        viewGeneration += 1
        setList(listClear(list: list))
        setPreviewLoad(.none)
        if baseRender == nil {
            setResult(nil)
            showPlainFrame()
        } else if result != nil {
            backToResult()
        }
    }

    func edit(_ working: WorkingInfo) {
        (stackID, base) = (working.stack, working.base)
        // Settings first: the list's save writes them too.
        if let settings = working.settings { stackSettings.use(settings) }
        setList(working.list)
    }

    /// Revert to render `id`: its frames and settings become the working
    /// version, based on it, and it is shown. The caller asks first when
    /// `RevertInfo.alert` says so. Only while the engine still offers it (a
    /// confirmation can outlast its question) and the request is current;
    /// never while retouching.
    func revert(to id: String) async {
        guard !busy, viewing, canRevert(to: id) else { return }
        let generation = beginOpening(.revert(id))
        defer { endOpening(generation) }
        guard let working = await ask({ try await $0.workingOfRender(request: $1, id: id) }),
            viewGeneration == generation, viewing, canRevert(to: id)
        else { return }
        edit(working)
        if result?.renderID != id { await showResult(of: id, continuing: generation) }
    }

    private func canRevert(to id: String) -> Bool {
        showing(of: id).revert?.render == id
    }

    /// Kept; Edited and the change line follow, being derived.
    func settingsChanged() {
        saveList()
    }
}

extension ShowingInfo {
    /// Before the engine starts.
    static let nothing = ShowingInfo(
        kind: .nothing, shown: nil, base: nil, edited: false, changeLine: nil, baseTileLine: nil,
        revert: nil, log: "nothing")
}

extension ShowingsInfo {
    /// Before the engine starts.
    static let nothing = ShowingsInfo(shown: .nothing, renders: [:])
}

/// The key of `AppModel.showingsMemo`: the working version by value, the
/// render shown, and the history's revision.
nonisolated struct ShowingsKey: Equatable {
    let working: WorkingInfo
    let shown: String?
    let history: Int
}

extension AppModel {
    /// "62 of 66 frames": what Stack stacks, under the inspector's Stack
    /// and in the Stack Settings sheet.
    var stackedFrames: LocalizedStringResource {
        let included = rows.filter(\.included).count
        return rows.count == 1 && included == 1 ? framesCount(1) : framesIncluded(included, of: rows.count)
    }
}

/// "46 frames".
nonisolated func framesCount(_ count: Int) -> LocalizedStringResource {
    LocalizedStringResource(
        "\(count) frames",
        comment: "A count of frames: under a stack's name, on a Library tile, in the camera's status, under a folder in the Add Frames sheet. The placeholder is how many; zero reads 'No frames'.")
}

/// "44 of 46 frames".
nonisolated func framesIncluded(_ included: Int, of count: Int) -> LocalizedStringResource {
    LocalizedStringResource(
        "\(included) of \(count) frames",
        comment: "How many frames are included, of how many in the list: under a stack's name, under the Stack button, the Stack Settings sheet's subtitle, on a Library tile. The placeholders are the two counts.")
}

extension AppModel {
    /// Shown from `here` while asked for and its render is in the history;
    /// closing it there closes it.
    func showsRenderInfo(_ here: Bool) -> Binding<Bool> {
        Binding(
            get: { [weak self] in here && self?.renderInfoRender != nil },
            set: { [weak self] shown in
                if !shown, let self, let id = self.renderInfo { self.dismiss(.renderInfo(id)) }
            })
    }

    /// While it is in the history.
    var renderInfoRender: RenderInfo? {
        guard let id = renderInfo else { return nil }
        return history.renders.first { $0.id == id }
    }

    /// Render `id`'s record against the working version, the engine's rows;
    /// none, logged, when unreadable. Asked on the pool only when its key
    /// changes (`renderSetupMemo`); none until it answers.
    func renderSetup(_ id: String) -> [SetupSectionInfo] {
        guard let engine else { return [] }
        let key = RenderSetupKey(id: id, working: working, history: history.revision)
        let answer = renderSetupMemo.answer(for: key) { key in
            do {
                return try await Request().run {
                    try await engine.renderSetup(request: $0, id: key.id, working: key.working)
                }
            } catch EngineError.Canceled {
                return nil
            } catch {
                infoLog.error("render info: \(Self.message(error).string, privacy: .public)")
                return []
            }
        }
        return answer ?? []
    }
}

/// The key of `AppModel.renderSetupMemo`: render `id`, the working
/// version, and the history's revision (a record changes only by a relink,
/// which rereads the history). A failure is kept as no sections.
nonisolated struct RenderSetupKey: Equatable {
    let id: String
    let working: WorkingInfo
    let history: Int
}
