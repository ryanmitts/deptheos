import Foundation
import PhotosUI
import SwiftUI
@preconcurrency import DeptheosEngine

private let log = AppLog(category: "files")

/// What was picked in Files and what adding it does. An image picked by
/// itself is copied in (only a folder's access can be kept without a scope
/// per file) unless a folder Deptheos reads holds it; a folder is used in
/// place or copied, as `copyFolders` says.
struct AddPlan: Sendable {
    struct Item: Identifiable, Sendable {
        let url: URL
        /// Access is kept through this (`keep`), never through `url`,
        /// which has no scope.
        let picked: Picked
        let isFolder: Bool
        let source: FileSource
        /// The folder's images, or the image itself.
        let images: [URL]
        /// As far as the system says before they are downloaded.
        let bytes: Int64
        /// Inside a folder Deptheos reads already (`AppModel.readable`):
        /// used in place, as that folder's other frames are.
        var covered = false

        var id: URL { url }
        var name: String { url.lastPathComponent }
    }

    var items: [Item]
    var copyFolders: Bool
    let free: Int64?

    /// Above this, copying single images from off the device is said first.
    static let largeCopy: Int64 = 1 << 30
    /// From this many single images in one folder, their copy is said
    /// first, offering to use them in place: a stack's worth, where the
    /// folder would have done.
    static let manyLoose = 10

    /// Single images no folder Deptheos reads holds: always copied.
    var loose: [Item] { items.filter { !$0.isFolder && $0.source != .owned && !$0.covered } }
    /// Single images inside a folder Deptheos reads: added in place.
    var covered: [Item] { items.filter { !$0.isFolder && $0.source != .owned && $0.covered } }
    var folders: [Item] { items.filter { $0.isFolder && $0.source != .owned } }
    /// The app's own: added in place.
    var owned: [Item] { items.filter { $0.source == .owned } }
    var copied: [Item] { loose + (copyFolders ? folders : []) }
    var inPlace: [Item] { copyFolders ? [] : folders }

    mutating func cover(_ reads: (String) -> Bool) {
        for index in items.indices where !items[index].isFolder && items[index].source != .owned {
            items[index].covered = reads(items[index].url.path)
        }
    }

    var copyBytes: Int64 { copied.reduce(0) { $0 + $1.bytes } }

    /// Where the picked folders are, when off the device.
    var place: Place? { folders.lazy.compactMap(\.source.place).first }

    /// The one folder the images to copy are all in, if so: picked itself
    /// (`AppModel.useFolder`), they are used in place. Only the images
    /// picked can be read, so what else is in it is unknown.
    var looseFolder: URL? {
        let folders = Set(loose.map { $0.url.deletingLastPathComponent().standardizedFileURL })
        return folders.count == 1 ? folders.first : nil
    }

    /// Something to choose or be told first: a folder off the device, many
    /// single images from one folder, or a large copy from off the device.
    var asks: Bool {
        if folders.contains(where: { $0.source != .device }) { return true }
        if looseFolder != nil, loose.count >= Self.manyLoose { return true }
        let away = loose.filter { $0.source != .device }.reduce(0) { $0 + $1.bytes }
        return away > Self.largeCopy
    }

    /// How many files fetching copies or downloads, as far as known before
    /// the folders kept in place are listed: its progress's start.
    var fetchCount: Int {
        let downloaded = (inPlace + covered).filter(\.source.downloads)
        return (copied + downloaded).reduce(0) { $0 + $1.images.count }
    }

    /// The copy needs more space than the device has.
    var short: Bool { free.map { copyBytes > $0 } ?? false }
}

/// How far a copy or download has come.
struct FetchProgress: Sendable {
    var done: Int
    let total: Int
    /// Nothing is copied, only downloaded in place.
    let downloadOnly: Bool

    var label: LocalizedStringResource {
        let current = min(done + 1, total)
        return downloadOnly
            ? LocalizedStringResource(
                "Downloading \(current) of \(total)",
                comment: "Progress in the Add sheet: frames picked in Files are downloaded where they are. The placeholders are the file's number and how many.")
            : LocalizedStringResource(
                "Copying \(current) of \(total)",
                comment: "Progress in the Add sheet: frames picked in Files are copied into Deptheos. The placeholders are the file's number and how many.")
    }
}

