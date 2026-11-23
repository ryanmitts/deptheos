import SwiftUI
@preconcurrency import DeptheosEngine

/// The Stack Settings, always the working version's: presented in a
/// `NavigationStack` both as the iPad's anchored popover and the iPhone's
/// sheet, with `StackStatusSection` at the head and `Stack` in the bar.
struct InspectorView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollViewReader { proxy in
            List {
                StackStatusSection()
                SettingsSection(pages: true)
                #if DEBUG
                Color.clear.frame(height: 1).id("end").listRowBackground(Color.clear)
                    .listRowSeparator(.hidden)
                #endif
            }
            // `-ScrollInspector YES` scrolls the settings to their end.
            .automation(Automation.flag("ScrollInspector"), after: .seconds(4)) {
                withAnimation { proxy.scrollTo("end", anchor: .bottom) }
            }
        }
        .listStyle(.insetGrouped)
    }
}

/// The sheet's first section, only with something to say: a run's progress
/// and Cancel, or a paused batch's; else the change line with Revert and
/// why Stack is off.
struct StackStatusSection: View {
    @Environment(AppModel.self) private var model

    /// Under a running stack's progress.
    static let background = LocalizedStringResource(
        "The stack goes on in the background while iOS allows it. Where iOS doesn't, leaving the app stops the stack.",
        comment: "Under a running stack's progress in Settings.")

    var body: some View {
        if let stacking = model.stacking {
            Section {
                StackProgressRow(stacking: stacking)
                    .buttonStyle(.bordered)
            } footer: {
                Text(Self.background)
            }
        } else if let run = model.batchRun {
            Section {
                PausedBatchRow(run: run)
                    .buttonStyle(.bordered)
            }
        } else {
            let changed = model.showing.changeLine != nil
            let hint = model.stackHint
            if changed || hint != nil {
                Section {
                    if changed {
                        ChangeLine(asRow: true)
                    }
                    if let hint {
                        Text(hint)
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                    }
                }
            }
        }
    }
}

/// How the working version stands against the render on screen, in the
/// engine's words, with Revert: "2 changes to the frames since #4", "1
/// difference from #3 in the settings". Nothing for the unedited base or no
/// render, and no Revert where it would change nothing. Revert runs at once
/// when nothing would be lost, else asks beside its button.
struct ChangeLine: View {
    @Environment(AppModel.self) private var model
    /// The row's insets in place of its own padding.
    var asRow = false
    @State private var asking: RevertInfo?

    /// The change line's button, and the confirmation's.
    static let revert = LocalizedStringResource(
        "Revert",
        comment: "Button beside the change line above Stack, and the destructive button of the dialog it may ask with: makes the working version the frames and settings of the render shown.")

    /// "Revert to #3", the menu bar's; "Revert" with no render.
    static func revertTitle(_ number: UInt32?) -> LocalizedStringResource {
        guard let number else { return revert }
        return LocalizedStringResource(
            "Revert to #\(number)",
            comment: "Menu item (the menu bar's Stack menu, a render's menu in the Renders list): makes the working version that render's frames and settings, based on it. The placeholder is its number.")
    }

    var body: some View {
        let showing = model.showing
        if let line = showing.changeLine {
            HStack(spacing: 8) {
                // Edited since the base; another render's line says how far
                // it is, not a change.
                if showing.kind == .base {
                    EditedDot()
                        .accessibilityHidden(true)
                }
                // The engine's words.
                Text(verbatim: line)
                    .font(.subheadline)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 8)
                if let revert = showing.revert {
                    Button(Self.revert) {
                        if revert.alert == nil {
                            Task { await model.revert(to: revert.render) }
                        } else {
                            asking = revert
                        }
                    }
                    .frame(minHeight: 44)
                    // In a form's row, only the button takes the tap.
                    .buttonStyle(.borderless)
                    .disabled(model.busy || model.paint.active)
                    .help(LocalizedStringResource(
                        "Make the frames and settings those of #\(revert.number)",
                        comment: "Pointer tooltip of Revert, above Stack. The placeholder is the number of the render it reverts to."))
                    .revertDialog($asking)
                }
            }
            .padding(.horizontal, asRow ? 0 : nil)
            .padding(.top, asRow ? 0 : 6)
        }
    }
}

extension View {
    /// Revert's confirmation in the engine's words, while `asking` would
    /// lose unstacked changes.
    func revertDialog(_ asking: Binding<RevertInfo?>) -> some View {
        modifier(RevertDialog(asking: asking))
    }
}

