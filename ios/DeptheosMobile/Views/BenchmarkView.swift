import SwiftUI
import UIKit
@preconcurrency import DeptheosEngine

/// Settings ▸ Benchmark: times Pyramid + ECC on the currently open stack or a
/// bundled sample, reporting median time and MP/s and exporting the JSON record
/// used by `web/measured/benchmarks/`.
struct BenchmarkView: View {
    @Environment(AppModel.self) private var model

    @State private var source: BenchSource = .sample(.hornet)
    @State private var mode: BenchMode = .quick
    @State private var running = false
    @State private var request: Request?
    @State private var statusText = ""
    @State private var report: BenchSummary?
    @State private var errorMessage: String?
    @State private var copied = false
    @State private var jsonFileURL: URL?

    private var currentStackFrames: [String] {
        model.rows.filter(\.included).map(\.url.path)
    }

    private var hasCurrentStack: Bool {
        currentStackFrames.count >= 2
    }

    private var currentStackLabel: String {
        let name = model.stackName ?? "Current Stack"
        return "\(name) (\(currentStackFrames.count) frames)"
    }

    private var libraryStacks: [LibraryBenchStack] {
        var seen = Set<String>()
        var result: [LibraryBenchStack] = []
        // Newest render per stack first.
        for render in model.history.renders.reversed() {
            if seen.insert(render.stack).inserted {
                let includedCount = zip(render.frames, render.included).filter(\.1).count
                let name = render.stackName ?? "Untitled Stack"
                result.append(LibraryBenchStack(
                    stackID: render.stack,
                    renderID: render.id,
                    name: name,
                    frames: includedCount
                ))
            }
        }
        return result
    }

    private var availableSources: [BenchSource] {
        var list: [BenchSource] = []
        var includedIDs = Set<String>()
        if hasCurrentStack {
            list.append(.currentStack)
            includedIDs.insert(model.stackID)
        }
        for stack in libraryStacks where !includedIDs.contains(stack.stackID) && stack.frames >= 2 {
            list.append(.library(stack))
        }
        for sample in SampleStack.offered where sample.bundled != nil {
            list.append(.sample(sample))
        }
        return list
    }

    var body: some View {
        Form {
            Section {
                Picker("Stack", selection: $source) {
                    ForEach(availableSources, id: \.self) { item in
                        Text(sourceTitle(item)).tag(item)
                    }
                }
                .disabled(running)

                Picker("Mode", selection: $mode) {
                    ForEach(BenchMode.allCases, id: \.self) { m in
                        Text(m.label).tag(m)
                    }
                }
                .disabled(running)
            } header: {
                Text("Configuration")
            } footer: {
                Text("Times Pyramid + ECC end to end (disk read, decode, align, stack, and viewer image). Thorough matches `deptheos bench --live --repeat 5 --cool 30`.")
            }

            Section {
                if running {
                    HStack(spacing: 12) {
                        ProgressView()
                        Text(statusText.isEmpty ? "Starting…" : statusText)
                            .foregroundStyle(.secondary)
                    }
                    Button("Cancel", role: .destructive) {
                        cancelBenchmark()
                    }
                } else {
                    Button {
                        startBenchmark()
                    } label: {
                        Label("Run Benchmark", systemImage: "play.fill")
                    }
                    .disabled(model.engine == nil || availableSources.isEmpty)
                }
            }

            if let errorMessage {
                Section {
                    Text(errorMessage)
                        .foregroundStyle(.red)
                }
            }

            if let report {
                Section("Results") {
                    LabeledContent("Median") {
                        Text(String(format: "%.2f s", report.seconds))
                            .monospacedDigit()
                            .fontWeight(.semibold)
                    }
                    LabeledContent("Throughput") {
                        Text(String(format: "%.0f MP/s", report.mpxPerS))
                            .monospacedDigit()
                    }
                    LabeledContent("Per Frame") {
                        Text(String(format: "%.1f ms", report.msPerFrame))
                            .monospacedDigit()
                    }
                    LabeledContent("Range") {
                        Text(String(format: "%.2f–%.2f s", report.fastest, report.slowest))
                            .monospacedDigit()
                    }
                    LabeledContent("Stack") {
                        Text("\(report.frames) × \(report.width)×\(report.height) \(report.format)")
                            .monospacedDigit()
                            .foregroundStyle(.secondary)
                    }
                }

                Section {
                    Button {
                        UIPasteboard.general.string = report.markdown
                        copied = true
                    } label: {
                        Label(copied ? "Copied Summary" : "Copy Summary", systemImage: copied ? "checkmark" : "doc.on.doc")
                    }
                    if let jsonFileURL {
                        ShareLink(item: jsonFileURL) {
                            Label("Share JSON…", systemImage: "square.and.arrow.up")
                        }
                    }
                }
            }
        }
        .navigationTitle("Benchmark")
        .navigationBarTitleDisplayMode(.inline)
        .onAppear {
            if hasCurrentStack {
                source = .currentStack
            } else if let first = availableSources.first {
                source = first
            }
        }
        .onDisappear {
            cancelBenchmark()
        }
    }

    private func sourceTitle(_ item: BenchSource) -> String {
        switch item {
        case .currentStack:
            return currentStackLabel
        case .library(let stack):
            return "\(stack.name) (\(stack.frames) frames)"
        case .sample(.hornet):
            return "Sample: Hornet (66 frames · 11 MP)"
        case .sample(.circuitBoard):
            return "Sample: Circuit Board (200 frames · 11 MP)"
        }
    }

