//! The stacking parameters panel: the method and its sliders first (what a
//! stack is chosen by and compared on), then Alignment, one summary row
//! ("ECC (affine), cropped") opening on the aligner, auto-crop and, for
//! camera RAW with a choice installed, the decoder.
//!
//! A double-click on a slider's name and Reset to Defaults move the widgets
//! as a drag would, so the app sees the same change.
//!
//! The sliders and dropdowns are gpui-kit's uncontrolled widgets, read when
//! a stack starts and as the view renders (GPUI redraws on their change), so
//! no subscription mirrors them; only the switch is controlled, hence the
//! plain `auto_crop` field. The app restores the last stack's settings over
//! the engine's defaults at launch; the preferences supply the rest of a
//! stack's options as it starts.

use deptheos_app::frames::list::List;
use deptheos_app::library::stacks::Compared;
use deptheos_app::library::stacks::Settings;
use deptheos_app::{
    AlignMode, BLEND_RANGE, DENOISE_RANGE, DepthConfig, PyramidConfig, RADIUS_RANGE, RawBackend,
    SMOOTHING_RANGE, SMOOTHNESS_RANGE, SessionOptions, SettingHelp, StackerConfig, WaveletConfig,
};
use gpui::prelude::*;
use gpui::{App, Context, Entity, FontWeight, SharedString, Subscription, Window, div, px};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::select::Select;
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, h_flex, v_flex};

use crate::choice::{Choice, ChoiceSelect, choice_select, choices_of, chosen, set_choices};
use crate::files::FileList;
use crate::form::{Segment, Segmented, disclosure, dot};
use crate::prefs::{Preferences, Prefs, prefs};
use crate::r#type::TypeStyled as _;
use crate::words::{Wording as _, say};

pub use deptheos_app::StackerKind as Algorithm;

pub(crate) use deptheos_app::{
    BLEND_RANGE_STEP, DENOISE_STEP, RADIUS_STEP, SMOOTHING_STEP, SMOOTHNESS_STEP,
};

pub struct StackParams {
    algorithm: ChoiceSelect<Algorithm>,
    align: ChoiceSelect<AlignMode>,
    /// The installed backends, offered only when a choice
    /// ([`Self::offers_backend`]).
    raw_backend: ChoiceSelect<RawBackend>,
    /// More than LibRaw is installed (checked at startup and on preference
    /// changes).
    backend_choice: bool,
    /// The preferences last seen, to tell which default changed.
    seen: Preferences,
    _prefs: Subscription,
    /// The widgets, watched to say [`ParamsEvent::Changed`].
    _widgets: Vec<Subscription>,
    /// Read for whether its files include camera RAW.
    files: Entity<FileList>,
    smoothness: Entity<SliderState>,
    smoothing: Entity<SliderState>,
    radius: Entity<SliderState>,
    blend_range: Entity<SliderState>,
    denoise: Entity<SliderState>,
    /// The wavelet consistency filter: no widget (a command-line
    /// experiment), kept from a restored render so it stacks the same.
    consistency: u32,
    auto_crop: bool,
    /// Set while a stack runs: the parameters are fixed for its duration.
    disabled: bool,
    /// The line under the method, with Revert, from the app view.
    change: Option<ChangeLine>,
    /// The base's settings: a dot marks what changed since.
    base: Option<Settings>,
    /// The render shown's settings: a tick on a slider marks its value.
    shown: Option<Settings>,
    /// Advanced is open.
    advanced_open: bool,
    /// Alignment is open.
    alignment_open: bool,
}

/// One of the methods' sliders: what a reset sets back to its default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Knob {
    Smoothness,
    Smoothing,
    Radius,
    BlendRange,
    Denoise,
}

impl Knob {
    /// The sliders of `method`, in the order the panel shows them.
    pub(crate) fn of(method: Algorithm) -> &'static [Knob] {
        match method {
            Algorithm::Pyramid => &[Knob::Smoothness],
            Algorithm::Depth => &[Knob::Smoothing, Knob::Radius, Knob::BlendRange],
            Algorithm::Wavelet => &[Knob::Denoise],
        }
    }

    /// The application's default, from the method's config.
    pub(crate) fn default_value(self) -> f32 {
        match self {
            Knob::Smoothness => PyramidConfig::default().smoothness,
            Knob::Smoothing => DepthConfig::default().smoothing,
            Knob::Radius => DepthConfig::default().radius,
            Knob::BlendRange => DepthConfig::default().blend_range,
            Knob::Denoise => WaveletConfig::default().denoise,
        }
    }
}

/// The line under the method: how the working version differs from the
/// render shown (`Showing::change_line`), with Revert when offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeLine {
    pub text: SharedString,
    /// It counts changes since the base, shown: it carries the edited dot.
    pub edited: bool,
    /// Revert to this render is offered (`Showing::revert`).
    pub revert: Option<u32>,
    /// Retouch is open: Revert is there but off until it closes.
    pub retouching: bool,
}

/// What the settings panel asks of the app view.
pub enum ParamsEvent {
    /// Revert to the render shown, asking first if unstacked changes would go.
    Revert,
    /// A setting may have changed: by the user, or set by the app.
    Changed,
}

impl gpui::EventEmitter<ParamsEvent> for StackParams {}