private struct RevertDialog: ViewModifier {
    @Environment(AppModel.self) private var model
    @Binding var asking: RevertInfo?

    func body(content: Content) -> some View {
        content.confirmationDialog(
            Text(verbatim: asking?.alert?.title ?? ""),
            isPresented: Binding(get: { asking?.alert != nil }, set: { if !$0 { asking = nil } }),
            titleVisibility: .visible, presenting: asking
        ) { revert in
            Button(ChangeLine.revert, role: .destructive) {
                Task { await model.revert(to: revert.render) }
            }
            Button(
                LocalizedStringResource(
                    "Cancel", comment: "Revert's confirmation: closes it, and nothing changes."),
                role: .cancel
            ) {}
        } message: { revert in
            // The engine's words.
            Text(verbatim: revert.alert?.body ?? "")
        }
    }
}

/// "2 frames left out · Show": Show selects the first in the list.
struct LeftOutSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let count = model.leftOutRows.count
        if count > 0 {
            Section {
                HStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.orange)
                    Text(verbatim: AppWords.words.leftOutLine(count: UInt32(count)))
                    Spacer()
                    Button(LocalizedStringResource(
                        "Show", comment: "Settings button: shows the frames the stack left out in the list.")
                    ) {
                        Task { await model.showLeftOut() }
                    }
                    .disabled(model.held)
                }
            }
        }
    }
}

/// The method and what it's good for, its main slider with the rest under
/// Advanced, and alignment. A setting changed since the base has a dot,
/// a slider a tick at the render shown's value. Footers say what moving
/// each does, in the engine's words.
struct SettingsSection: View {
    @Environment(AppModel.self) private var model
    /// As rows opening pages (the iPhone's sheet).
    var pages = false
    /// Where it opens in place.
    @State private var advancedOpen = false

