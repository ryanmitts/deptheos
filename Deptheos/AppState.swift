import Foundation
import Metal
import CoreGraphics
#if canImport(AppKit)
import AppKit
#endif
import UniformTypeIdentifiers
import DeptheosKit
import AsyncAlgorithms

enum PaintSaveStatus {
    case saved, unsaved, saving
}

/// Central app state — owns the Metal context, viewport infrastructure,
/// and coordinates stacking, display, and project lifecycle.
@Observable
@MainActor
final class AppState {
    let metalContext: MetalContext
    let appSettings: AppSettings

    // Viewport layer
    let viewport: ViewportState

    // Stacking
    let stacking: StackingController
    let displayBridge: DisplayBridge

    // Current project
    var currentProject: StackingProject?

    // Project store (sandbox auto-save)
    let projectStore = ProjectStore()
    var sidebarSelection: SidebarSelection? = .newStack

    /// The currently selected project URL (for save paths, highlight).
    var selectedProjectID: URL? {
        get {
            if case .project(let url) = sidebarSelection { return url }
            return nil
        }
        set {
            sidebarSelection = newValue.map { .project($0) }
        }
    }

    // Export
    var showExportSheet: Bool = false

    // File dialog state (iOS uses SwiftUI .fileImporter modifiers)
    var showOpenFiles: Bool = false
    var showOpenFolder: Bool = false
    var showOpenProject: Bool = false

    // Paint mode
    let paintCoordinator = PaintCoordinator()
    var isPaintMode: Bool = false
    var paintSaveStatus: PaintSaveStatus = .saved
    private var paintSaveChannel: AsyncChannel<Void>?
    private var paintSaveConsumer: Task<Void, Never>?
    private var paintIsDirty: Bool = false

    // Workspace
    let workspace = WorkspaceState()

    // Before/after hold-to-preview (backslash key)
    var isHoldingBeforeAfter = false
    private var beforeAfterSavedTexture: MTLTexture?

    // View mode
    var comparisonStyle: ComparisonStyle = .sideBySide {
        didSet { viewport.renderer.markDirty() }
    }

    /// The effective view mode, derived from current state.
    var viewMode: ViewMode {
        if isPaintMode || (workspace.selectedImageIndex != nil && currentProject != nil) {
            switch comparisonStyle {
            case .compare: return .compare
            case .sideBySide: return .sideBySide
            case .sideBySideVertical: return .sideBySideVertical
            }
        } else if currentProject != nil {
            return .result
        } else {
            return .preview
        }
    }


    init(appSettings: AppSettings) throws {
        self.appSettings = appSettings
        DeptheosDng.initialize()

        let ctx = try MetalContext()
        self.metalContext = ctx
        let vp = ViewportState(device: ctx.device, commandQueue: ctx.commandQueue)
        self.viewport = vp
        self.displayBridge = DisplayBridge(metalContext: ctx)
        self.stacking = try StackingController(
            metalContext: ctx,
            workspace: workspace,
            viewport: vp,
            displayBridge: displayBridge
        )

        // Wire view mode computation to DisplayState for the renderer
        viewport.displayState.viewModeProvider = { [weak self] in
            self?.viewMode ?? .preview
        }

        // Wire stacking lifecycle callbacks
        stacking.onWillStack = { [weak self] in
            guard let self else { return }
            if self.isPaintMode {
                self.syncPaintToProject()
                self.exitPaintMode()
            }
            self.currentProject = nil
        }

        stacking.onStackingComplete = { [weak self] result in
            guard let self else { return }
            await self.handleStackingComplete(result)
        }
    }