impl StackParams {
    pub fn new(files: Entity<FileList>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let p = prefs(cx).clone();
        let pyramid = PyramidConfig::default();
        let depth = DepthConfig::default();
        let wavelet = WaveletConfig::default();
        let backends = installed_backends();
        let backend_choice = backends.len() > 1;
        let algorithm = choice_select(choices_of(cx.wording()), &Algorithm::default(), window, cx);
        let align = choice_select(choices_of(cx.wording()), &AlignMode::default(), window, cx);
        let raw_backend = choice_select(backends, &p.raw_backend.backend(), window, cx);
        let smoothness = slider(cx, SMOOTHNESS_RANGE, SMOOTHNESS_STEP, pyramid.smoothness);
        let smoothing = slider(cx, SMOOTHING_RANGE, SMOOTHING_STEP, depth.smoothing);
        let radius = slider(cx, RADIUS_RANGE, RADIUS_STEP, depth.radius);
        let blend_range = slider(cx, BLEND_RANGE, BLEND_RANGE_STEP, depth.blend_range);
        let denoise = slider(cx, DENOISE_RANGE, DENOISE_STEP, wavelet.denoise);
        let mut widgets = vec![
            cx.observe(&algorithm, |_, _, cx| cx.emit(ParamsEvent::Changed)),
            cx.observe(&align, |_, _, cx| cx.emit(ParamsEvent::Changed)),
            cx.observe(&raw_backend, |_, _, cx| cx.emit(ParamsEvent::Changed)),
        ];
        for slider in [&smoothness, &smoothing, &radius, &blend_range, &denoise] {
            widgets.push(cx.observe(slider, |_, _, cx| cx.emit(ParamsEvent::Changed)));
        }
        Self {
            algorithm,
            align,
            raw_backend,
            backend_choice,
            files,
            smoothness,
            smoothing,
            radius,
            blend_range,
            denoise,
            _widgets: widgets,
            consistency: wavelet.consistency,
            auto_crop: SessionOptions::default().alignment.crops(),
            disabled: false,
            change: None,
            base: None,
            shown: None,
            advanced_open: false,
            alignment_open: false,
            seen: p,
            _prefs: cx.observe_global_in::<Prefs>(window, Self::on_prefs_changed),
        }
    }