    var body: some View {
        @Bindable var settings = model.stackSettings
        let was = model.baseRender?.settings
        let old = OldValues(was, shown: model.shownRender?.settings)
        Section {
            Picker(LocalizedStringResource("Method", comment: "Settings: the picker of the stacking method."), selection: $settings.algorithm) {
                ForEach(AlgorithmKind.allCases) { Text($0.label).tag($0) }
            }
            .pickerStyle(.segmented)
            if pages {
                // The iPhone's sheet is half the screen: the slider sits under
                // the picker, in view with the photo.
                methodControls(settings, old: old)
            } else {
                methodDescription(settings)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
        } header: {
            SettingName(
                text: LocalizedStringResource("Method", comment: "Settings: the heading over the stacking method's picker."),
                edited: was != nil && old.algorithm != settings.algorithm)
        } footer: {
            if pages {
                VStack(alignment: .leading, spacing: 6) {
                    methodDescription(settings)
                    methodHelp(settings)
                }
            }
        }
        .disabled(model.held)
        LeftOutSection()
        Group {
            if !pages {
                Section {
                    methodControls(settings, old: old)
                } footer: {
                    methodHelp(settings)
                }
            }
            if model.offersRawDecoder {
                rawDecoder(settings, was: was)
            }
            if pages {
                Section {
                    NavigationLink {
                        List {
                            Section {
                                alignment(settings, was: was)
                            } footer: {
                                Help(settings: [.aligner, .crop])
                            }
                        }
                        .navigationTitle(LocalizedStringResource(
                            "Alignment", comment: "Title of the iPhone page with the alignment settings."))
                        .disabled(model.held)
                    } label: {
                        HStack {
                            Text("Alignment", comment: "iPhone Settings: the row that opens the alignment settings.")
                            if let was, was.align != settings.align || was.alignMethod != settings.alignMethod || was.crop != settings.crop {
                                EditedDot()
                            }
                            Spacer()
                            Text(alignmentSummary(settings)).foregroundStyle(.secondary)
                        }
                    }
                }
            } else {
                Section {
                    alignment(settings, was: was)
                } header: {
                    Text("Alignment", comment: "Settings: the section header over the alignment settings.")
                } footer: {
                    Help(settings: [.aligner, .crop])
                }
            }
            Section {
                Button(LocalizedStringResource(
                    "Reset Settings", comment: "Settings button: puts the method's sliders back to their defaults.")) { settings.reset() }
            }
        }
        .disabled(model.held)
    }

    private func methodDescription(_ settings: StackSettingsModel) -> Text {
        Text(verbatim: AppWords.words.algorithmDescription(algorithm: settings.settings.algorithm))
    }

    /// For Depth, the rest under Advanced.
    @ViewBuilder
    private func methodControls(_ settings: StackSettingsModel, old: OldValues) -> some View {
        @Bindable var settings = settings
        switch settings.algorithm {
        case .pyramid:
            SettingSlider(
                name: LocalizedStringResource("Smoothness", comment: "Settings: the Pyramid method's slider, a name."),
                value: $settings.smoothness, range: settings.ranges.smoothness,
                old: old.smoothness, tick: old.shown.smoothness, ends: (Self.crisp, Self.calm))
        case .depth:
            SettingSlider(
                name: LocalizedStringResource("Smoothing", comment: "Settings: the Depth method's main slider, a name."),
                value: $settings.smoothing, range: settings.ranges.smoothing,
                old: old.smoothing, tick: old.shown.smoothing,
                ends: (
                    LocalizedStringResource("Narrow outlines", comment: "Settings: under the Smoothing slider's low end."),
                    LocalizedStringResource("Even backgrounds", comment: "Settings: under the Smoothing slider's high end.")))
            let advanced = HStack {
                Text("Advanced", comment: "Settings, for Depth: the row that opens its Radius and Blend Range sliders.")
                if changed(old.radius, settings.radius) || changed(old.blendRange, settings.blendRange) {
                    EditedDot()
                }
                Spacer()
                Text(
                    "Radius, blend range",
                    comment: "Settings, for Depth: beside Advanced, what it holds."
                ).foregroundStyle(.secondary)
            }
            if pages {
                NavigationLink {
                    List {
                        Section {
                            depthAdvanced(settings, old: old)
                        } footer: {
                            Help(settings: [.radius, .blendRange])
                        }
                    }
                    .navigationTitle(LocalizedStringResource(
                        "Advanced", comment: "Title of the page with Depth's Radius and Blend Range sliders."))
                    .disabled(model.held)
                } label: {
                    advanced
                }
            } else {
                DisclosureGroup(isExpanded: $advancedOpen) {
                    depthAdvanced(settings, old: old)
                } label: {
                    advanced
                }
            }
        case .wavelet:
            SettingSlider(
                name: LocalizedStringResource("Denoise", comment: "Settings: the Wavelet method's slider, a name."),
                value: $settings.denoise, range: settings.ranges.denoise,
                old: old.denoise, tick: old.shown.denoise,
                ends: (
                    LocalizedStringResource("All detail", comment: "Settings: under the Denoise slider's low end."),
                    LocalizedStringResource("Less noise", comment: "Settings: under the Denoise slider's high end.")))
        }
    }

    /// What moving the sliders shown does.
    @ViewBuilder
    private func methodHelp(_ settings: StackSettingsModel) -> some View {
        switch settings.algorithm {
        case .pyramid: Help(settings: [.smoothness])
        case .depth: Help(settings: !pages && advancedOpen ? [.smoothing, .radius, .blendRange] : [.smoothing])
        case .wavelet: Help(settings: [.denoise])
        }
    }

    @ViewBuilder
    private func depthAdvanced(_ settings: StackSettingsModel, old: OldValues) -> some View {
        @Bindable var settings = settings
        SettingSlider(
            name: LocalizedStringResource("Radius", comment: "Settings, Depth's Advanced: a slider, a name."),
            value: $settings.radius, range: settings.ranges.radius,
            old: old.radius, tick: old.shown.radius,
            ends: (
                LocalizedStringResource("Thin detail", comment: "Settings: under the Radius slider's low end."),
                LocalizedStringResource("Steadier", comment: "Settings: under the Radius slider's high end.")))
        SettingSlider(
            name: LocalizedStringResource("Blend Range", comment: "Settings, Depth's Advanced: a slider, a name."),
            value: $settings.blendRange,
            range: settings.ranges.blendRange, old: old.blendRange, tick: old.shown.blendRange,
            ends: (
                LocalizedStringResource("One frame per spot", comment: "Settings: under the Blend Range slider's low end."),
                LocalizedStringResource("Mixed, softer", comment: "Settings: under the Blend Range slider's high end.")))
    }

    @ViewBuilder
    private func alignment(_ settings: StackSettingsModel, was: StackSettings?) -> some View {
        @Bindable var settings = settings
        Toggle(isOn: $settings.align) {
            SettingName(
                text: LocalizedStringResource("Align frames", comment: "Settings switch: line the frames up before stacking."),
                edited: was.map { $0.align != settings.align } ?? false)
        }
        // The desktop's two aligners, by its names for them.
        Picker(selection: $settings.alignMethod) {
            ForEach([AlignMode.ecc, .grid], id: \.self) { method in
                // The engine's name for the aligner.
                Text(verbatim: AppWords.words.alignMethodLabel(method: method)).tag(method)
            }
        } label: {
            SettingName(
                text: LocalizedStringResource("Aligner", comment: "Settings: the picker of the aligner, ECC or Grid search."),
                edited: was.map { $0.alignMethod != settings.alignMethod } ?? false)
        }
        .disabled(!settings.align)
        Toggle(isOn: $settings.crop) {
            SettingName(
                text: LocalizedStringResource(
                    "Crop to the area every frame covers",
                    comment: "Settings switch: crop the result to the area all the aligned frames cover."),
                edited: was.map { $0.crop != settings.crop } ?? false)
        }
        .disabled(!settings.align)
    }

    /// Shown while the list has camera RAW or mosaic DNGs (ADR-131):
    /// Apple's decoder renders them, LibRaw keeps them linear.
    @ViewBuilder
    private func rawDecoder(_ settings: StackSettingsModel, was: StackSettings?) -> some View {
        @Bindable var settings = settings
        Section {
            Picker(selection: $settings.rawDecoder) {
                Text("Apple", comment: "Settings, RAW Decoder: Apple's decoder, a name.").tag(RawBackend.apple)
                Text(verbatim: "LibRaw").tag(RawBackend.libRaw)
            } label: {
                SettingName(
                    text: LocalizedStringResource(
                        "RAW Decoder", comment: "Settings: the picker of who decodes camera RAW files, Apple or LibRaw."),
                    edited: was.map { $0.rawDecoder != settings.rawDecoder } ?? false)
            }
        } footer: {
            switch settings.rawDecoder {
            case .libRaw, .adobeDng:
                Text(
                    "LibRaw keeps RAW files linear, as the camera recorded them: the result exports as DNG too, to develop later.",
                    comment: "Settings: under RAW Decoder, with LibRaw chosen.")
            case .apple:
                Text(
                    "Apple's decoder develops RAW files with the system's color and tone: the result exports as TIFF, JPEG or PNG. Linear DNGs, such as ProRAW, stay linear either way.",
                    comment: "Settings: under RAW Decoder, with Apple chosen.")
            }
        }
    }

    /// "ECC (affine) · cropped", "Grid search (similarity) · uncropped",
    /// "Off".
    private func alignmentSummary(_ settings: StackSettingsModel) -> UserText {
        guard settings.align else {
            return .app(LocalizedStringResource("Off", comment: "iPhone Settings, beside Alignment: frames aren't aligned."))
        }
        // The engine's name for the aligner.
        let method = UserText.verbatim(AppWords.words.alignMethodLabel(method: settings.alignMethod))
        return .line([
            method,
            settings.crop
                ? .app(LocalizedStringResource(
                    "cropped", comment: "iPhone Settings, beside Alignment, after the aligner's name: the result is cropped to the area every frame covers."))
                : .app(LocalizedStringResource(
                    "uncropped", comment: "iPhone Settings, beside Alignment, after the aligner's name: the result isn't cropped.")),
        ])
    }

    /// The low end of Smoothness.
    private static let crisp = LocalizedStringResource(
        "Crisp, more noise", comment: "Settings: under the Smoothness slider's low end.")
    /// Smoothness's high end.
    private static let calm = LocalizedStringResource(
        "Calm, softer", comment: "Settings: under the Smoothness slider's high end.")

    private func changed(_ old: Float?, _ value: Float) -> Bool {
        old.map { abs($0 - value) > 0.0001 } ?? false
    }
}

/// The engine's help lines, one sentence per setting (the desktop's
/// slider tooltips).
private struct Help: View {
    let settings: [SettingHelp]