    private func handleStackingComplete(_ result: StackingResult) async {
        do {
            // Preserve the original validArea before cropping — compare sources
            // need it to warp+crop raw source images to match the cropped result.
            let originalValidArea = result.validArea

            // Crop alignment borders if needed
            let cropped = appSettings.autoCrop ? try await result.cropped() : result

            // Run DNG color pipeline and display
            try await displayBridge.showResult(cropped, in: viewport.displayState)

            viewport.fitToTexture(
                width: cropped.resultTexture.width,
                height: cropped.resultTexture.height
            )

            // Create project using config/transforms captured by the coordinator.
            // Pass originalValidArea so compare sources can be warped+cropped to match.
            guard let config = stacking.coordinator.activeConfig else { return }
            let transforms: [AffineTransform2D?] = stacking.coordinator.lastTransforms.map { $0 as AffineTransform2D? }

            let project = StackingProject(
                result: cropped,
                config: config,
                cameraRAWBackend: workspace.cameraRAWBackend,
                sourceImagePaths: workspace.allURLs.map(\.path),
                transforms: transforms,
                validArea: appSettings.autoCrop ? originalValidArea : nil,
                imageIncluded: workspace.imageItems.map(\.isIncluded)
            )
            currentProject = project

            // Auto-save to sandbox
            let urls = workspace.allURLs
            let ctx = metalContext
            let store = projectStore
            Task {
                do {
                    let savedURL = try await Self.backgroundAutoSave(
                        store: store, project: project, urls: urls, ctx: ctx
                    )
                    selectedProjectID = savedURL
                    await store.scanProjects()
                } catch {
                    print("[AppState] Auto-save failed: \(error)")
                }
            }
        } catch {
            stacking.coordinator.error = error.localizedDescription
        }
    }

    // MARK: - Save / Open Project

    func saveProject() {
        guard let project = currentProject else { return }

        #if os(macOS)
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.deptheosProject]
        panel.nameFieldStringValue = "Untitled.deptheos"
        panel.message = "Save stacking project"