    /// Follow the preferences: offer Adobe once the converter is found, and
    /// apply a changed default decoder.
    fn on_prefs_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = prefs(cx).clone();
        let prev = std::mem::replace(&mut self.seen, next.clone());
        let backends = installed_backends();
        let had_choice = std::mem::replace(&mut self.backend_choice, backends.len() > 1);
        set_choices(&self.raw_backend, backends, window, cx);
        // Without the converter the picker was hidden on LibRaw; once found,
        // the default decoder applies as at startup.
        let found = self.backend_choice && !had_choice;
        if next.raw_backend != prev.raw_backend || found {
            let backend = next.raw_backend.backend();
            self.raw_backend.update(cx, |s, cx| s.set_selected_value(&backend, window, cx));
        }
        cx.notify();
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.disabled = disabled;
        cx.notify();
    }

    /// What the settings are compared with: the change line, the base's
    /// settings (dots) and the render shown's (ticks). Set as the app draws.
    pub fn set_compared(
        &mut self,
        change: Option<ChangeLine>,
        base: Option<Settings>,
        shown: Option<Settings>,
    ) {
        self.change = change;
        self.base = base;
        self.shown = shown;
    }

    fn algorithm(&self, cx: &App) -> Algorithm {
        chosen(&self.algorithm, cx).unwrap_or(Algorithm::Pyramid)
    }

    /// The picker shows only with more than LibRaw installed and camera RAW
    /// (not DNG) in the list.
    fn offers_backend(&self, cx: &App) -> bool {
        self.backend_choice && self.files.read(cx).has_camera_raw()
    }

    /// The chosen backend, LibRaw while the picker is hidden.
    fn raw_backend(&self, cx: &App) -> RawBackend {
        if self.offers_backend(cx) {
            chosen(&self.raw_backend, cx).unwrap_or_default()
        } else {
            RawBackend::LibRaw
        }
    }

    /// A batch's default camera RAW decoder: the sidebar's choice while it
    /// shows one, else the preferred decoder.
    pub fn batch_backend(&self, cx: &App) -> RawBackend {
        if self.offers_backend(cx) {
            self.raw_backend(cx)
        } else if self.backend_choice {
            prefs(cx).raw_backend.backend()
        } else {
            RawBackend::LibRaw
        }
    }

    /// The Adobe converter is installed.
    pub fn has_backend_choice(&self) -> bool {
        self.backend_choice
    }

    /// Nothing can be changed: a stack runs.
    fn locked(&self) -> bool {
        self.disabled
    }

    fn knob(&self, knob: Knob) -> &Entity<SliderState> {
        match knob {
            Knob::Smoothness => &self.smoothness,
            Knob::Smoothing => &self.smoothing,
            Knob::Radius => &self.radius,
            Knob::BlendRange => &self.blend_range,
            Knob::Denoise => &self.denoise,
        }
    }

    /// `knob` is at its default, to within half a step.
    fn is_default(&self, knob: Knob, cx: &App) -> bool {
        let state = self.knob(knob).read(cx);
        (state.value().start() - knob.default_value()).abs() < state.step_value() / 2.0
    }

    /// Every slider of the method shown is at its default.
    pub(crate) fn at_defaults(&self, cx: &App) -> bool {
        Knob::of(self.algorithm(cx)).iter().all(|&k| self.is_default(k, cx))
    }

    /// Set `knob` to its default, as a drag would (not while locked).
    pub(crate) fn reset(&mut self, knob: Knob, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked() || self.is_default(knob, cx) {
            return;
        }
        tracing::debug!("settings: {knob:?} reset to its default");
        set_slider(self.knob(knob), knob.default_value(), window, cx);
        cx.notify();
    }

    /// Reset to Defaults: every slider of the method, Advanced's too; the
    /// method, aligner, crop and decoder stay.
    pub(crate) fn reset_method(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for &knob in Knob::of(self.algorithm(cx)) {
            self.reset(knob, window, cx);
        }
    }

    /// The stacker configuration the widgets describe.
    pub fn stacker_config(&self, cx: &App) -> StackerConfig {
        match self.algorithm(cx) {
            Algorithm::Pyramid => {
                StackerConfig::Pyramid(PyramidConfig { smoothness: value(&self.smoothness, cx) })
            }
            Algorithm::Depth => StackerConfig::Depth(DepthConfig {
                radius: value(&self.radius, cx),
                smoothing: value(&self.smoothing, cx),
                blend_range: value(&self.blend_range, cx),
            }),
            Algorithm::Wavelet => StackerConfig::Wavelet(WaveletConfig {
                denoise: value(&self.denoise, cx),
                consistency: self.consistency,
            }),
        }
    }

    /// Set the widgets to saved settings (a render's, a failed stack's, or
    /// the list's). Only the panel's are restored: the GUI always aligns,
    /// preference-set options stay, and an uninstalled backend leaves the
    /// picker as it was.
    pub fn restore(
        &mut self,
        config: &StackerConfig,
        options: &SessionOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The hidden wavelet value is the config's or the default, never a
        // previous restore's.
        self.consistency = match config {
            StackerConfig::Wavelet(c) => c.consistency,
            StackerConfig::Pyramid(_) | StackerConfig::Depth(_) => {
                WaveletConfig::default().consistency
            }
        };
        let algorithm = match config {
            StackerConfig::Pyramid(c) => {
                set_slider(&self.smoothness, c.smoothness, window, cx);
                Algorithm::Pyramid
            }
            StackerConfig::Depth(c) => {
                set_slider(&self.radius, c.radius, window, cx);
                set_slider(&self.smoothing, c.smoothing, window, cx);
                set_slider(&self.blend_range, c.blend_range, window, cx);
                Algorithm::Depth
            }
            StackerConfig::Wavelet(c) => {
                set_slider(&self.denoise, c.denoise, window, cx);
                Algorithm::Wavelet
            }
        };
        self.algorithm.update(cx, |s, cx| s.set_selected_value(&algorithm, window, cx));
        // An unaligned render (iOS or CLI) gives the aligner and crop it
        // would resume with.
        let alignment = options.alignment.settings();
        self.align.update(cx, |s, cx| s.set_selected_value(&alignment.mode, window, cx));
        self.auto_crop = alignment.crop;
        self.raw_backend.update(cx, |s, cx| s.set_selected_value(&options.raw_backend, window, cx));
        cx.notify();
    }

    /// The session options the widgets and the preferences describe.
    pub fn session_options(&self, cx: &App) -> SessionOptions {
        let prefs = prefs(cx);
        SessionOptions {
            alignment: prefs.alignment(chosen(&self.align, cx).unwrap_or_default(), self.auto_crop),
            raw_backend: self.raw_backend(cx),
            // Recorded in the render, so its list keeps this order.
            order: self.files.read(cx).order(),
            ..prefs.session_options()
        }
    }
}

impl Render for StackParams {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sections =
            [self.method_section(cx).pt_0(), self.sliders_section(cx), self.alignment_section(cx)];
        v_flex().children(sections)
    }
}