/// Images and folders on their way into the list. The sheet is up only for
/// a choice, a long copy or download, or a failure (`showsSheet`), which is
/// said in it since an alert under a sheet never shows; planning and
/// reading are said in the Frames list (`inList`). What was picked stays
/// readable through its `PickedScopes`, carried from state to state.
///
/// Lifecycle: none, planning, asking (the plan), fetching, reading (its
/// request) or failed (why, with what came through); the app `waits`
/// while planning, asking, fetching or failed. Changed only by
/// `setAdding`: here, by `copyFramesIn` (`AppModel+Imports.swift`), and
/// by `AppModel.addPaths` and `clearList`.
///
///     none ──picked──▶ planning ──something to choose──▶ asking ──Add──▶ fetching ──done──▶ reading
///                         │                                 ├──Cancel──▶ none
///                         │                                 └──their folder picked (Use Them Where They Are…)──▶
///                         │                                      asking, or with nothing left to choose
///                         │                                      fetching
///                         ├──something to copy off the device or download──▶ fetching ──Cancel──▶ none
///                         └──all usable where it is, or copied on the device (clones, at once)──▶ reading
///     none ──picked in Photos──▶ fetching (copied into Imports) ──done──▶ reading
///     none ──added (a burst's folder, a file opened)──▶ reading
///     reading ──more added──▶ reading (both paths, a new request)
///     reading ──read, or another stack on screen, Remove All Frames──▶ none
///     planning, asking, fetching ──it can't go on, or part of it failed──▶ failed
///     failed ──OK──▶ reading (what did come through), or none
enum Adding {
    case none
    /// Listing and sizing the picked items (by name), copying what is on
    /// the device.
    case planning([String], PickedScopes)
    /// The sheet shows the plan, waiting for Add.
    case asking(AddPlan, PickedScopes)
    /// Copying, or downloading in place.
    case fetching(FetchProgress, FetchCancel, PickedScopes)
    case reading(ReadRequest)
    /// Said in the sheet until OK; `then` is what came through, read into
    /// the list after.
    case failed(UserText, then: [String])

    /// Nothing else is picked in Files meanwhile.
    var isActive: Bool {
        if case .none = self { false } else { true }
    }

    /// The rest of the app waits: another stack on screen would take what
    /// is picked.
    var waits: Bool {
        switch self {
        case .none, .reading: false
        case .planning, .asking, .fetching, .failed: true
        }
    }

    var showsSheet: Bool {
        switch self {
        case .none, .planning, .reading: false
        case .asking, .fetching, .failed: true
        }
    }

    /// The names the Frames list shows on their way in, with no sheet up,
    /// leaving out the app's own folders (a copy from Photos).
    var inList: [String]? {
        switch self {
        case .planning(let names, _): names
        case .reading(let request):
            request.paths
                .filter { !Bookmarks.isOwned(URL(fileURLWithPath: $0)) }
                .map { ($0 as NSString).lastPathComponent }
        case .none, .asking, .fetching, .failed: nil
        }
    }

    /// Open while on its way; none once read, or for what wasn't picked
    /// in Files.
    var scopes: PickedScopes {
        switch self {
        case .planning(_, let scopes), .asking(_, let scopes), .fetching(_, _, let scopes): scopes
        case .none, .reading, .failed: .empty
        }
    }

    var isReading: Bool {
        if case .reading = self { true } else { false }
    }

    /// `request` is the read on its way.
    func isReading(_ request: ReadRequest) -> Bool {
        if case .reading(let current) = self { current.id == request.id } else { false }
    }

    var name: String {
        switch self {
        case .none: "none"
        case .planning(let names, _): "planning, \(names.count) picked"
        case .asking: "asking"
        case .fetching: "fetching"
        case .reading(let request): "reading #\(request.id.number), \(request.paths.count) paths"
        case .failed(_, let then): "failed, \(then.count) paths to read"
        }
    }
}

