import Metal

/// Tile coordinate key for hashing.
struct TileCoord: Hashable {
    let x: Int
    let y: Int
}

/// GPU-resident snapshot of a single tile.
struct TileSnapshot {
    let coord: TileCoord
    let texture: MTLTexture
    let width: Int
    let height: Int
}

/// A complete undo entry — the set of tiles captured before a stroke.
struct UndoEntry {
    let tiles: [TileSnapshot]
    let byteSize: Int
}

/// Tile-based undo/redo for the paint canvas.
///
/// During a stroke, dirty tiles are tracked and their pre-stroke state is captured
/// via GPU texture-to-texture blits into a pool of tile textures.
/// Undo restores tiles by blitting the saved textures back to the canvas.
///
/// Tile size: 256×256 pixels. Memory budget: 512 MB (FIFO eviction).
final class PaintUndoManager {
    static let tileSize = 256
    static let maxUndoBytes = 512 * 1024 * 1024 // 512 MB
    static let bytesPerPixel = 8 // rgba16Unorm or rgba16Float (both 8 bytes/pixel)
    private static let maxPoolSize = 1024 // match undo budget capacity (~512 MB / 512 KB per tile)

    private let device: MTLDevice
    private var pixelFormat: MTLPixelFormat = .rgba16Unorm

    private(set) var undoStack: [UndoEntry] = []
    private(set) var redoStack: [UndoEntry] = []
    private var undoBytes: Int = 0
    private var redoBytes: Int = 0

    /// Tiles already snapshotted for this stroke (avoid duplicate captures).
    private var strokeSnapshotted: Set<TileCoord> = []
    /// Pre-stroke tile snapshots collected during the stroke.
    private var pendingSnapshots: [TileSnapshot] = []
    private var pendingSnapshotBytes: Int = 0

    /// Pool of reusable tile textures to avoid repeated allocation.
    private var texturePool: [MTLTexture] = []

    var canUndo: Bool { !undoStack.isEmpty }
    var canRedo: Bool { !redoStack.isEmpty }

    init(device: MTLDevice, pixelFormat: MTLPixelFormat = .rgba16Unorm) {
        self.device = device
        self.pixelFormat = pixelFormat
    }

    // MARK: - Tile Texture Pool