impl StackParams {
    /// The method picker, with a line on what it is good for (all of it in
    /// the tooltip).
    fn method_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let disabled = self.locked();
        let algorithm = self.algorithm(cx);
        let config = self.stacker_config(cx);
        let method_changed = self.base.is_some_and(|b| b.config.kind() != config.kind());
        let weak = cx.entity().downgrade();
        let methods = Segmented::new("method")
            .full_width()
            .children(Algorithm::ALL.iter().map(|a| {
                let about = a.description(crate::words::of(cx));
                Segment::new(a.label(crate::words::of(cx)))
                    .disabled(disabled)
                    .tooltip(move |window, cx| sentence_tip(about.clone(), window, cx))
            }))
            .selected_index(Algorithm::ALL.iter().position(|a| *a == algorithm).unwrap_or(0))
            .on_click(move |ix, window, cx| {
                let Some(&picked) = Algorithm::ALL.get(*ix) else { return };
                weak.update(cx, |this, cx| {
                    if !this.locked() {
                        this.algorithm
                            .update(cx, |s, cx| s.set_selected_value(&picked, window, cx));
                        cx.notify();
                    }
                })
                .ok();
            });
        let method_note = match algorithm {
            Algorithm::Pyramid => say!(cx, "settings-method-pyramid-note"),
            Algorithm::Depth => say!(cx, "settings-method-depth-note"),
            Algorithm::Wavelet => say!(cx, "settings-method-wavelet-note"),
        };
        let heading = h_flex()
            .gap_1p5()
            .items_center()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .child(say!(cx, "settings-method"))
            .when(method_changed, |el| el.child(dot(edited_color(cx))));
        section(cx)
            .child(heading)
            .child(div().debug_selector(|| "settings-method".into()).child(methods))
            .child(div().text_xs().text_color(cx.theme().muted_foreground).child(method_note))
    }

    /// The method's sliders and Reset to Defaults, under the change line.
    /// That line sits under the picker so its appearing shifts no control
    /// above it.
    fn sliders_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let disabled = self.locked();
        let algorithm = self.algorithm(cx);
        let config = self.stacker_config(cx);
        // The base's settings for the dots (per value only for the same
        // method) and the render shown's for the ticks.
        let values = |settings: Option<Settings>| {
            let config = settings.map(|s| s.config).filter(|c| c.kind() == config.kind());
            let main = config.map(|c| match c {
                StackerConfig::Pyramid(p) => p.smoothness,
                StackerConfig::Depth(d) => d.smoothing,
                StackerConfig::Wavelet(w) => w.denoise,
            });
            let depth = match config {
                Some(StackerConfig::Depth(d)) => Some(d),
                _ => None,
            };
            Marks { main, radius: depth.map(|d| d.radius), blend: depth.map(|d| d.blend_range) }
        };
        let (was, tick) = (values(self.base), values(self.shown));
        let main = match algorithm {
            Algorithm::Pyramid => SliderSpec {
                label: say!(cx, "settings-smoothness"),
                knob: Knob::Smoothness,
                range: SMOOTHNESS_RANGE,
                marks: (was.main, tick.main),
                ends: (say!(cx, "settings-crisp"), say!(cx, "settings-calm")),
                help: SettingHelp::Smoothness,
            },
            Algorithm::Depth => SliderSpec {
                label: say!(cx, "settings-smoothing"),
                knob: Knob::Smoothing,
                range: SMOOTHING_RANGE,
                marks: (was.main, tick.main),
                ends: (say!(cx, "settings-narrow-outlines"), say!(cx, "settings-even-backgrounds")),
                help: SettingHelp::Smoothing,
            },
            Algorithm::Wavelet => SliderSpec {
                label: say!(cx, "settings-denoise"),
                knob: Knob::Denoise,
                range: DENOISE_RANGE,
                marks: (was.main, tick.main),
                ends: (say!(cx, "settings-all-detail"), say!(cx, "settings-less-noise")),
                help: SettingHelp::Denoise,
            },
        };
        let main = self.setting_slider(main, cx);
        // Depth's Radius is used as often as Smoothing, so it sits beside it;
        // only Blend Range is under Advanced.
        let radius = (algorithm == Algorithm::Depth).then(|| {
            let spec = SliderSpec {
                label: say!(cx, "settings-radius"),
                knob: Knob::Radius,
                range: RADIUS_RANGE,
                marks: (was.radius, tick.radius),
                ends: (say!(cx, "settings-thin-detail"), say!(cx, "settings-steadier")),
                help: SettingHelp::Radius,
            };
            self.setting_slider(spec, cx)
        });
        let advanced = (algorithm == Algorithm::Depth).then(|| {
            let spec = SliderSpec {
                label: say!(cx, "settings-blend-range"),
                knob: Knob::BlendRange,
                range: BLEND_RANGE,
                marks: (was.blend, tick.blend),
                ends: (say!(cx, "settings-one-frame"), say!(cx, "settings-mixed")),
                help: SettingHelp::BlendRange,
            };
            let blend = self.setting_slider(spec, cx);
            let open = self.advanced_open;
            let row = disclosure(
                "advanced",
                say!(cx, "settings-advanced"),
                say!(cx, "settings-advanced-hidden"),
                open,
                None,
                cx.listener(|this, _, _, cx| {
                    this.advanced_open = !this.advanced_open;
                    cx.notify();
                }),
                cx,
            );
            v_flex().gap_2().child(row).when(open, |el| el.child(blend))
        });
        let method_label = algorithm.label(crate::words::of(cx));
        let reset = h_flex().justify_end().child(
            Button::new("reset-defaults")
                .label(say!(cx, "settings-reset-defaults"))
                .xsmall()
                .ghost()
                .disabled(disabled || self.at_defaults(cx))
                .tooltip(say!(cx, "settings-reset-defaults-tooltip", method = method_label))
                .on_click(cx.listener(|this, _, window, cx| this.reset_method(window, cx))),
        );
        let change = self.render_change_line(edited_color(cx), cx);
        section(cx).children(change).child(main).children(radius).children(advanced).child(reset)
    }

    /// Alignment and decoding: set seldom, so one summary row that opens on
    /// the controls.
    fn alignment_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let disabled = self.locked();
        let options = self.session_options(cx);
        let base = self.base;
        let edited = edited_color(cx);
        let align_changed =
            base.is_some_and(|b| b.options.alignment.mode() != options.alignment.mode());
        let crop_changed =
            base.is_some_and(|b| b.options.alignment.crops() != options.alignment.crops());
        let offers_backend = self.offers_backend(cx);
        let backend_changed =
            offers_backend && base.is_some_and(|b| b.options.raw_backend != options.raw_backend);
        let decoder = offers_backend.then(|| {
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .text_xs()
                        .child(say!(cx, "settings-decoder"))
                        .when(backend_changed, |el| el.child(dot(edited))),
                )
                .child(Select::new(&self.raw_backend).small().disabled(disabled))
                .child(help(SettingHelp::Decoder, cx))
        });
        let crop_help = SettingHelp::Crop.help(crate::words::of(cx));
        let open = self.alignment_open;
        let mode = options.alignment.mode().unwrap_or_default();
        let aligner = deptheos_app::align_label(mode, crate::words::of(cx));
        let summary = match options.alignment.crops() {
            true => say!(cx, "settings-alignment-cropped", aligner = aligner),
            false => say!(cx, "settings-alignment-uncropped", aligner = aligner),
        };
        let row = disclosure(
            "alignment",
            say!(cx, "settings-alignment"),
            summary,
            open,
            (align_changed || crop_changed || backend_changed).then_some(edited),
            cx.listener(|this, _, _, cx| {
                this.alignment_open = !this.alignment_open;
                cx.notify();
            }),
            cx,
        )
        .debug_selector(|| "settings-alignment".into());
        section(cx).child(row).when(open, |el| {
            el.child(Select::new(&self.align).small().disabled(disabled))
                .child(help(SettingHelp::Aligner, cx))
                .child(
                    h_flex()
                        .id("crop-row")
                        .tooltip(move |window, cx| sentence_tip(crop_help.clone(), window, cx))
                        .gap_2()
                        .justify_between()
                        .child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .text_xs()
                                .child(say!(cx, "settings-crop"))
                                .when(crop_changed, |el| el.child(dot(edited))),
                        )
                        .child(
                            Switch::new("auto-crop")
                                .checked(self.auto_crop)
                                .small()
                                .disabled(disabled)
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.auto_crop = *checked;
                                    cx.emit(ParamsEvent::Changed);
                                    cx.notify();
                                })),
                        ),
                )
                .children(decoder)
        })
    }

    /// A setting's slider: label (dotted when changed since the base),
    /// value, a tick at the render shown's value, end words, and tooltips.
    /// Unless locked, a double-click on the label row resets it (the kit's
    /// slider has no handler, and a track click moves the thumb).
    fn setting_slider(&self, spec: SliderSpec, cx: &mut Context<Self>) -> gpui::Div {
        let SliderSpec { label, knob, range: (min, max), marks: (old, tick), ends, help } = spec;
        let (low, high) = ends;
        let disabled = self.locked();
        let (edited, muted) = (edited_color(cx), cx.theme().muted_foreground);
        let state = self.knob(knob);
        let v = value(state, cx);
        let step = state.read(cx).step_value();
        let shown = slider_text(v, step);
        let differs = |o: &f32| (o - v).abs() > f32::EPSILON;
        let changed = old.is_some_and(|o| differs(&o));
        let tick = tick.filter(differs);
        let reset = (!disabled).then(|| Reset {
            tooltip: say!(
                cx,
                "settings-slider-reset-tooltip",
                value = slider_text(knob.default_value(), step)
            ),
            params: cx.entity().downgrade(),
        });
        let help = help.help(crate::words::of(cx));
        let name = format!("slider-name-{knob:?}");
        let track = format!("slider-track-{knob:?}");
        v_flex()
            .gap_0p5()
            .child(
                h_flex()
                    .id(gpui::ElementId::Name(name.clone().into()))
                    .debug_selector(move || name.clone())
                    .justify_between()
                    .text_xs()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(label)
                            .when(changed, |el| el.child(dot(edited))),
                    )
                    .child(div().tabular().child(shown))
                    .when_some(reset, |el, Reset { tooltip, params }| {
                        el.tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tooltip.clone())
                                .build(window, cx)
                        })
                        .on_click(move |event, window, cx| {
                            if event.click_count() == 2 {
                                params.update(cx, |p, cx| p.reset(knob, window, cx)).ok();
                            }
                        })
                    }),
            )
            .child(
                v_flex()
                    .id(gpui::ElementId::Name(track.into()))
                    .tooltip(move |window, cx| sentence_tip(help.clone(), window, cx))
                    .gap_0p5()
                    .child(Slider::new(state).disabled(disabled))
                    .when_some(tick, |el, tick| {
                        let at = ((tick - min) / (max - min)).clamp(0.0, 1.0);
                        el.child(
                            div().relative().h(px(5.0)).mx(px(8.0)).child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left(gpui::relative(at))
                                    .w(px(2.0))
                                    .h_full()
                                    .rounded_full()
                                    .bg(edited),
                            ),
                        )
                    })
                    .child(
                        h_flex()
                            .justify_between()
                            .text_xs()
                            .text_color(muted)
                            .child(low)
                            .child(high),
                    ),
            )
    }
}