/// The security scopes of what was picked in Files, opened as picked and
/// closed when the last `Adding` state holding them is left, so nothing
/// stays open after. What stays in place has its bookmark kept by then
/// (`AppModel.keep`).
nonisolated final class PickedScopes: Sendable {
    static let empty = PickedScopes(open: [])

    /// The URLs whose scope opened.
    private let urls: [URL]

    private init(open urls: [URL]) {
        self.urls = urls
    }

    /// Those that open are closed with this.
    static func opening(_ picked: [URL]) -> PickedScopes {
        PickedScopes(open: picked.filter { $0.startAccessingSecurityScopedResource() })
    }

    deinit {
        for url in urls { url.stopAccessingSecurityScopedResource() }
    }
}

/// Frames just added to a stack that had frames: offers New Stack Instead
/// in case they are another subject.
struct AddedToExisting: Equatable {
    /// "Added 12 frames to canon1.", worded by the engine.
    let text: String
    let files: [String]
}

/// A read of frames into the list, numbered so an overtaken one drops.
struct ReadRequest {
    let id: Ticket
    /// The stack on screen when it began: it lands only there.
    let stack: String
    let paths: [String]
}

extension AppModel {
    /// Add the app's own images and folders (each adds the images directly
    /// inside it) in place; they need no bookmark. What is picked in Files
    /// goes through `addPicked`.
    func add(_ urls: [URL]) async {
        guard !held, !urls.isEmpty else { return }
        await addPaths(urls.map(\.standardizedFileURL.path))
    }

    /// Add the readable images and folders at `paths` as a read of its own
    /// (`Adding.reading`), neither dropped by nor dropping a render shown.
    /// One asked for during another takes over that one's paths, so nothing
    /// picked is lost; Remove All Frames drops it. The engine's add plan
    /// decides (ADR-144): with another stack on screen, nothing is added,
    /// and that is said, as is each folder that couldn't be read.
    func addPaths(_ paths: [String]) async {
        var paths = paths
        if case .reading(let pending) = adding, pending.stack == stackID {
            paths = pending.paths + paths
        }
        guard !paths.isEmpty else { return }
        let request = ReadRequest(id: reads.next(), stack: stackID, paths: paths)
        setAdding(.reading(request))
        // Their roots' scopes, held while they are read.
        let lease = await access(paths)
        let listed: Listed
        do {
            listed = try await Request().run { try await listExpand(request: $0, paths: paths, words: AppWords.words) }
        } catch {
            // Still this read's: the list takes frames again.
            if adding.isReading(request) { setAdding(.none) }
            report(error)
            return
        }
        withExtendedLifetime(lease) {}
        guard adding.isReading(request) else { return }
        setAdding(.none)
        // Planned against the list as it is now, keeping changes made
        // meanwhile (ADR-144).
        let plan = addPlan(
            files: listed.files, into: request.stack, onScreen: stackID,
            running: !running.isIdle, list: list, words: AppWords.words)
        let said: [String]
        switch plan {
        case .refused(let message), .nothing(let message): said = listed.unreadable + [message]
        case .add: said = listed.unreadable
        }
        if !said.isEmpty {
            errorMessage = ErrorMessage(.verbatim(said.joined(separator: "\n")))
        }
        guard case .add(let added, let offer) = plan else { return }
        let before = Set(rows.map(\.file))
        let existingStackName = stackName
        setList(added)
        let addedRows = rows.filter { !before.contains($0.file) }
        guard let first = addedRows.first else { return }
        if offer {
            let name = existingStackName ?? stackName ?? String(localized: Self.untitledStack)
            addedToExisting = AddedToExisting(
                text: AppWords.words.addedFramesText(frames: UInt32(addedRows.count), stack: name),
                files: addedRows.flatMap(\.files))
        } else {
            addedToExisting = nil
        }
        // The render shown stays behind the frame for the home control and
        // the change line.
        await select(first.file)
    }