        guard panel.runModal() == .OK, let url = panel.url else { return }
        saveProject(to: url)
        #else
        // On iOS, save directly via the project store (auto-save)
        let ctx = metalContext
        Task {
            do {
                try await projectStore.autoSave(
                    project: project,
                    imageURLs: workspace.allURLs,
                    metalContext: ctx
                )
                await projectStore.scanProjects()
            } catch {
                stacking.coordinator.error = "Save failed: \(error.localizedDescription)"
            }
        }
        #endif
    }

    func saveProject(to url: URL) {
        guard let project = currentProject else { return }

        let bookmarks = BookmarkManager.createBookmarks(for: workspace.allURLs)
        let ctx = metalContext

        Task {
            do {
                try await Self.backgroundSaveProject(project, to: url, bookmarks: bookmarks, ctx: ctx)
                print("[AppState] Saved project to \(url.path)")
            } catch {
                print("[AppState] Save failed: \(error)")
                stacking.coordinator.error = "Save failed: \(error.localizedDescription)"
            }
        }
    }

    func openProject() {
        #if os(macOS)
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.deptheosProject]
        panel.canChooseDirectories = true
        panel.canChooseFiles = true
        panel.message = "Open stacking project"

        guard panel.runModal() == .OK, let url = panel.url else { return }
        openProject(from: url)
        #else
        showOpenProject = true
        #endif
    }

    func openProject(from url: URL) {
        let ctx = metalContext
        let bridge = displayBridge
        let vp = viewport

        Task {
            do {
                let (project, resolvedURLs) = try await Self.backgroundLoadProject(from: url, ctx: ctx)

                // Show painted result if paint data exists, otherwise the raw result
                if let paintTex = project.paintTexture {
                    if let srgb = try? await bridge.renderLinearToSRGB(
                        texture: paintTex, renderParams: project.dngRenderParams
                    ) {
                        vp.displayState.setMainTexture(srgb, width: srgb.width, height: srgb.height)
                    }
                } else {
                    await bridge.showProject(project, in: vp.displayState)
                }

                currentProject = project
                restoreSettings(from: project)

                let urls: [URL]
                if !resolvedURLs.isEmpty {
                    urls = resolvedURLs
                } else {
                    urls = project.sourceImagePaths.map { URL(fileURLWithPath: $0) }
                }
                workspace.setImageItems(
                    from: urls,
                    included: project.imageIncluded
                )

                vp.fitToTexture(
                    width: project.resultTexture.width,
                    height: project.resultTexture.height
                )

                print("[AppState] Opened project from \(url.path)")
            } catch {
                print("[AppState] Open failed: \(error)")
                stacking.coordinator.error = "Open failed: \(error.localizedDescription)"
            }
        }
    }

    // MARK: - Project Store

    func loadProjectFromStore(_ entry: ProjectEntry) {
        // Save any paint data from the current project before switching
        if isPaintMode {
            syncPaintToProject()
            exitPaintMode()
        } else if currentProject?.paintTexture != nil {
            Task { await performPaintSave() }
        }

        let ctx = metalContext
        let bridge = displayBridge
        let vp = viewport
        let store = projectStore

        Task {
            do {
                let (project, resolvedURLs) = try await store.loadProject(
                    entry: entry, metalContext: ctx
                )

                // Show painted result if paint data exists, otherwise the raw result
                if let paintTex = project.paintTexture {
                    if let srgb = try? await bridge.renderLinearToSRGB(
                        texture: paintTex, renderParams: project.dngRenderParams
                    ) {
                        vp.displayState.setMainTexture(srgb, width: srgb.width, height: srgb.height)
                    }
                } else {
                    await bridge.showProject(project, in: vp.displayState)
                }

                currentProject = project
                restoreSettings(from: project)
                workspace.setImageItems(
                    from: resolvedURLs,
                    included: project.imageIncluded
                )
                vp.fitToTexture(
                    width: project.resultTexture.width,
                    height: project.resultTexture.height
                )

                print("[AppState] Loaded project from store: \(entry.name)")
            } catch {
                print("[AppState] Store load failed: \(error)")
                stacking.coordinator.error = "Load failed: \(error.localizedDescription)"
            }
        }
    }

    // MARK: - Workspace

    /// Reset to a fresh workspace — clear project, images, and display.
    func resetWorkspace() {
        if isPaintMode { exitPaintMode() }
        currentProject = nil
        workspace.removeAllImages()
        viewport.displayState.clear()
        viewport.renderer.markDirty()
        stacking.coordinator.error = nil
    }

    /// Restore UI stacking settings from a project.
    private func restoreSettings(from project: StackingProject) {
        workspace.restoreSettings(from: project)
    }

    // MARK: - Paint Mode

    func enterPaintMode() {
        guard let project = currentProject, !isPaintMode else { return }

        paintCoordinator.cameraRAWBackend = project.cameraRAWBackend
        paintCoordinator.dngConverterCache = workspace.dngConverterCache
        paintCoordinator.dcpProfileDirectory = appSettings.dcpProfileDirectory
        paintCoordinator.enterPaintMode(
            project: project,
            metalContext: metalContext,
            displayBridge: displayBridge
        )
        isPaintMode = true
        paintSaveStatus = .saved
        paintIsDirty = false

        // Start debounced auto-save consumer
        let channel = AsyncChannel<Void>()
        paintSaveChannel = channel
        paintSaveConsumer = Task { @MainActor [weak self] in
            for await _ in channel.debounce(for: .seconds(2), clock: ContinuousClock()) {
                guard let self, !Task.isCancelled else { return }
                await self.performPaintSave()
            }
        }

        // Run initial DNG pipeline and set display texture
        let ctx = metalContext
        paintCoordinator.paintState.ensureDisplayTexture(metalContext: ctx)

        Task {
            await paintCoordinator.refreshDisplayAsync()
            if let displayTex = paintCoordinator.paintState.displayTexture {
                viewport.displayState.setMainTexture(displayTex, width: displayTex.width, height: displayTex.height)
                viewport.renderer.markDirty()
            }

            // Auto-select first included source and await it so the brush cursor
            // has the source texture ready before the user starts interacting.
            if let firstIncluded = workspace.imageItems.firstIndex(where: { $0.isIncluded }) {
                workspace.selectedImageIndex = firstIncluded
                await loadPaintSourceAsync(at: firstIncluded)
            }
        }
    }

    func exitPaintMode() {
        guard isPaintMode else { return }

        // Tear down auto-save consumer
        paintSaveConsumer?.cancel()
        paintSaveConsumer = nil
        paintSaveChannel = nil

        // Clean up brush cursor and release the source texture
        viewport.renderer.clearBrushCursor()
        viewport.renderer.brushCursorSourceTexture = nil

        let paintTexture = paintCoordinator.exitPaintMode()

        // Store paint texture on the project (in-memory)
        if let tex = paintTexture {
            currentProject?.paintTexture = tex
        }

        isPaintMode = false

        // Show the painted result (or raw result if no paint)
        if let project = currentProject {
            let texToDisplay = project.paintTexture ?? project.resultTexture
            Task {
                let srgb = try? await displayBridge.renderLinearToSRGB(
                    texture: texToDisplay,
                    renderParams: project.dngRenderParams
                )
                if let srgb {
                    viewport.displayState.setMainTexture(srgb, width: srgb.width, height: srgb.height)
                    viewport.renderer.markDirty()
                }
            }
        }

        // Flush pending save immediately if dirty
        if paintIsDirty {
            Task { await performPaintSave() }
        }

        paintSaveStatus = .saved
    }

    /// Sync the current paint canvas to the project (for auto-save before re-stack).
    private func syncPaintToProject() {
        if let canvas = paintCoordinator.paintState.paintTexture {
            currentProject?.paintTexture = canvas
        }
    }

    /// Schedule a debounced auto-save by sending to the channel.
    func schedulePaintAutoSave() {
        paintIsDirty = true
        paintSaveStatus = .unsaved
        Task { await paintSaveChannel?.send(()) }
    }

    /// Called when a paint stroke finishes (mouse up).
    func paintStrokeEnded() {
        updatePaintDisplay()
        schedulePaintAutoSave()
    }

    /// Perform the paint save immediately. Updates paintSaveStatus.
    private func performPaintSave() async {
        syncPaintToProject()

        guard let project = currentProject,
              let paintTex = project.paintTexture,
              let projectURL = selectedProjectID else { return }

        paintSaveStatus = .saving
        paintIsDirty = false

        let ctx = metalContext
        let success = await Self.backgroundPaintSave(
            paintTex: paintTex, projectURL: projectURL, ctx: ctx
        )

        // Only mark saved if nothing dirtied it while we were saving
        if !paintIsDirty {
            paintSaveStatus = success ? .saved : .unsaved
        }
    }

    /// Load a source image for painting at the given index.
    func loadPaintSource(at index: Int) {
        guard isPaintMode, index < workspace.imageItems.count else { return }
        Task { await loadPaintSourceAsync(at: index) }
    }

    /// Awaitable version — used by enterPaintMode to ensure the initial source is ready
    /// before the brush cursor becomes active.
    private func loadPaintSourceAsync(at index: Int) async {
        guard index < workspace.imageItems.count else { return }
        let url = workspace.imageItems[index].url
        await paintCoordinator.prepareSource(url: url, frameIndex: index)
        viewport.renderer.brushCursorSourceTexture = paintCoordinator.paintState.sourceDisplayTexture
        viewport.renderer.markDirty()
    }

    func paintUndo() {
        guard isPaintMode else { return }
        paintCoordinator.undo()
        updatePaintDisplay()
        schedulePaintAutoSave()
    }

    func paintRedo() {
        guard isPaintMode else { return }
        paintCoordinator.redo()
        updatePaintDisplay()
        schedulePaintAutoSave()
    }

    func paintUndoAll() {
        guard isPaintMode else { return }
        paintCoordinator.undoAll()
        updatePaintDisplay()
        schedulePaintAutoSave()
    }

    func paintRedoAll() {
        guard isPaintMode else { return }
        paintCoordinator.redoAll()
        updatePaintDisplay()
        schedulePaintAutoSave()
    }

    func paintResetToOriginal() {
        guard isPaintMode else { return }
        paintCoordinator.resetToOriginal()
        updatePaintDisplay()
        schedulePaintAutoSave()
    }

    /// Update the viewport with the latest paint display texture.
    /// The display texture is updated in-place by the DNG pipeline, so we just need
    /// to ensure it's set as the main texture and mark the renderer dirty.
    func updatePaintDisplay() {
        if let displayTex = paintCoordinator.paintState.displayTexture {
            viewport.displayState.setMainTexture(displayTex, width: displayTex.width, height: displayTex.height)
        }
        viewport.renderer.markDirty()
    }

    // MARK: - Before/After Preview

    /// Hold backslash: temporarily show the selected source image as the main texture.
    func beginBeforeAfterPreview() {
        guard currentProject != nil, !isPaintMode, !isHoldingBeforeAfter else { return }
        guard let mainTex = viewport.displayState.mainTexture else { return }

        // Save current result texture
        beforeAfterSavedTexture = mainTex
        isHoldingBeforeAfter = true

        // Ensure a source image is selected, then swap it into the main slot
        ensureImageSelected()
        if let compareTex = viewport.displayState.compareTexture {
            viewport.displayState.setMainTexture(compareTex, width: compareTex.width, height: compareTex.height)
            viewport.renderer.markDirty()
        }
    }

    /// Release backslash: restore the result texture.
    func endBeforeAfterPreview() {
        guard isHoldingBeforeAfter, let saved = beforeAfterSavedTexture else { return }
        viewport.displayState.setMainTexture(saved, width: saved.width, height: saved.height)
        viewport.renderer.markDirty()
        beforeAfterSavedTexture = nil
        isHoldingBeforeAfter = false
    }

    // MARK: - Image Selection

    /// Deselect the current image, returning to result/preview view.
    func deselectImage() {
        workspace.selectedImageIndex = nil
        viewport.displayState.setCompareTexture(nil)
    }

    /// Ensure an image is selected (for compare/side-by-side). Selects the first included image if none.
    func ensureImageSelected() {
        if workspace.selectedImageIndex == nil {
            if let first = workspace.imageItems.firstIndex(where: { $0.isIncluded }) {
                workspace.selectedImageIndex = first
                loadSelectedImage()
            }
        }
    }

    // MARK: - Source Image Display

    func loadSelectedImage() {
        guard let index = workspace.selectedImageIndex, index < workspace.imageItems.count else {
            viewport.displayState.setCompareTexture(nil)
            return
        }

        // In paint mode, load as paint source and also as compare texture
        if isPaintMode {
            loadPaintSource(at: index)
        }

        let url = workspace.imageItems[index].url
        let bridge = displayBridge
        let vp = viewport

        if let project = currentProject {
            // Post-stacking: load as compare texture with warp+crop
            Task {
                bridge.cameraRAWBackend = project.cameraRAWBackend
                bridge.dngConverterCache = self.workspace.dngConverterCache
                bridge.dcpProfileDirectory = self.appSettings.dcpProfileDirectory
                await bridge.showSourceCompare(
                    url: url,
                    frameIndex: index,
                    transforms: project.transforms,
                    validArea: project.validArea,
                    renderParams: project.dngRenderParams,
                    in: vp.displayState
                )
            }
        } else {
            // Pre-stacking: show as main texture preview
            Task {
                bridge.cameraRAWBackend = self.workspace.cameraRAWBackend
                bridge.dngConverterCache = self.workspace.dngConverterCache
                bridge.dcpProfileDirectory = self.appSettings.dcpProfileDirectory
                await bridge.showSourcePreview(url: url, in: vp.displayState)
                if let tex = vp.displayState.mainTexture {
                    vp.fitToTexture(width: tex.width, height: tex.height)
                }
            }
        }
    }

    // MARK: - Background Work (@concurrent)

    @concurrent
    private static func backgroundAutoSave(
        store: ProjectStore, project: StackingProject,
        urls: [URL], ctx: MetalContext
    ) async throws -> URL {
        try await store.autoSave(project: project, imageURLs: urls, metalContext: ctx)
    }

    @concurrent
    private static func backgroundSaveProject(
        _ project: StackingProject, to url: URL,
        bookmarks: [Data], ctx: MetalContext
    ) async throws {
        try await project.save(to: url, metalContext: ctx)
        BookmarkManager.saveBookmarks(bookmarks, to: url)
    }

    @concurrent
    private static func backgroundLoadProject(
        from url: URL, ctx: MetalContext
    ) async throws -> (StackingProject, [URL]) {
        let project = try await StackingProject.load(from: url, metalContext: ctx)
        let bookmarks = BookmarkManager.loadBookmarks(from: url)
        let resolvedURLs = BookmarkManager.resolveBookmarks(bookmarks)
        return (project, resolvedURLs)
    }

    @concurrent
    private static func backgroundPaintSave(
        paintTex: MTLTexture, projectURL: URL, ctx: MetalContext
    ) async -> Bool {
        guard let pixelData = await paintTex.readPixelData(
            commandQueue: ctx.commandQueue
        ) else {
            print("[AppState] Failed to read paint texture for save")
            return false
        }

        do {
            // JXL encoding is CPU-intensive — bridge to DispatchQueue
            // to avoid starving the cooperative thread pool
            let width = paintTex.width
            let height = paintTex.height
            let jxlData = try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Data, Error>) in
                DispatchQueue.global(qos: .utility).async {
                    do {
                        cont.resume(returning: try JXLCodec.encode(
                            pixels: pixelData,
                            width: width,
                            height: height,
                            effort: 3
                        ))
                    } catch {
                        cont.resume(throwing: error)
                    }
                }
            }
            let paintURL = projectURL.appendingPathComponent("paint.jxl")
            try jxlData.write(to: paintURL)
            return true
        } catch {
            print("[AppState] Failed to save paint data: \(error)")
            return false
        }
    }
}

/// Sidebar navigation selection.
enum SidebarSelection: Hashable {
    case newStack
    case project(URL)
}