    private func takeTileTexture() -> MTLTexture? {
        if let texture = texturePool.popLast() {
            return texture
        }
        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: pixelFormat,
            width: Self.tileSize, height: Self.tileSize, mipmapped: false
        )
        desc.storageMode = .private
        desc.usage = [.shaderRead, .shaderWrite]
        let texture = device.makeTexture(descriptor: desc)
        texture?.label = "paint_undo_tile"
        return texture
    }

    private func returnTileTexture(_ texture: MTLTexture) {
        if texturePool.count < Self.maxPoolSize {
            texturePool.append(texture)
        }
    }

    private func returnTileTextures(from tiles: [TileSnapshot]) {
        for tile in tiles {
            returnTileTexture(tile.texture)
        }
    }

    // MARK: - Tile Coordinate Helpers

    /// Compute which tiles overlap a pixel-space bounding box.
    func tilesForBBox(x: Int, y: Int, width: Int, height: Int) -> [TileCoord] {
        let ts = Self.tileSize
        let minTX = max(x, 0) / ts
        let minTY = max(y, 0) / ts
        let maxTX = max(x + width - 1, 0) / ts
        let maxTY = max(y + height - 1, 0) / ts
        var coords = [TileCoord]()
        for ty in minTY...maxTY {
            for tx in minTX...maxTX {
                coords.append(TileCoord(x: tx, y: ty))
            }
        }
        return coords
    }

    // MARK: - Batched Readback Helper

    /// Snapshot tile regions from a texture into pooled tile textures using a single command buffer.
    private func snapshotTileRegions(
        coords: [(coord: TileCoord, originX: Int, originY: Int, tileW: Int, tileH: Int)],
        from sourceTexture: MTLTexture,
        commandQueue: MTLCommandQueue
    ) -> [TileSnapshot] {
        guard !coords.isEmpty else { return [] }

        guard let cb = commandQueue.makeCommandBuffer(),
              let blit = cb.makeBlitCommandEncoder() else { return [] }

        var snapshots = [TileSnapshot]()
        var totalBytes = 0

        for info in coords {
            guard let tileTexture = takeTileTexture() else { continue }

            blit.copy(
                from: sourceTexture,
                sourceSlice: 0, sourceLevel: 0,
                sourceOrigin: MTLOrigin(x: info.originX, y: info.originY, z: 0),
                sourceSize: MTLSize(width: info.tileW, height: info.tileH, depth: 1),
                to: tileTexture,
                destinationSlice: 0, destinationLevel: 0,
                destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0)
            )

            snapshots.append(TileSnapshot(coord: info.coord, texture: tileTexture, width: info.tileW, height: info.tileH))
            totalBytes += info.tileW * info.tileH * Self.bytesPerPixel
        }

        blit.endEncoding()
        cb.commit()

        return snapshots
    }

    // MARK: - Stroke Lifecycle

    /// Begin a new stroke. Call before any dabs are applied.
    func beginStroke() {
        strokeSnapshotted.removeAll()
        pendingSnapshots.removeAll()
        pendingSnapshotBytes = 0
    }

    /// Mark tiles as dirty for the current stroke and snapshot any that haven't been captured yet.
    /// Must be called before the dab modifies the paint texture.
    func snapshotTilesBeforeDab(
        dabBBoxX: Int, dabBBoxY: Int, dabBBoxW: Int, dabBBoxH: Int,
        paintTexture: MTLTexture,
        commandQueue: MTLCommandQueue
    ) {
        let tiles = tilesForBBox(x: dabBBoxX, y: dabBBoxY, width: dabBBoxW, height: dabBBoxH)
        let newTiles = tiles.filter { !strokeSnapshotted.contains($0) }
        guard !newTiles.isEmpty else { return }

        let ts = Self.tileSize
        let imgW = paintTexture.width
        let imgH = paintTexture.height

        var coords = [(coord: TileCoord, originX: Int, originY: Int, tileW: Int, tileH: Int)]()
        for coord in newTiles {
            let originX = coord.x * ts
            let originY = coord.y * ts
            let tileW = min(ts, imgW - originX)
            let tileH = min(ts, imgH - originY)
            guard tileW > 0, tileH > 0 else { continue }
            coords.append((coord: coord, originX: originX, originY: originY, tileW: tileW, tileH: tileH))
        }

        let snapshots = snapshotTileRegions(coords: coords, from: paintTexture, commandQueue: commandQueue)

        for snapshot in snapshots {
            let bytes = snapshot.width * snapshot.height * Self.bytesPerPixel
            pendingSnapshots.append(snapshot)
            pendingSnapshotBytes += bytes
            strokeSnapshotted.insert(snapshot.coord)
        }
    }

    /// End the current stroke. Pushes the pending snapshots as an undo entry.
    func endStroke() {
        guard !pendingSnapshots.isEmpty else { return }

        let entry = UndoEntry(tiles: pendingSnapshots, byteSize: pendingSnapshotBytes)
        undoStack.append(entry)
        undoBytes += pendingSnapshotBytes

        // Clear redo stack (new stroke invalidates redo history)
        for redoEntry in redoStack {
            returnTileTextures(from: redoEntry.tiles)
        }
        redoBytes = 0
        redoStack.removeAll()

        // Evict oldest undo entries if over budget
        while undoBytes > Self.maxUndoBytes && undoStack.count > 1 {
            let evicted = undoStack.removeFirst()
            undoBytes -= evicted.byteSize
            returnTileTextures(from: evicted.tiles)
        }

        pendingSnapshots.removeAll()
        pendingSnapshotBytes = 0
        strokeSnapshotted.removeAll()
    }

    // MARK: - Undo / Redo

    /// Undo the last stroke. Captures current state of affected tiles to redo stack first.
    func undo(paintTexture: MTLTexture, commandQueue: MTLCommandQueue) -> Bool {
        guard let entry = undoStack.popLast() else { return false }
        undoBytes -= entry.byteSize

        // Capture current state of these tiles for redo
        let redoEntry = captureCurrentState(tiles: entry.tiles, paintTexture: paintTexture, commandQueue: commandQueue)
        redoStack.append(redoEntry)
        redoBytes += redoEntry.byteSize

        // Restore tiles from undo entry
        restoreTiles(entry.tiles, to: paintTexture, commandQueue: commandQueue)

        // Return the undo entry textures to the pool (data now lives in redoEntry)
        returnTileTextures(from: entry.tiles)

        return true
    }

    /// Redo the last undone stroke.
    func redo(paintTexture: MTLTexture, commandQueue: MTLCommandQueue) -> Bool {
        guard let entry = redoStack.popLast() else { return false }
        redoBytes -= entry.byteSize

        // Capture current state for undo
        let undoEntry = captureCurrentState(tiles: entry.tiles, paintTexture: paintTexture, commandQueue: commandQueue)
        undoStack.append(undoEntry)
        undoBytes += undoEntry.byteSize

        // Restore tiles from redo entry
        restoreTiles(entry.tiles, to: paintTexture, commandQueue: commandQueue)

        // Return the redo entry textures to the pool
        returnTileTextures(from: entry.tiles)

        return true
    }

    /// Clear all undo/redo history.
    func reset() {
        for entry in undoStack { returnTileTextures(from: entry.tiles) }
        for entry in redoStack { returnTileTextures(from: entry.tiles) }
        returnTileTextures(from: pendingSnapshots)
        undoStack.removeAll()
        redoStack.removeAll()
        undoBytes = 0
        redoBytes = 0
        pendingSnapshots.removeAll()
        pendingSnapshotBytes = 0
        strokeSnapshotted.removeAll()
    }

    // MARK: - Private Helpers

    /// Capture current tile state from the paint texture (for redo).
    private func captureCurrentState(
        tiles: [TileSnapshot],
        paintTexture: MTLTexture,
        commandQueue: MTLCommandQueue
    ) -> UndoEntry {
        let ts = Self.tileSize
        let imgW = paintTexture.width
        let imgH = paintTexture.height

        var coords = [(coord: TileCoord, originX: Int, originY: Int, tileW: Int, tileH: Int)]()
        for tile in tiles {
            let originX = tile.coord.x * ts
            let originY = tile.coord.y * ts
            let tileW = min(ts, imgW - originX)
            let tileH = min(ts, imgH - originY)
            guard tileW > 0, tileH > 0 else { continue }
            coords.append((coord: tile.coord, originX: originX, originY: originY, tileW: tileW, tileH: tileH))
        }

        let snapshots = snapshotTileRegions(coords: coords, from: paintTexture, commandQueue: commandQueue)
        let totalBytes = snapshots.reduce(0) { $0 + $1.width * $1.height * Self.bytesPerPixel }
        return UndoEntry(tiles: snapshots, byteSize: totalBytes)
    }

    /// Restore tile textures back to the GPU paint texture.
    private func restoreTiles(
        _ tiles: [TileSnapshot],
        to paintTexture: MTLTexture,
        commandQueue: MTLCommandQueue
    ) {
        guard !tiles.isEmpty else { return }
        let ts = Self.tileSize

        guard let cb = commandQueue.makeCommandBuffer(),
              let blit = cb.makeBlitCommandEncoder() else { return }

        for tile in tiles {
            let originX = tile.coord.x * ts
            let originY = tile.coord.y * ts

            blit.copy(
                from: tile.texture,
                sourceSlice: 0, sourceLevel: 0,
                sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
                sourceSize: MTLSize(width: tile.width, height: tile.height, depth: 1),
                to: paintTexture,
                destinationSlice: 0, destinationLevel: 0,
                destinationOrigin: MTLOrigin(x: originX, y: originY, z: 0)
            )
        }

        blit.endEncoding()
        cb.commit()
        cb.waitUntilCompleted()
    }
}