    /// New Stack Instead: the frames just added move to a new stack.
    func moveAddedToNewStack() async {
        guard !held, let added = addedToExisting else { return }
        addedToExisting = nil
        let files = added.files
        guard !files.isEmpty else { return }
        setList(listRemove(list: list, files: files))
        let fresh = newWorking(files, settings: working.settings)
        await switchStack(to: fresh.stack, opening: .newStack(fresh.stack), fallback: fresh)
        if let first = rows.first {
            await select(first.file)
        }
    }

    func addPicked(_ picked: [Picked]) async {
        guard !busy, !adding.isActive, !picked.isEmpty else { return }
        // Readable while planned and copied; what stays in place has its
        // bookmark kept (`keep`).
        setAdding(.planning(picked.map(\.name), .opening(picked.map(\.url))))
        var plan = await Self.plan(picked)
        // An image inside a folder Deptheos reads stays there: its scope
        // is opened first.
        let singles = plan.items.filter { !$0.isFolder && $0.source != .owned }.map(\.url.path)
        if !singles.isEmpty {
            let read = await readable(singles)
            plan.cover { read.contains($0) }
            if !plan.covered.isEmpty {
                log.info("added from Files: \(plan.covered.count) of \(singles.count) single images are in folders read already")
            }
        }
        guard !plan.items.isEmpty else {
            failAdding(.app(LocalizedStringResource(
                "None of the files you picked are ones Deptheos reads.",
                comment: "Alert text after Add from Files: nothing picked is an image Deptheos reads.")))
            return
        }
        if plan.asks {
            setAdding(.asking(plan, adding.scopes))
        } else {
            await fetch(plan)
        }
    }

    /// Which of `files` are under a root whose scope opens. Held only while
    /// asked: later readers take their own lease.
    func readable(_ files: [String]) async -> Set<String> {
        let lease = await access(files)
        guard let roots = await ask({ try await $0.rootsOf(request: $1, paths: files) }) else { return [] }
        return Set(zip(files, roots).compactMap { file, root in
            root.map(\.root).map(lease.opened) == true ? file : nil
        })
    }

    func setCopyFolders(_ copy: Bool) {
        guard case .asking(var plan, let scopes) = adding else { return }
        plan.copyFolders = copy
        setAdding(.asking(plan, scopes))
    }

    /// Add, in the sheet: the choice is kept for the next folder from the
    /// same kind of place.
    func confirmAdding() async {
        guard case .asking(let plan, _) = adding else { return }
        for folder in plan.folders {
            UserDefaults.standard.set(plan.copyFolders, forKey: folder.source.defaultsKey)
        }
        await fetch(plan)
    }

    /// Use Them Where They Are…: `folder` is kept and the single images
    /// inside it are added in place; with nothing left to choose, at once.
    /// Returns what to say when it can't be used; the sheet stays.
    func useFolder(_ folder: Picked) async -> UserText? {
        guard case .asking(let asked, _) = adding else { return nil }
        let before = asked.loose.count
        guard await keep(folder) else { return .app(Self.accessNotKept(folder.name)) }
        let read = await readable(asked.loose.map(\.url.path))
        // Canceled meanwhile.
        guard case .asking(var plan, _) = adding else { return nil }
        plan.cover { read.contains($0) }
        guard plan.loose.count < before else {
            // Not their folder. Its root goes with the rest no file names
            // when the Library is next read.
            return .app(LocalizedStringResource(
                "That folder doesn't hold the frames you picked. Choose the folder they are in, or add them as copies.",
                comment: "Alert text in the Add Frames sheet after Use Them Where They Are…: the folder picked is not the one the picked frames are in."))
        }
        log.info("added from Files: their folder picked, \(before - plan.loose.count) of \(before) single images used where they are")
        if plan.asks {
            setAdding(.asking(plan, adding.scopes))
        } else {
            await fetch(plan)
        }
        return nil
    }

    /// OK under a failure: what came through is read into the list.
    func acknowledgeAddFailure() async {
        guard case .failed(_, let paths) = adding else { return }
        setAdding(.none)
        guard !held else { return }
        await addPaths(paths)
    }