    var body: some View {
        Text(verbatim: settings.map { AppWords.words.settingHelp(setting: $0) }.joined(separator: " "))
    }
}

/// The base's values, where its method is chosen, for the dots; the render
/// shown's, for the ticks.
private struct OldValues {
    var algorithm: AlgorithmKind?
    var smoothness: Float?
    var radius: Float?
    var smoothing: Float?
    var blendRange: Float?
    var denoise: Float?
    var shown: Values

    /// By method.
    struct Values {
        var smoothness: Float?
        var radius: Float?
        var smoothing: Float?
        var blendRange: Float?
        var denoise: Float?
    }

    init(_ settings: StackSettings?, shown: StackSettings?) {
        self.shown = Values()
        switch shown?.algorithm {
        case .pyramid(let smoothness): self.shown.smoothness = smoothness
        case .depth(let radius, let smoothing, let blendRange):
            (self.shown.radius, self.shown.smoothing, self.shown.blendRange) = (radius, smoothing, blendRange)
        case .wavelet(let denoise, _): self.shown.denoise = denoise
        case nil: break
        }
        switch settings?.algorithm {
        case .pyramid(let smoothness):
            algorithm = .pyramid
            self.smoothness = smoothness
        case .depth(let radius, let smoothing, let blendRange):
            algorithm = .depth
            (self.radius, self.smoothing, self.blendRange) = (radius, smoothing, blendRange)
        case .wavelet(let denoise, _):
            algorithm = .wavelet
            self.denoise = denoise
        case nil:
            break
        }
    }
}

/// A change since the base: read as "Changed" on a setting, hidden from
/// VoiceOver beside words that say so.
struct EditedDot: View {
    static let changed = LocalizedStringResource(
        "Changed", comment: "VoiceOver: a setting or render changed since the base render.")

