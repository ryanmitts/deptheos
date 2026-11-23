import Foundation
@preconcurrency import DeptheosEngine

private let log = AppLog(category: "files")
/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// A focus bracket bundled with the app, for Try a Sample Stack, copied
/// into Imports under its name. The catalog, copy and order are the
/// engine's (`BundledSample`), as on the desktop; this adds its menu's
/// words.
nonisolated enum SampleStack: String, Identifiable, Sendable {
    /// 66 frames of a hornet, hand-held.
    case hornet
    /// 200 frames of a circuit board, on a tripod with a focus rail.
    case circuitBoard = "pcb"

    var id: Self { self }

    /// In the engine's order.
    static var offered: [SampleStack] { bundledSamples().map(SampleStack.init) }

    init(_ bridged: BundledSample) {
        switch bridged {
        case .hornet: self = .hornet
        case .circuitBoard: self = .circuitBoard
        }
    }

    var bridged: BundledSample {
        switch self {
        case .hornet: .hornet
        case .circuitBoard: .circuitBoard
        }
    }

    /// Its folder's name in Imports, and so its stack's. A name: not
    /// translated.
    var name: String { sampleName(sample: bridged) }

    var title: LocalizedStringResource {
        switch self {
        case .hornet:
            LocalizedStringResource(
                "Hornet",
                comment: "Item of the Try a Sample Stack menu: the sample stack of a hornet. Its stack is named Hornet, untranslated, as a folder is.")
        case .circuitBoard:
            LocalizedStringResource(
                "Circuit Board",
                comment: "Item of the Try a Sample Stack menu: the sample stack of a circuit board. Its stack is named Circuit Board, untranslated, as a folder is.")
        }
    }

    /// Its frames, and how it was shot.
    var detail: LocalizedStringResource {
        switch self {
        case .hornet:
            LocalizedStringResource(
                "66 frames · handheld",
                comment: "Under the Hornet item of the Try a Sample Stack menu: how many frames the sample has, and that it was shot with the camera held in the hand.")
        case .circuitBoard:
            LocalizedStringResource(
                "200 frames · on a tripod",
                comment: "Under the Circuit Board item of the Try a Sample Stack menu: how many frames the sample has, and that it was shot with the camera on a tripod.")
        }
    }

    var bundled: URL? {
        Bundle.main.url(forResource: sampleFolder(sample: bridged), withExtension: nil)
    }
}

/// Try a Sample Stack: a bundled focus bracket (`SampleStack`) is copied
/// into Imports and opened as a new, unstacked stack.
///
/// Lifecycle: idle, or copying a `SampleStack` through the bridge's
/// `copy_sample` (the engine's `frames::sample` rule). While copying the
/// app is `held`. Changed only by `setSampleCopy`, from here.
///
///     idle ──Try a Sample Stack ▸ a sample──▶ copying(sample) ──a whole copy found or made, and listed──▶ idle,
///                                                 │                the sample opening as a new stack
///                                                 └──it couldn't be copied (no space, the engine's reason)──▶ idle,
///                                                            the alert saying why
///
/// Frames are read from the copy, never the bundle, whose path changes
/// with every update. A whole copy from an earlier try is reused, so each
/// sample takes its space once. The copy is an ordinary Imports copy, so
/// Storage counts it unused once no stack uses it.
enum SampleCopy: Equatable {
    case idle
    /// Copied into Imports, or the copy there checked, then listed.
    case copying(SampleStack)

    var name: String {
        switch self {
        case .idle: "idle"
        case .copying(let sample): "copying \(sample.rawValue)"
        }
    }
}

extension AppModel {
    /// `sample`, copied in once, goes on screen as a new stack; the stack
    /// on screen keeps its working version.
    func trySample(_ sample: SampleStack) async {
        guard engine != nil, !busy, sampleCopy == .idle else { return }
        guard let bundled = sample.bundled else {
            log.error("the sample \(sample.rawValue, privacy: .public) is missing from the app's bundle")
            errorMessage = ErrorMessage(.app(Self.sampleNotCopied))
            return
        }
        setSampleCopy(.copying(sample))
        let started = Date.now
        let imports: URL
        do {
            imports = try AppFiles.importsFolder()
        } catch {
            setSampleCopy(.idle)
            report(error)
            return
        }
        let free = AppFiles.freeSpace().map { UInt64(max($0, 0)) }
        let copied = await ask { engine, request in
            try await engine.copySample(
                request: request, sample: sample.bridged, from: bundled.path, into: imports.path, free: free)
        }
        guard let copied else {
            setSampleCopy(.idle)
            log.info("sample not copied")
            return
        }
        let folder = URL(filePath: copied.folder, directoryHint: .isDirectory)
        if !copied.reused { AppFiles.notBackedUp(folder) }
        let took = Date.now.timeIntervalSince(started)
        log.info("sample \(sample.rawValue, privacy: .public) \(copied.reused ? "found in Imports" : "copied into Imports", privacy: .public) in \(took, format: .fixed(precision: 2)) s")
        let path = folder.standardizedFileURL.path
        let empty = listClear(list: ListInfo(files: [], excluded: [], source: .raw))
        let listed = await ask { _, request in try await listAdd(request: request, list: empty, paths: [path]) }
        // Nothing else could start meanwhile (`held`); the copy is ours.
        guard sampleCopy == .copying(sample) else { return }
        setSampleCopy(.idle)
        guard let listed else { return }
        let working = newWorking(listed.files, settings: settings)
        // The photo and its Stack button, no sheet over them.
        await switchStack(to: working.stack, opening: .sample(working.stack), fallback: working) { _ in
            slideIn(then: .closed)
            await loadSourcePreview()
        }
    }

    /// With no reason to give.
    nonisolated static let sampleNotCopied = LocalizedStringResource(
        "The sample stack couldn't be copied into Deptheos.",
        comment: "Alert text after Try a Sample Stack, when there is no reason to give.")
}