/// One of the inspector's sections, ruled off from the next.
fn section(cx: &App) -> gpui::Div {
    v_flex().gap_2().py_3().border_b_1().border_color(cx.theme().border)
}

/// What moving a setting does, under it.
fn help(setting: SettingHelp, cx: &App) -> gpui::Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(setting.help(crate::words::of(cx)))
}

/// A slider for [`StackParams::setting_slider`]: name, widget, range, the
/// base's and render shown's values (`marks`), end words and tooltip.
struct SliderSpec {
    label: String,
    knob: Knob,
    range: (f32, f32),
    marks: (Option<f32>, Option<f32>),
    ends: (String, String),
    help: SettingHelp,
}

impl StackParams {
    /// "1 change to the settings since #4" with the edited dot, or
    /// "1 difference from #3 in the settings" with Revert; nothing when the
    /// working version is the render shown's.
    fn render_change_line(
        &self,
        edited: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let ChangeLine { text, edited: marked, revert, retouching } = self.change.clone()?;
        Some(
            h_flex()
                .debug_selector(|| "change-line".into())
                .gap_1p5()
                .items_center()
                .text_xs()
                .when(marked, |el| el.child(dot(edited)))
                .child(div().flex_1().child(text))
                .when_some(revert, |el, number| {
                    el.child(
                        Button::new("revert")
                            .label(say!(cx, "settings-revert"))
                            .xsmall()
                            .ghost()
                            .disabled(self.disabled || retouching)
                            .tooltip(say!(cx, "settings-revert-tooltip", number = number))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ParamsEvent::Revert))),
                    )
                })
                .into_any_element(),
        )
    }
}