    var body: some View {
        Circle().fill(Color.edited).frame(width: 6, height: 6)
            .accessibilityLabel(Self.changed)
    }
}

private struct SettingName: View {
    let text: LocalizedStringResource
    let edited: Bool

    var body: some View {
        HStack(spacing: 6) {
            Text(text)
            if edited { EditedDot() }
        }
    }
}

/// A slider with its name and value; with `old`, a dot once moved from the
/// base's value; with `tick`, a tick at the render shown's value; with
/// `ends`, what each end does.
struct SettingSlider: View {
    let name: LocalizedStringResource
    @Binding var value: Float
    let range: SettingRange
    var old: Float?
    var tick: Float?
    var ends: (LocalizedStringResource, LocalizedStringResource)?

    var body: some View {
        let changed = old.map { abs($0 - value) > 0.0001 } ?? false
        let ticked = tick.flatMap { abs($0 - value) > 0.0001 ? $0 : nil }
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                SettingName(text: name, edited: changed)
                Spacer()
                Text(value, format: .number.precision(.fractionLength(range.step < 1 ? 1 : 0)))
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
            }
            Slider(value: $value, in: range.min...range.max, step: range.step)
                .overlay(alignment: .leading) {
                    if let ticked {
                        GeometryReader { geometry in
                            let fraction = CGFloat((ticked - range.min) / max(range.max - range.min, 0.0001))
                            // Not the edited color: it marks the render shown.
                            Rectangle()
                                .fill(.secondary)
                                .frame(width: 2, height: 10)
                                // The track runs between the thumb's centers.
                                .position(
                                    x: 14 + (geometry.size.width - 28) * fraction,
                                    y: geometry.size.height / 2)
                        }
                        .allowsHitTesting(false)
                        .accessibilityHidden(true)
                    }
                }
            if let ends {
                HStack {
                    Text(ends.0)
                    Spacer()
                    Text(ends.1)
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
        .accessibilityValue(changed ? EditedDot.changed : .empty)
    }
}

/// A stack in progress: the batch's progress, the stage and bar, and
/// Cancel while there's something to cancel. Its container styles it.
struct StackProgressRow: View {
    static let cancel = LocalizedStringResource(
        "Cancel", comment: "Button beside a stack's or a batch's progress: stops it. What is made is kept.")

    @Environment(AppModel.self) private var model
    let stacking: StackingState
    var spacing: CGFloat = 12

    var body: some View {
        HStack(spacing: spacing) {
            VStack(alignment: .leading, spacing: 6) {
                if let run = model.batchRun {
                    // The engine's words.
                    Text(verbatim: run.progress).font(.callout.weight(.medium)).lineLimit(1)
                }
                Text(stacking.label).font(.callout)
                ProgressView(value: stacking.fraction)
            }
            // A single stack being saved has nothing left to cancel.
            if model.batchRun != nil || !stacking.isSaving {
                Button(Self.cancel, role: .cancel) { model.cancelStack() }
            }
        }
    }
}

/// A batch paused in the background: how far it is, and Cancel.
struct PausedBatchRow: View {
    @Environment(AppModel.self) private var model
    let run: BatchRun
    var spacing: CGFloat = 12

    var body: some View {
        HStack(spacing: spacing) {
            // The engine's words.
            Text(verbatim: run.progress).font(.callout)
            Button(StackProgressRow.cancel, role: .cancel) { model.cancelStack() }
        }
    }
}
