import SwiftUI
import UIKit
@preconcurrency import DeptheosEngine

/// The menu bar and keyboard shortcuts (the iPad's, and the Command-hold
/// list), with the desktop app's keys.
struct AppCommands: Commands {
    let model: AppModel

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button(LocalizedStringResource("New Stack", comment: "Menu item: starts a new, empty stack.")) {
                Task { await model.startNewStack() }
            }
            .keyboardShortcut("n")
            .disabled(model.engine == nil || model.busy || model.showCamera)
            Button(CameraView.takeBurst) {
                model.present(.camera)
            }
            .keyboardShortcut("n", modifiers: [.command, .shift])
            .disabled(model.held || model.showCamera)
        }
        CommandMenu(LocalizedStringResource("Stack", comment: "Menu bar: the menu of what is done with the stack on screen.")) {
            Button(LocalizedStringResource("Stack", comment: "The Stack button: makes a render from the working version.")) {
                // The live map and the progress are in the viewer.
                model.setPanel(.closed)
                Task { await model.stack() }
            }
            .keyboardShortcut(.return)
            .disabled(!onStacking || !model.canStack)
            revertItem
            Button(RenderInfoView.title) {
                if let id = model.result?.renderID { model.present(.renderInfo(id)) }
            }
            .keyboardShortcut("i")
            .disabled(!onStacking || model.shownRender == nil || model.paint.active)
            Button(LocalizedStringResource("Export…", comment: "Toolbar button: opens the Export sheet for the result shown.")) {
                model.present(.export)
            }
            .keyboardShortcut("e")
            .disabled(!onStacking || !model.canExport || model.showExport)
            Divider()
            Button(LocalizedStringResource("Library", comment: "Menu bar item: goes back to the Library from a stack.")) {
                model.setScreen(.library)
            }
            .keyboardShortcut("l")
            .disabled(!onStacking || model.paint.active)
        }
        // In the system's View menu, with the desktop's keys: here, not on
        // the view bar's buttons, which a menu or narrow bar hides.
        CommandGroup(before: .toolbar) {
            viewCommands
        }
    }

    /// Stack ▸ Revert to #3: the change line's Revert. No key: it can lose
    /// work, so it asks first when it would.
    @ViewBuilder
    private var revertItem: some View {
        let showing = model.showing
        Button(ChangeLine.revertTitle(showing.revert?.number ?? showing.shown)) {
            guard let revert = showing.revert else { return }
            if revert.alert == nil {
                Task { await model.revert(to: revert.render) }
            } else {
                model.present(.revert(revert))
            }
        }
        .disabled(!onStacking || showing.revert == nil || model.busy || model.paint.active)
    }

    @ViewBuilder
    private var viewCommands: some View {
        let compare = model.compareState
        checked(CompareLayout.resultLabel, on: model.viewState.isResultAlone) {
            model.showResultAlone()
        }
        .keyboardShortcut("1", modifiers: [])
        .disabled(!canView || typing)
        ForEach(CompareLayout.Kind.allCases, id: \.self) { layout in
            // Flicker chosen again flips, as on the desktop.
            checked(layout.label, on: compare?.layout.kind == layout) {
                model.setLayout(layout)
            }
            .keyboardShortcut(layout.key, modifiers: [])
            .disabled(!canCompare || typing)
        }
        checked(ViewerDeck.compareLabel, on: compare != nil) {
            model.toggleCompare()
        }
        .disabled(!canCompare)
        Divider()
        // In Retouch, over the canvas, lit for the frame painted from.
        let paint = model.paint
        checked(ViewerDeck.depthMap, on: retouching ? paint.showsDepthMap : model.showsDepthMap) {
            if retouching { paint.toggleDepthMap() } else { model.toggleDepthMap() }
        }
        .keyboardShortcut("m", modifiers: [])
        .disabled(!(canView || retouching) || model.result?.map == nil || typing)
        // Not in Retouch, whose source shows the frame painted from as is.
        checked(ViewerDeck.contributionLabel, on: !retouching && model.contributionChosen) {
            model.toggleContribution()
        }
        .keyboardShortcut("c", modifiers: [])
        .disabled(retouching || !model.canShowContribution || !onStacking || typing)
        Divider()
        Button(ViewerDeck.zoomToFit) {
            model.fitRequest += 1
        }
        .keyboardShortcut("f", modifiers: [])
        .disabled(!onStacking || typing)
        // A title that changes, as the system's Show/Hide Toolbar. No key:
        // Tab moves the keyboard's focus.
        Button(model.panel.hidesControls ? Self.showControls : Self.hideControls) {
            withAnimation(.easeInOut(duration: 0.2)) {
                model.toggleControls(voiceOver: UIAccessibility.isVoiceOverRunning)
            }
        }
        .disabled(!onStacking || model.paint.active || model.panel.isOpen || UIAccessibility.isVoiceOverRunning)
        Divider()
        Button(Self.stepTitle(-1, renders: model.stepsRenders)) {
            Task { await model.paint.active ? model.stepPaintSource(by: -1) : model.step(by: -1) }
        }
        .keyboardShortcut(.leftArrow, modifiers: [])
        .disabled(!canStep || typing)
        Button(Self.stepTitle(1, renders: model.stepsRenders)) {
            Task { await model.paint.active ? model.stepPaintSource(by: 1) : model.step(by: 1) }
        }
        .keyboardShortcut(.rightArrow, modifiers: [])
        .disabled(!canStep || typing)
    }

    /// By frame, or by render while one is compared with.
    static func stepTitle(_ step: Int, renders: Bool) -> LocalizedStringResource {
        switch (step < 0, renders) {
        case (true, false):
            LocalizedStringResource("Previous Frame", comment: "Menu bar View item: the frame before (key ←).")
        case (false, false):
            LocalizedStringResource("Next Frame", comment: "Menu bar View item: the next frame (key →).")
        case (true, true):
            LocalizedStringResource(
                "Previous Render",
                comment: "Menu bar View item while comparing with a render: the render before in the Renders list (key ←).")
        case (false, true):
            LocalizedStringResource(
                "Next Render",
                comment: "Menu bar View item while comparing with a render: the next render in the Renders list (key →).")
        }
    }

    static let hideControls = LocalizedStringResource(
        "Hide Controls",
        comment: "Menu bar View item: hides every control over the photo. A tap on the photo also hides them and shows them again.")
    static let showControls = LocalizedStringResource(
        "Show Controls",
        comment: "Menu bar View item while the controls over the photo are hidden: shows them again.")

    /// A menu item checked while it is on.
    private func checked(
        _ title: LocalizedStringResource, on: Bool, action: @escaping () -> Void
    ) -> some View {
        Toggle(title, isOn: Binding(get: { on }, set: { _ in action() }))
    }

    private var canView: Bool {
        onStacking && !model.paint.active && model.stacking == nil && model.result?.display != nil
    }

    private var retouching: Bool {
        onStacking && model.paint.active
    }

    /// With a frame to compare it with.
    private var canCompare: Bool {
        canView && model.compareFrame != nil
    }

    /// In Retouch the arrows step what the brush paints from.
    private var canStep: Bool {
        onStacking && !model.rows.isEmpty
    }

    /// A key without a modifier is typing.
    private var typing: Bool { model.editingText }

    /// With nothing over it.
    private var onStacking: Bool {
        model.screen == .stacking && !model.showCamera && !model.showExport
    }
}