    private func startBenchmark() {
        guard let engine = model.engine else { return }

        running = true
        errorMessage = nil
        report = nil
        copied = false
        jsonFileURL = nil
        statusText = "Preparing frames…"
        UIApplication.shared.isIdleTimerDisabled = true

        let req = Request()
        request = req
        let selectedSource = source
        let currentName = model.stackName ?? "stack"
        let currentPaths = currentStackFrames
        let machine = currentMachineInfo()
        let repeats = mode.repeats
        let coolSecs = mode.coolSecs

        let observer = BenchProgressBridge { update in
            Task { @MainActor in
                switch update {
                case .warmup:
                    statusText = "Warming up GPU shaders…"
                case let .cooldown(round, totalRounds, secondsLeft):
                    statusText = "Cooling down before run \(round) of \(totalRounds) (\(secondsLeft) s)…"
                case let .running(round, totalRounds, frame, frames):
                    statusText = "Run \(round) of \(totalRounds) · Frame \(frame) of \(frames)"
                }
            }
        }

        Task {
            defer {
                UIApplication.shared.isIdleTimerDisabled = false
                running = false
                request = nil
            }
            do {
                let resolved: (name: String, paths: [String])
                switch selectedSource {
                case .currentStack:
                    resolved = (currentName, currentPaths)
                case .library(let stack):
                    let working = try await engine.workingOfRender(request: req, id: stack.renderID)
                    let paths = listRows(list: working.list).filter(\.included).map(\.url.path)
                    resolved = (stack.name, paths)
                case .sample(let sample):
                    guard let dir = sample.bundled,
                          let urls = try? FileManager.default.contentsOfDirectory(
                            at: dir, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles]
                          )
                    else {
                        errorMessage = "Bundled sample frames could not be found."
                        return
                    }
                    let sorted = urls
                        .filter { ["jpg", "jpeg", "png", "tif", "tiff", "dng"].contains($0.pathExtension.lowercased()) }
                        .sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }
                        .map(\.path)
                    resolved = (sample.name, sorted)
                }

                guard resolved.paths.count >= 2 else {
                    errorMessage = "Need at least 2 frames to run a benchmark."
                    return
                }

                statusText = "Warming up GPU shaders…"
                let summary = try await engine.runBenchmark(
                    name: resolved.name,
                    paths: resolved.paths,
                    repeats: repeats,
                    coolSecs: coolSecs,
                    machine: machine,
                    request: req,
                    observer: observer
                )
                report = summary
                jsonFileURL = writeTempJSON(summary)
            } catch EngineError.Canceled {
                // User canceled.
            } catch {
                errorMessage = error.localizedDescription
            }
        }
    }

    private func cancelBenchmark() {
        request?.cancel()
        request = nil
        running = false
        UIApplication.shared.isIdleTimerDisabled = false
    }

    private func writeTempJSON(_ summary: BenchSummary) -> URL? {
        let slug: (String) -> String = { s in
            s.lowercased()
                .map { $0.isLetter || $0.isNumber ? String($0) : "-" }
                .joined()
                .trimmingCharacters(in: CharacterSet(charactersIn: "-"))
        }
        let filename = "\(slug(summary.label))-\(slug(summary.stackName))-\(summary.format.lowercased()).json"
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(filename)
        try? summary.json.write(to: url, atomically: true, encoding: .utf8)
        return url
    }

    private func currentMachineInfo() -> BenchMachine {
        let device = UIDevice.current
        let process = ProcessInfo.processInfo
        device.isBatteryMonitoringEnabled = true
        let power: String? = switch device.batteryState {
        case .charging, .full: "AC Power"
        case .unplugged: "Battery Power"
        default: nil
        }
        let powerMode = process.isLowPowerModeEnabled ? "low power" : "automatic"
        let memGB = (Double(process.physicalMemory) / Double(1 << 30) * 10.0).rounded() / 10.0
        let hwModel = hardwareIdentifier()
        let label = device.model
        let os = "\(device.systemName) \(device.systemVersion)"
        return BenchMachine(
            label: label,
            os: os,
            cpu: hwModel,
            threads: UInt32(process.activeProcessorCount),
            memoryGb: memGB,
            power: power,
            powerMode: powerMode
        )
    }

    private func hardwareIdentifier() -> String {
        var sysinfo = utsname()
        uname(&sysinfo)
        return withUnsafePointer(to: &sysinfo.machine) {
            $0.withMemoryRebound(to: CChar.self, capacity: 1) { String(cString: $0) }
        }
    }
}

private struct LibraryBenchStack: Hashable {
    let stackID: String
    let renderID: String
    let name: String
    let frames: Int
}

private enum BenchSource: Hashable {
    case currentStack
    case library(LibraryBenchStack)
    case sample(SampleStack)
}

private enum BenchMode: CaseIterable, Hashable {
    case quick
    case thorough

    var repeats: UInt32 {
        switch self {
        case .quick: 3
        case .thorough: 5
        }
    }

    var coolSecs: UInt64 {
        switch self {
        case .quick: 5
        case .thorough: 30
        }
    }

    var label: String {
        switch self {
        case .quick: "Quick (3 runs · 5 s pause)"
        case .thorough: "Thorough (5 runs · 30 s cooldown)"
        }
    }
}

private final class BenchProgressBridge: BenchObserver, @unchecked Sendable {
    private let onUpdate: @Sendable (BenchUpdate) -> Void

    init(_ onUpdate: @escaping @Sendable (BenchUpdate) -> Void) {
        self.onUpdate = onUpdate
    }

    func progress(update: BenchUpdate) {
        onUpdate(update)
    }
}
