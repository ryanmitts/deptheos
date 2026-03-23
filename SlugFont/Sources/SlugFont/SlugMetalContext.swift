import Metal

public enum SlugMetalError: Error {
    case noDevice
    case noCommandQueue
    case noLibrary
    case functionNotFound(String)
}

/// Shared Metal device, command queue, and shader library.
public final class SlugMetalContext: @unchecked Sendable {
    public let device: MTLDevice
    public let commandQueue: MTLCommandQueue
    public let library: MTLLibrary

    public init(device: MTLDevice? = nil) throws {
        guard let dev = device ?? MTLCreateSystemDefaultDevice() else {
            throw SlugMetalError.noDevice
        }
        self.device = dev

        guard let queue = dev.makeCommandQueue() else {
            throw SlugMetalError.noCommandQueue
        }
        self.commandQueue = queue

        // Try compiled metallib first (Xcode), fall back to runtime compilation (SPM).
        if let lib = try? dev.makeDefaultLibrary(bundle: Bundle.module) {
            self.library = lib
        } else {
            let headerURL = Bundle.module.url(forResource: "SlugShaderTypes", withExtension: "h")
            let header = try headerURL.map { try String(contentsOf: $0, encoding: .utf8) } ?? ""
            let shaderURLs = Bundle.module.urls(forResourcesWithExtension: "metal", subdirectory: nil) ?? []
            guard !shaderURLs.isEmpty else {
                throw SlugMetalError.noLibrary
            }
            let shaderSource = try shaderURLs
                .map { try String(contentsOf: $0, encoding: .utf8) }
                .joined(separator: "\n")
                .replacingOccurrences(of: "#include \"SlugShaderTypes.h\"", with: "")
                .replacingOccurrences(of: "#include <metal_stdlib>\nusing namespace metal;", with: "")
            self.library = try dev.makeLibrary(source: header + "\n" + shaderSource, options: nil)
        }
    }
}