/// Each slider's value in one render's settings; `None` for another method.
#[derive(Clone, Copy)]
struct Marks {
    main: Option<f32>,
    radius: Option<f32>,
    blend: Option<f32>,
}

/// The edited dot's color in the current mode.
pub(crate) fn edited_color(cx: &App) -> gpui::Hsla {
    let [light, dark] = crate::color::brand().edited;
    gpui::rgb(if cx.theme().mode.is_dark() { dark } else { light }).into()
}

/// A slider's label value with its step's decimals, so "2.0" doesn't
/// shorten to "2" and the label holds still.
fn slider_text(value: f32, step: f32) -> String {
    if step >= 1.0 { format!("{value:.0}") } else { format!("{value:.1}") }
}

/// Double-clicking a slider's name resets it; `tooltip` says so.
struct Reset {
    tooltip: String,
    params: gpui::WeakEntity<StackParams>,
}

/// A sentence-long tooltip wrapped at about the inspector's width (the
/// kit's runs one line).
fn sentence_tip(text: String, window: &mut Window, cx: &mut App) -> gpui::AnyView {
    gpui_kit::component::tooltip::Tooltip::element(move |_, _| {
        div().max_w(px(260.0)).child(text.clone())
    })
    .build(window, cx)
}

/// What counts as a change to `list`'s working version ([`Compared`]): the
/// decoder too when its picker shows (`decoder_choice` and camera RAW).
pub(crate) fn compared_settings(list: &List, decoder_choice: bool) -> Compared {
    let offered = decoder_choice && list.files.iter().any(|f| crate::files::is_convertible_raw(f));
    Compared::desktop(offered)
}

/// The RAW backends that can run here, as picker entries.
pub(crate) fn installed_backends() -> Vec<Choice<RawBackend>> {
    RawBackend::ALL
        .iter()
        .filter(|b| b.is_available())
        .map(|&b| Choice::new(b.label(), b))
        .collect()
}

/// The one place slider ranges and steps are built: a `step` of 0 makes
/// gpui-base's snapping compute `(v / 0) * 0 = NaN`.
fn slider(cx: &mut App, (min, max): (f32, f32), step: f32, value: f32) -> Entity<SliderState> {
    debug_assert!(step > 0.0, "slider step must be positive");
    cx.new(|_| SliderState::new().min(min).max(max).step(step).default_value(value))
}

fn value(state: &Entity<SliderState>, cx: &App) -> f32 {
    state.read(cx).value().start()
}

fn set_slider(state: &Entity<SliderState>, value: f32, window: &mut Window, cx: &mut App) {
    state.update(cx, |s, cx| s.set_value(value, window, cx));
}

