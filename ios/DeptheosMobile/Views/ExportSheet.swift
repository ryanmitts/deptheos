import Photos
import SwiftUI
import UIKit
import os
@preconcurrency import DeptheosEngine

/// Each lifecycle transition, one line.
private let stateLog = AppLog(category: "state")

/// Export a render (the shown result, or one from the Library): the engine
/// writes the file once a destination is chosen: Photos, Share or Files.
struct ExportSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    let result: StackResult
    /// For its paint and its name.
    let renderID: String

    /// The format used last for this kind of stack, remembered as it
    /// changes.
    @State private var format = ExportFormat.jpeg
    /// From Settings ▸ Export; a change here is kept as the preference, as
    /// on the desktop.
    @State private var quality = Double(defaultPreferences().export.jpegQuality)
    @State private var sixteenBit = true
    /// An adjusted camera RAW render's; the last choice, as on the desktop.
    @State private var space = ExportSpace.srgb
    /// Whether the render has adjustments set: `nil` until read.
    @State private var adjusted: Bool?
    @State private var export = Export.idle {
        didSet { stateLog.debug("export sheet: \(export.name, privacy: .public)") }
    }
    @State private var photosDenied = false

    private enum Destination: Equatable {
        case photos, share, files
        #if DEBUG
        /// The end-to-end test's folder (`E2E.exports`).
        case testFolder
        #endif
    }

    /// The file for the options chosen, and where it is going. Each
    /// destination writes the file if there is none, then sends it.
    ///
    /// Lifecycle: idle, writing (options, destination), written (the file)
    /// or sending (file, destination). Held in `export`, changed only by
    /// the buttons, the writes and sends they start, and an option changed.
    ///
    ///     idle ──a destination──▶ writing(options, destination) ──written──▶ sending(file, destination)
    ///     writing ──failed, or the options changed meanwhile──▶ idle
    ///     written ──a destination──▶ sending(file, destination)
    ///     sending ──saved to Photos or Files──▶ (the sheet closes)
    ///     sending ──shared, canceled or failed──▶ written(file)
    ///     written ──an option changed──▶ idle
    ///     any ──the sheet closes──▶ idle
    private enum Export {
        case idle
        case writing(Options, Destination)
        /// Kept for the next destination, while the options stay.
        case written(URL)
        case sending(URL, Destination)

        var name: String {
            switch self {
            case .idle: "idle"
            case .writing(_, let destination): "writing for \(destination)"
            case .written: "written"
            case .sending(_, let destination): "sending to \(destination)"
            }
        }

        /// While it matches the chosen options.
        var file: URL? {
            switch self {
            case .written(let file), .sending(let file, _): file
            case .idle, .writing: nil
            }
        }

        var destination: Destination? {
            switch self {
            case .writing(_, let destination), .sending(_, let destination): destination
            case .idle, .written: nil
            }
        }
    }

    /// The share sheet or Files mover, while sending there; closed without
    /// a result, the file is kept.
    private func presenting(_ destination: Destination) -> Binding<Bool> {
        Binding(
            get: { if case .sending(_, destination) = export { true } else { false } },
            set: { shown in
                if !shown, case .sending(let file, destination) = export { export = .written(file) }
            })
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Picker(LocalizedStringResource("Format", comment: "Export sheet: the picker of the file format."), selection: $format) {
                        ForEach(result.exportFormats(), id: \.self) { format in
                            // The engine's name for the format.
                            Text(verbatim: exportFormatLabel(format: format)).tag(format)
                        }
                    }
                    .accessibilityIdentifier("export-format")
                    switch format {
                    case .jpeg:
                        VStack(alignment: .leading) {
                            Text(
                                "Quality \(Int(quality))",
                                comment: "Export sheet: the JPEG quality slider's label. The placeholder is the quality, from 1 to 100.")
                                // The slider says it.
                                .accessibilityHidden(true)
                            Slider(value: $quality, in: qualityRange, step: 1)
                                .accessibilityLabel(Text(verbatim: AppWords.words.preferenceLabel(preference: .jpegQuality)))
                                .accessibilityValue(Text(Int(quality), format: .number))
                        }
                    case .png, .tiff:
                        Toggle(
                            LocalizedStringResource(
                                "16 bits per channel", comment: "Export sheet and Settings ▸ Export switch for PNG and TIFF: write 16 bits per color channel rather than 8."),
                            isOn: Binding(get: { sixteenWritten }, set: { sixteenBit = $0 }))
                            .disabled(depthFixed)
                            .accessibilityIdentifier("export-16")
                    case .dng:
                        Text(
                            "DNG keeps the linear camera data, with the frames' camera profile, for your RAW editor to develop.",
                            comment: "Export sheet: the note shown when DNG is the format.")
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                    if offersSpaces {
                        Picker(
                            LocalizedStringResource(
                                "Color", comment: "Export sheet: the picker of the color space an adjusted RAW render is written in."),
                            selection: Binding(get: { spaceWritten }, set: { space = $0 })
                        ) {
                            ForEach(ExportSpace.allCases.filter(isAvailable), id: \.self) { space in
                                // The engine's name for the space.
                                Text(verbatim: AppWords.words.exportSpaceLabel(space: space)).tag(space)
                            }
                        }
                        .accessibilityIdentifier("export-space")
                        Text(verbatim: AppWords.words.exportSpaceProphotoNote())
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                } footer: {
                    // A DNG keeps the whole picture, its crop a tag.
                    let size = format == .dng
                        ? PictureSize(width: result.width(), height: result.height()) : result.outputSize()
                    Text(
                        "\(size.width.formatted(.number.grouping(.never))) × \(size.height.formatted(.number.grouping(.never))) pixels",
                        comment: "Export sheet footer: the exported picture's size. The placeholders are its width and height in pixels, as numbers in the user's locale.")
                }
                Section {
                    #if DEBUG
                    if E2E.exports != nil {
                        // Not translated: debug builds only, for the test; first, so
                        // it shows at the sheet's first height.
                        Button {
                            Task { await send(to: .testFolder) }
                        } label: {
                            Label { Text(verbatim: "Save to Test Folder") } icon: { Image(systemName: "testtube.2") }
                        }
                        .disabled(export.destination != nil)
                        .accessibilityIdentifier("export-test-folder")
                    }
                    #endif
                    row(
                        .photos,
                        LocalizedStringResource("Save to Photos", comment: "Export sheet: saves the exported file directly to the Photos library."),
                        systemImage: "photo.badge.plus")
                    row(
                        .share,
                        LocalizedStringResource("Share…", comment: "Export sheet: opens the system's share sheet with the exported file."),
                        systemImage: "square.and.arrow.up")
                    .background(ShareSheet(file: presenting(.share).wrappedValue ? export.file : nil) {
                        presenting(.share).wrappedValue = false
                    })
                    row(
                        .files,
                        LocalizedStringResource("Save to Files…", comment: "Export sheet: saves the exported file in the Files app."),
                        systemImage: "folder")
                }
            }
            .navigationTitle(LocalizedStringResource("Export", comment: "Title of the Export sheet."))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(role: .close) { dismiss() }
                }
            }
            .onChange(of: format) { _, format in
                model.preferences.remember(format, for: kind)
            }
            .onChange(of: quality) { _, quality in
                model.preferences.export.jpegQuality = UInt8(quality)
            }
            .onChange(of: sixteenBit) { _, sixteenBit in
                // Only where it's a choice: a result without 16 bits shows
                // it off.
                if !depthFixed {
                    model.preferences.export.sixteenBit = sixteenBit
                }
            }
            .onChange(of: space) { _, space in
                model.preferences.export.space = space
            }
            .task {
                // The sheet is for this result alone: closing it cancels the read.
                guard let read = await model.exportAdjusted(result), !Task.isCancelled else { return }
                adjusted = read
            }
            .onChange(of: options) {
                if case .written(let file) = export {
                    discard(file)
                    export = .idle
                }
            }
            // The file is done with; one still being written is discarded
            // when it arrives.
            .onDisappear {
                if let file = export.file { discard(file) }
                export = .idle
            }
            .onAppear {
                // The format used last for this kind of stack; the first
                // time, DNG for a RAW stack (it keeps linear data), else JPEG.
                let formats = result.exportFormats()
                let fallback: ExportFormat = formats.contains(.dng) ? .dng : .jpeg
                format = model.preferences.lastFormat(for: kind).flatMap { formats.contains($0) ? $0 : nil } ?? fallback
                let preferred = model.preferences.export
                quality = Double(preferred.jpegQuality)
                sixteenBit = preferred.sixteenBit
                space = preferred.space
            }
            .fileMover(isPresented: presenting(.files), file: export.file) { moved in
                switch moved {
                case .success:
                    // Moved: nothing left to discard but its folder.
                    dismiss()
                case .failure(let error):
                    model.report(error)
                }
            }
            .alert(
                LocalizedStringResource(
                    "Photos Access Off",
                    comment: "Alert title when Save to Photos is tapped without permission to add to the Photos library."),
                isPresented: $photosDenied
            ) {
                Button(LocalizedStringResource(
                    "Open Settings",
                    comment: "Export sheet alert button when Photos access is off: opens Deptheos's page in the Settings app.")
                ) {
                    if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
                }
                Button(LocalizedStringResource("Cancel", comment: "Cancels an alert."), role: .cancel) {}
            } message: {
                Text(
                    "Allow Deptheos to add photos in Settings to save exports to your photo library.",
                    comment: "Alert message when Save to Photos is tapped without permission to add to the Photos library.")
            }
        }
        .singleItemSheetBar()
        .presentationDetents([.medium, .large])
    }

    /// Its symbol turns to a spinner while writing or sending for it; the
    /// others wait.
    private func row(_ destination: Destination, _ title: LocalizedStringResource, systemImage: String) -> some View {
        Button {
            Task { await send(to: destination) }
        } label: {
            Label {
                Text(title)
            } icon: {
                if export.destination == destination {
                    ProgressView()
                } else {
                    Image(systemName: systemImage)
                }
            }
        }
        .disabled(export.destination != nil)
    }

    /// As written: the bits and space the format and stack allow.
    private struct Options: Equatable {
        let format: ExportFormat
        let quality: Double
        let sixteenBit: Bool
        let space: ExportSpace
    }

    private var options: Options {
        Options(format: format, quality: quality, sixteenBit: sixteenWritten, space: spaceWritten)
    }

    /// 16 bits can be written: the stack has them, or it is adjusted.
    private var hasSixteen: Bool {
        result.hasSixteenBit() || adjusted == true
    }

    /// An adjusted camera RAW render's JPEG, PNG or TIFF picks its space.
    private var offersSpaces: Bool {
        result.exportOffersSpaces(format: format, adjusted: adjusted == true)
    }

    private func isAvailable(_ space: ExportSpace) -> Bool {
        exportSpaceAvailable(space: space, format: format, source: hasSixteen)
    }

    /// The space written: ProPhoto only at 16 bits, else Adobe RGB.
    private var spaceWritten: ExportSpace {
        exportSpaceForFormat(space: space, format: format, source: hasSixteen)
    }

    /// ProPhoto holds the depth at 16 bits.
    private var prophoto: Bool {
        offersSpaces && spaceWritten == .proPhoto
    }

    private var depthFixed: Bool {
        exportDepthFixed(format: format, source: hasSixteen) || prophoto
    }

    private var sixteenWritten: Bool {
        exportSixteenBit(format: format, asked: sixteenBit || prophoto, source: hasSixteen)
    }

    /// For the format remembered: RAW and DNG stacks can export DNG.
    private var kind: AppPreferences.ExportKind {
        result.exportFormats().contains(.dng) ? .dng : .standard
    }

    /// The engine's.
    private var qualityRange: ClosedRange<Double> {
        let range = preferenceRange(preference: .jpegQuality)
        return Double(range?.min ?? 1)...Double(range?.max ?? 100)
    }

    private func send(to destination: Destination) async {
        if destination == .photos {
            switch PHPhotoLibrary.authorizationStatus(for: .addOnly) {
            case .denied, .restricted:
                photosDenied = true
                return
            default:
                break
            }
        }
        let file: URL
        switch export {
        case .written(let written):
            file = written
        case .idle:
            let asked = options
            export = .writing(asked, destination)
            // The file name pattern and TIFF compression are the
            // preferences'.
            var preferences = model.preferences.export
            preferences.jpegQuality = UInt8(asked.quality)
            preferences.sixteenBit = asked.sixteenBit
            preferences.space = asked.space
            let written = await model.export(
                result, renderID: renderID, format: asked.format, options: preferences)
            // The sheet closed or the options changed meanwhile: not the
            // file wanted now.
            guard case .writing(asked, destination) = export, asked == options, let written else {
                if let written { discard(written) }
                if case .writing(asked, destination) = export { export = .idle }
                return
            }
            file = written
        case .writing, .sending:
            return
        }
        export = .sending(file, destination)
        // The share sheet and the Files mover show while it is sending.
        if destination == .photos { await saveToPhotosLibrary(file) }
        #if DEBUG
        if destination == .testFolder { saveToTestFolder(file) }
        #endif
    }

    #if DEBUG
    /// With a note of what was asked for, in the test's folder.
    private func saveToTestFolder(_ file: URL) {
        let bits = options.sixteenBit ? 16 : 8
        // The frame the render was painted from, while Retouch has it open.
        let paintedFrom = model.paint.painted
            ? model.paint.paintedFrom.sorted().first.map { URL(fileURLWithPath: $0).lastPathComponent }
            : nil
        do {
            try E2E.keep(file, format: options.format, bits: bits, paintedFrom: paintedFrom)
            dismiss()
        } catch {
            model.report(error)
            if case .sending(file, .testFolder) = export { export = .written(file) }
        }
    }
    #endif

    private func saveToPhotosLibrary(_ file: URL) async {
        let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
        guard case .sending(file, .photos) = export else { return }
        guard status == .authorized || status == .limited else {
            export = .written(file)
            photosDenied = true
            return
        }
        do {
            try await Self.addToPhotos(file)
            guard case .sending(file, .photos) = export else { return }
            model.notice = .app(LocalizedStringResource(
                "Saved to Photos.",
                comment: "Notice after saving an exported render to the Photos library."))
            dismiss()
        } catch {
            model.report(error)
            if case .sending(file, .photos) = export { export = .written(file) }
        }
    }

    /// Nonisolated so the change block isn't the main actor's: Photos runs
    /// it on its own queue, and a block formed on the main actor (this
    /// target's default isolation) fails its isolation check and crashes.
    @concurrent nonisolated private static func addToPhotos(_ file: URL) async throws {
        try await PHPhotoLibrary.shared().performChanges {
            let request = PHAssetCreationRequest.forAsset()
            request.addResource(with: .photo, fileURL: file, options: nil)
        }
    }

    /// And its temporary folder (empty once Save to Files has moved it).
    private func discard(_ file: URL) {
        try? FileManager.default.removeItem(at: file.deletingLastPathComponent())
    }
}