    /// Cancel: nothing is added; `fetch` deletes what was copied once it
    /// stops.
    func cancelAdding() {
        switch adding {
        case .asking:
            endAdding()
        case .fetching(_, let cancel, _): cancel.cancel()
        case .planning, .reading, .failed: break
        case .none: break
        }
    }

    private func endAdding() {
        setAdding(.none)
    }

    private func failAdding(_ text: UserText, then paths: [String] = []) {
        log.info("adding failed: \(paths.count) paths came through")
        setAdding(.failed(text, then: paths))
    }

    /// List and size what was picked, its files looked at through
    /// `Blocking`.
    private static func plan(_ picked: [Picked]) async -> AddPlan {
        var items: [AddPlan.Item] = []
        for picked in picked {
            let url = picked.url.standardizedFileURL
            let isFolder = (try? await Blocking.run {
                (try? url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) ?? false
            }) ?? false
            let images = isFolder ? await FrameFiles.images(in: url) : (FrameFiles.isImage(url) ? [url] : [])
            guard !images.isEmpty,
                  let (bytes, source) = try? await Blocking.run(.interactive, {
                      let bytes = images.reduce(Int64(0)) { sum, image in
                          sum + Int64((try? image.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
                      }
                      return (bytes, FileSource.of(url))
                  })
            else { continue }
            items.append(AddPlan.Item(
                url: url, picked: picked, isFolder: isFolder, source: source, images: images, bytes: bytes))
        }
        let folders = items.filter { $0.isFolder && $0.source != .owned }
        return AddPlan(
            items: items, copyFolders: folders.contains { $0.source.copies },
            free: (try? await Blocking.run { AppFiles.freeSpace() }) ?? nil)
    }

    /// One file: copied to `target`, or with none only downloaded in place.
    struct FetchJob: Sendable {
        let source: URL
        let target: URL?
        /// A download or a copy from off the device; a copy on the device
        /// is a clone, done at once.
        let slow: Bool

        init(source: URL, target: URL?, from: FileSource) {
            self.source = source
            self.target = target
            slow = target == nil || from != .device
        }
    }

    /// Copy what is copied, download what stays in place with a cloud
    /// service, then add it all. From the sheet it is fetching at once, so
    /// Cancel stops it and Add can't start it again.
    private func fetch(_ plan: AddPlan) async {
        let cancel = FetchCancel()
        if case .asking(_, let scopes) = adding {
            setAdding(.fetching(
                FetchProgress(done: 0, total: plan.fetchCount, downloadOnly: plan.copied.isEmpty), cancel, scopes))
        }
        guard !plan.short else {
            let needed = Self.size(plan.copyBytes)
            let free = Self.size(plan.free ?? 0)
            failAdding(.app(LocalizedStringResource(
                "There isn't enough space to copy these frames into Deptheos: they need \(needed) and \(free) is free. Free up space, then add them again.",
                comment: "Alert text after Add from Files. The placeholders are sizes, such as '2.1 GB': what the copy needs and what the device has free.")))
            return
        }
        var jobs: [FetchJob] = []
        var made: [URL] = []
        var paths = plan.owned.map(\.url.path)
        do {
            if !plan.loose.isEmpty {
                let folder = try AppFiles.newFolder(in: "Imports", named: "Import")
                AppFiles.notBackedUp(folder)
                made.append(folder)
                jobs += plan.loose.flatMap { item in
                    item.images.map { FetchJob(source: $0, target: folder, from: item.source) }
                }
            }
            for item in plan.copied where item.isFolder {
                let folder = try AppFiles.newImportFolder(named: item.name)
                made.append(folder)
                jobs += item.images.map { FetchJob(source: $0, target: folder, from: item.source) }
            }
        } catch {
            for folder in made { try? FileManager.default.removeItem(at: folder) }
            failAdding(Self.message(error))
            return
        }
        paths += made.map(\.standardizedFileURL.path)
        // The folders whose access could not be kept: left out, and said.
        var notKept: [String] = []
        for item in plan.inPlace {
            guard await keep(item.picked) else {
                notKept.append(item.name)
                continue
            }
            paths.append(item.url.path)
            // A cloud service's frame may not be on the device: got now, so
            // the folder lists whole.
            if item.source.downloads {
                jobs += item.images.map { FetchJob(source: $0, target: nil, from: item.source) }
            }
        }
        // Canceled while a folder was kept: what was made goes.
        if cancel.isCanceled {
            for folder in made { try? FileManager.default.removeItem(at: folder) }
            endAdding()
            return
        }
        // Their folder's access is kept already.
        for item in plan.covered {
            paths.append(item.url.path)
            if item.source.downloads {
                jobs += item.images.map { FetchJob(source: $0, target: nil, from: item.source) }
            }
        }
        var failed: (count: Int, reason: String?) = (0, nil)
        // What the failures can have been: copies, downloads or both.
        let copies = jobs.contains { $0.target != nil }
        let downloads = jobs.contains { $0.target == nil }
        if !jobs.isEmpty {
            let downloadOnly = !jobs.contains { $0.target != nil }
            // Clones on the device alone stay planning, so no sheet flashes
            // up. After asking, the sheet is up and fetching already.
            let fetching = if case .fetching = adding { true } else { false }
            if fetching || jobs.contains(where: \.slow) {
                setAdding(.fetching(
                    FetchProgress(done: 0, total: jobs.count, downloadOnly: downloadOnly), cancel, adding.scopes))
            }
            let outcome = (try? await Blocking.run(.background) { [weak self, jobs] in
                Self.run(jobs, cancel: cancel) { done in
                    Task { @MainActor in self?.fetched(done) }
                }
            }) ?? nil
            guard let outcome, !cancel.isCanceled else {
                // Canceled: nothing is added.
                for folder in made { try? FileManager.default.removeItem(at: folder) }
                endAdding()
                return
            }
            failed = outcome
            log.info("added from Files: \(jobs.count - failed.count) of \(jobs.count) files fetched")
        }
        // A copy that took nothing is not a folder to add.
        for folder in made where await FrameFiles.images(in: folder).isEmpty {
            try? FileManager.default.removeItem(at: folder)
            paths.removeAll { $0 == folder.standardizedFileURL.path }
        }
        // Canceled as the last file came: nothing is added.
        if cancel.isCanceled {
            for folder in made { try? FileManager.default.removeItem(at: folder) }
            endAdding()
            return
        }
        // One thing is said: a folder left out, before files of the rest.
        var problem: UserText?
        if let name = notKept.first {
            problem = .app(Self.accessNotKept(name))
        } else if failed.count > 0 {
            problem = .app(Self.fetchFailed(
                failed.count, reason: failed.reason, copies: copies, downloads: downloads))
        }
        if let problem {
            failAdding(problem, then: paths)
            return
        }
        endAdding()
        guard !held else { return }
        await addPaths(paths)
    }

    /// Add from Photos: the originals are copied into their own folder in
    /// Imports, then read into the list. Photos that can't be read are said
    /// once the rest is in.
    func addPhotos(_ items: [PhotosPickerItem]) async {
        guard !busy, !adding.isActive, !items.isEmpty else { return }
        let folder: URL
        do {
            folder = try AppFiles.newFolder(in: "Imports", named: "Import")
        } catch {
            return failAdding(Self.message(error))
        }
        AppFiles.notBackedUp(folder)
        let cancel = FetchCancel()
        setAdding(.fetching(FetchProgress(done: 0, total: items.count, downloadOnly: false), cancel, .empty))
        var copied = 0
        for (index, item) in items.enumerated() where !cancel.isCanceled {
            // One photo that can't be read doesn't stop the others.
            if let file = try? await item.loadTransferable(type: PhotoFile.self) {
                let target = AppFiles.unusedName(file.url.lastPathComponent, in: folder)
                if (try? FileManager.default.moveItem(at: file.url, to: target)) != nil { copied += 1 }
                try? FileManager.default.removeItem(at: file.url.deletingLastPathComponent())
            }
            fetched(index + 1)
        }
        log.info("added from Photos: \(copied) of \(items.count) copied")
        guard !cancel.isCanceled, copied > 0 else {
            // Canceled, or nothing came: no folder to add.
            try? FileManager.default.removeItem(at: folder)
            if cancel.isCanceled { return endAdding() }
            return failAdding(.app(LocalizedStringResource(
                "None of the photos could be read.",
                comment: "Alert text: none of the photos picked from Photos could be copied into Deptheos.")))
        }
        let path = folder.standardizedFileURL.path
        guard copied == items.count else {
            return failAdding(.app(LocalizedStringResource(
                "\(items.count - copied) photos couldn't be read; the others were added.",
                comment: "Alert text: some of the photos picked from Photos couldn't be copied. The placeholder is how many.")),
                then: [path])
        }
        endAdding()
        guard !held else { return }
        await addPaths([path])
    }

    func fetched(_ done: Int) {
        guard case .fetching(var progress, let cancel, let scopes) = adding else { return }
        progress.done = max(progress.done, done)
        setAdding(.fetching(progress, cancel, scopes))
    }

    /// Do `jobs` in turn, reporting after each: how many failed and why the
    /// first did; `nil` when canceled.
    nonisolated static func run(
        _ jobs: [FetchJob], cancel: FetchCancel, progress: @Sendable (Int) -> Void
    ) -> (count: Int, reason: String?)? {
        var failed = 0
        var reason: String?
        for (index, job) in jobs.enumerated() {
            if cancel.isCanceled { return nil }
            do {
                if let folder = job.target {
                    try FrameFiles.reading(job.source, cancel: cancel) { actual in
                        // On the device this is a clone: no space, no time.
                        try FileManager.default.copyItem(
                            at: actual, to: AppFiles.unusedName(job.source.lastPathComponent, in: folder))
                    }
                } else if let why = try FrameFiles.unreadable(job.source.path, cancel: cancel) {
                    failed += 1
                    reason = reason ?? why
                }
            } catch is FrameFiles.Canceled {
                return nil
            } catch {
                failed += 1
                reason = reason ?? error.localizedDescription
            }
            progress(index + 1)
        }
        return (failed, reason)
    }

    /// "14.2 GB", in the user's locale.
    nonisolated static func size(_ bytes: Int64) -> String {
        bytes.formatted(.byteCount(style: .file))
    }
}

extension AppModel {
    /// "3 frames couldn't be copied.", with the system's reason if given.
    nonisolated static func fetchFailed(
        _ count: Int, reason: String?, copies: Bool, downloads: Bool
    ) -> LocalizedStringResource {
        switch (copies && downloads ? 2 : copies ? 0 : 1, reason) {
        case (0, let reason?):
            LocalizedStringResource(
                "\(count) frames couldn't be copied. \(reason)",
                comment: "Alert text after Add from Files: frames picked couldn't be copied into Deptheos. The placeholders are how many frames and the system's sentence saying why.")
        case (0, nil):
            LocalizedStringResource(
                "\(count) frames couldn't be copied.",
                comment: "Alert text after Add from Files: frames picked couldn't be copied into Deptheos. The placeholder is how many frames.")
        case (1, let reason?):
            LocalizedStringResource(
                "\(count) frames couldn't be downloaded. \(reason)",
                comment: "Alert text after Add from Files: frames picked couldn't be downloaded where they are. The placeholders are how many frames and the system's sentence saying why.")
        case (1, nil):
            LocalizedStringResource(
                "\(count) frames couldn't be downloaded.",
                comment: "Alert text after Add from Files: frames picked couldn't be downloaded where they are. The placeholder is how many frames.")
        case (_, let reason?):
            LocalizedStringResource(
                "\(count) frames couldn't be copied or downloaded. \(reason)",
                comment: "Alert text after Add from Files: frames picked couldn't be copied into Deptheos or downloaded. The placeholders are how many frames and the system's sentence saying why.")
        case (_, nil):
            LocalizedStringResource(
                "\(count) frames couldn't be copied or downloaded.",
                comment: "Alert text after Add from Files: frames picked couldn't be copied into Deptheos or downloaded. The placeholder is how many frames.")
        }
    }
}