/// A label, its value and the slider, with a hover tooltip (the paint panel's).
pub(crate) fn labeled_slider(
    id: &'static str,
    label: String,
    value: String,
    tooltip: String,
    state: &Entity<SliderState>,
) -> impl IntoElement + use<> {
    v_flex()
        .id(id)
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
        })
        .gap_0p5()
        .child(
            h_flex().justify_between().text_xs().child(label).child(div().tabular().child(value)),
        )
        .child(Slider::new(state))
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;
    use crate::test_support::open;

    #[test]
    fn slider_values_keep_their_decimals() {
        assert_eq!(slider_text(2.0, 0.1), "2.0");
        assert_eq!(slider_text(2.46, 0.1), "2.5");
        assert_eq!(slider_text(8.0, 1.0), "8");
    }

    /// Grid search, not cropped.
    fn grid_uncropped() -> SessionOptions {
        let alignment = deptheos_app::Alignment::On(deptheos_app::AlignSettings {
            mode: AlignMode::Grid,
            crop: false,
            ..Default::default()
        });
        SessionOptions { alignment, ..Default::default() }
    }

    fn new_params(window: &mut Window, cx: &mut App) -> Entity<StackParams> {
        let files = cx.new(|cx| FileList::new(crate::test_support::frames(), window, cx));
        cx.new(|cx| StackParams::new(files, window, cx))
    }

    #[gpui::test]
    fn config_follows_the_widgets(cx: &mut TestAppContext) {
        let (window, params) = open(cx, new_params);
        params.read_with(cx, |p, cx| {
            let StackerConfig::Pyramid(config) = p.stacker_config(cx) else {
                panic!("pyramid by default")
            };
            assert_eq!(config.smoothness, PyramidConfig::default().smoothness);
            let options = p.session_options(cx);
            assert_eq!(options.alignment.mode(), Some(AlignMode::default()));
            assert!(options.alignment.crops());
        });

        cx.update_window(window, |_, window, cx| {
            params.update(cx, |p, cx| {
                p.algorithm.update(cx, |s, cx| s.set_selected_value(&Algorithm::Depth, window, cx));
                p.radius.update(cx, |s, cx| s.set_value(12.0, window, cx));
                p.align.update(cx, |s, cx| s.set_selected_value(&AlignMode::Grid, window, cx));
            })
        })
        .unwrap();
        params.read_with(cx, |p, cx| {
            let StackerConfig::Depth(config) = p.stacker_config(cx) else {
                panic!("depth once chosen")
            };
            assert_eq!(config.radius, 12.0);
            assert_eq!(p.session_options(cx).alignment.mode(), Some(AlignMode::Grid));
        });
    }

    #[gpui::test]
    fn restore_sets_every_widget(cx: &mut TestAppContext) {
        let (window, params) = open(cx, new_params);
        let depth =
            StackerConfig::Depth(DepthConfig { radius: 14.0, smoothing: 2.5, blend_range: 7.5 });
        let options = grid_uncropped();
        cx.update_window(window, |_, window, cx| {
            params.update(cx, |p, cx| p.restore(&depth, &options, window, cx))
        })
        .unwrap();
        params.read_with(cx, |p, cx| {
            let StackerConfig::Depth(c) = p.stacker_config(cx) else { panic!("depth restored") };
            assert_eq!((c.radius, c.smoothing, c.blend_range), (14.0, 2.5, 7.5));
            let restored = p.session_options(cx).alignment;
            assert_eq!((restored.mode(), restored.crops()), (Some(AlignMode::Grid), false));
        });

        let pyramid = StackerConfig::Pyramid(PyramidConfig { smoothness: 6.5 });
        cx.update_window(window, |_, window, cx| {
            params.update(cx, |p, cx| p.restore(&pyramid, &SessionOptions::default(), window, cx))
        })
        .unwrap();
        params.read_with(cx, |p, cx| {
            let StackerConfig::Pyramid(c) = p.stacker_config(cx) else {
                panic!("pyramid restored")
            };
            assert_eq!(c.smoothness, 6.5);
            assert!(p.session_options(cx).alignment.crops());
        });

        let wavelet = StackerConfig::Wavelet(WaveletConfig { denoise: 2.5, consistency: 2 });
        cx.update_window(window, |_, window, cx| {
            params.update(cx, |p, cx| p.restore(&wavelet, &SessionOptions::default(), window, cx))
        })
        .unwrap();
        params.read_with(cx, |p, cx| {
            let StackerConfig::Wavelet(c) = p.stacker_config(cx) else {
                panic!("wavelet restored")
            };
            assert_eq!((c.denoise, c.consistency), (2.5, 2));
        });

        // Another method's config resets the hidden value, so a later wavelet
        // stack doesn't inherit the 2.
        let pyramid = StackerConfig::Pyramid(PyramidConfig { smoothness: 4.0 });
        cx.update_window(window, |_, window, cx| {
            params.update(cx, |p, cx| p.restore(&pyramid, &SessionOptions::default(), window, cx))
        })
        .unwrap();
        params.read_with(cx, |p, _| {
            assert_eq!(p.consistency, WaveletConfig::default().consistency);
        });
    }

    /// A stack's options take grid limits, camera profile and reverse check
    /// from the preferences.
    #[gpui::test]
    fn follows_the_preferences(cx: &mut TestAppContext) {
        use crate::prefs::{Preferences, Prefs};
        use deptheos_app::DcpPreference;

        let (_, params) = open(cx, |window, cx| {
            let custom = Preferences {
                alignment: deptheos_app::preferences::AlignmentPreferences {
                    grid_rotation_deg: 2.0,
                    exposure_compensation: true,
                    ..Default::default()
                },
                camera_profile: DcpPreference::None,
                ..Preferences::default()
            };
            Prefs::install(custom, None, cx);
            new_params(window, cx)
        });
        params.read_with(cx, |p, cx| {
            let options = p.session_options(cx);
            let grid = options.alignment.settings().grid;
            assert_eq!(grid.rotation_deg, 2.0);
            assert!(grid.apply_brightness);
            assert_eq!(options.camera_profile, DcpPreference::None);
        });
    }

    /// Depth with every slider and frame setting off its default.
    fn moved(cx: &mut TestAppContext, window: gpui::AnyWindowHandle, params: &Entity<StackParams>) {
        let depth =
            StackerConfig::Depth(DepthConfig { radius: 14.0, smoothing: 2.5, blend_range: 7.5 });
        let options = grid_uncropped();
        cx.update_window(window, |_, window, cx| {
            params.update(cx, |p, cx| p.restore(&depth, &options, window, cx))
        })
        .unwrap();
    }

    /// Depth's radius, smoothing and blend range as the widgets hold them.
    fn depth(params: &Entity<StackParams>, cx: &mut TestAppContext) -> (f32, f32, f32) {
        params.read_with(cx, |p, cx| match p.stacker_config(cx) {
            StackerConfig::Depth(c) => (c.radius, c.smoothing, c.blend_range),
            other => panic!("depth stays chosen, not {other:?}"),
        })
    }

    fn depth_defaults() -> (f32, f32, f32) {
        let d = DepthConfig::default();
        (d.radius, d.smoothing, d.blend_range)
    }

    fn reset(
        cx: &mut TestAppContext,
        window: gpui::AnyWindowHandle,
        params: &Entity<StackParams>,
        knob: Option<Knob>,
    ) {
        cx.update_window(window, |_, window, cx| {
            params.update(cx, |p, cx| match knob {
                Some(knob) => p.reset(knob, window, cx),
                None => p.reset_method(window, cx),
            })
        })
        .unwrap();
    }

    /// A reset acts as a drag: the panel says Changed, marking the working
    /// version edited.
    #[gpui::test]
    fn reset_puts_one_slider_back(cx: &mut TestAppContext) {
        let (window, params) = open(cx, new_params);
        moved(cx, window, &params);
        let changes = std::rc::Rc::new(std::cell::Cell::new(0));
        let _watch = cx.update(|cx| {
            let changes = changes.clone();
            cx.subscribe(&params, move |_, event: &ParamsEvent, _| {
                if matches!(event, ParamsEvent::Changed) {
                    changes.set(changes.get() + 1);
                }
            })
        });
        reset(cx, window, &params, Some(Knob::Radius));
        cx.run_until_parked();
        assert_eq!(depth(&params, cx), (DepthConfig::default().radius, 2.5, 7.5));
        assert!(changes.get() > 0, "the reset says Changed");
        params.read_with(cx, |p, cx| assert!(!p.at_defaults(cx), "the others are still moved"));
    }

    /// Reset to Defaults: every slider of the method, Advanced's too, nothing else.
    #[gpui::test]
    fn reset_to_defaults_puts_the_method_back(cx: &mut TestAppContext) {
        let (window, params) = open(cx, new_params);
        moved(cx, window, &params);
        params.read_with(cx, |p, cx| assert!(!p.at_defaults(cx)));
        reset(cx, window, &params, None);
        assert_eq!(depth(&params, cx), depth_defaults());
        params.read_with(cx, |p, cx| {
            assert!(p.at_defaults(cx));
            let options = p.session_options(cx).alignment;
            let kept = (options.mode(), options.crops());
            assert_eq!(kept, (Some(AlignMode::Grid), false), "kept");
        });
    }

    /// Nothing resets while a stack runs.
    #[gpui::test]
    fn reset_waits_while_locked(cx: &mut TestAppContext) {
        let (window, params) = open(cx, new_params);
        moved(cx, window, &params);
        let before = depth(&params, cx);
        params.update(cx, |p, cx| p.set_disabled(true, cx));
        reset(cx, window, &params, Some(Knob::Smoothing));
        reset(cx, window, &params, None);
        assert_eq!(depth(&params, cx), before);
    }

    /// Double-clicking a slider's name resets it; a single click doesn't.
    #[gpui::test]
    fn double_clicking_a_name_resets_it(cx: &mut TestAppContext) {
        use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent};
        let (window, params) = open(cx, new_params);
        moved(cx, window, &params);
        let mut vcx = gpui::VisualTestContext::from_window(window, cx);
        vcx.run_until_parked();
        let name = vcx.debug_bounds("slider-name-Radius").expect("Radius shows beside Smoothing");
        let click = |vcx: &mut gpui::VisualTestContext, count: usize| {
            let (position, button, modifiers) =
                (name.center(), MouseButton::Left, Modifiers::none());
            vcx.simulate_event(MouseDownEvent {
                position,
                modifiers,
                button,
                click_count: count,
                first_mouse: false,
            });
            vcx.simulate_event(MouseUpEvent { position, modifiers, button, click_count: count });
        };
        click(&mut vcx, 1);
        assert_eq!(depth(&params, &mut vcx).0, 14.0, "one click leaves it");
        click(&mut vcx, 2);
        assert_eq!(
            depth(&params, &mut vcx),
            (DepthConfig::default().radius, 2.5, 7.5),
            "only Radius"
        );
    }

    /// A batch's decoder doesn't follow the list: without camera RAW there,
    /// it is the preferred one (Automatic: Adobe when installed), not the
    /// hidden picker's LibRaw.
    #[gpui::test]
    fn a_batch_decodes_with_the_preferred_backend(cx: &mut TestAppContext) {
        let (window, params) = open(cx, new_params);
        let libraw = SessionOptions { raw_backend: RawBackend::LibRaw, ..Default::default() };
        let pyramid = StackerConfig::Pyramid(PyramidConfig::default());
        cx.update_window(window, |_, window, cx| {
            params.update(cx, |p, cx| {
                // As if the converter were installed.
                p.backend_choice = true;
                p.restore(&pyramid, &libraw, window, cx)
            })
        })
        .unwrap();
        params.read_with(cx, |p, cx| {
            assert!(!p.offers_backend(cx));
            assert_eq!(p.batch_backend(cx), prefs(cx).raw_backend.backend());
        });
        params.update(cx, |p, _| p.backend_choice = false);
        params.read_with(cx, |p, cx| assert_eq!(p.batch_backend(cx), RawBackend::LibRaw));
    }

    /// A restored Adobe render's backend applies only while the picker is
    /// offered.
    #[gpui::test]
    fn backend_is_libraw_without_camera_raw(cx: &mut TestAppContext) {
        let (window, params) = open(cx, new_params);
        let adobe = SessionOptions { raw_backend: RawBackend::AdobeDng, ..Default::default() };
        let pyramid = StackerConfig::Pyramid(PyramidConfig::default());
        cx.update_window(window, |_, window, cx| {
            params.update(cx, |p, cx| p.restore(&pyramid, &adobe, window, cx))
        })
        .unwrap();
        params.read_with(cx, |p, cx| {
            assert!(!p.offers_backend(cx));
            assert_eq!(p.session_options(cx).raw_backend, RawBackend::LibRaw);
        });
    }
}
