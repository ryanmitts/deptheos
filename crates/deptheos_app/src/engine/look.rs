//! What a draw turns a stack's stored pixels into: the stack's color, the
//! render's adjustments and the output space, as one value every draw
//! takes. Source frames, the depth map, Retouch and aligned frames show the
//! stored picture, through [`Look::stored`] and [`Look::exported`] with no
//! adjustments, so a draw with none says so.

use deptheos_core::io;
use deptheos_dng::{RenderParams, Temperature, WhiteXy};
use deptheos_render::dng_render::Constants;
use deptheos_render::{Adjustments, Color, Draw, OutputSpace, Reframe, WhiteBalance, resolve};
use deptheos_render::{resolve_adjusted, slider};

use crate::engine::present::display_space;
use crate::library::adjustments::RenderAdjustments;
use crate::runs::session::{CompletedStack, StackColor};

#[derive(Clone, Copy)]
pub struct Look<'a> {
    stack: &'a CompletedStack,
    adjusted: Adjusted<'a>,
    /// The window drawn, applied last: the redraw command's and Adjust's.
    reframe: Reframe,
    target: Target,
}

/// What a look does to the stored picture.
#[derive(Clone, Copy, Debug)]
enum Adjusted<'a> {
    /// Nothing: the stored picture.
    Stored,
    /// These adjustments; neutral ones draw as the stored picture does.
    Set(&'a Adjustments),
    /// These adjustments through the adjusted path even when neutral, on
    /// the GPU: the redraw command's and Adjust's.
    Redrawn(&'a Adjustments),
}

/// Where a look's colors go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// The screen: sRGB while Color Management is on, else a standard
    /// stack's values in its own profile.
    Display,
    File(OutputSpace),
}

impl<'a> Look<'a> {
    /// The render on screen, under `adjustments`.
    pub fn shown(stack: &'a CompletedStack, adjustments: &'a Adjustments) -> Self {
        Self {
            stack,
            adjusted: Adjusted::Set(adjustments),
            reframe: Reframe::NONE,
            target: Target::Display,
        }
    }

    /// The stored picture on screen, with no adjustments: source frames,
    /// the depth map and Retouch.
    pub fn stored(stack: &'a CompletedStack) -> Self {
        Self { stack, adjusted: Adjusted::Stored, reframe: Reframe::NONE, target: Target::Display }
    }

    /// The render exported, under `adjustments` (`None`: the stored
    /// picture, as aligned frames export): a linear stack in sRGB, a
    /// standard one in its own profile.
    pub fn exported(stack: &'a CompletedStack, adjustments: Option<&'a Adjustments>) -> Self {
        let output = match stack.color {
            StackColor::Linear { .. } => OutputSpace::Srgb,
            StackColor::Srgb { .. } => OutputSpace::StackProfile,
        };
        let adjusted = adjustments.map_or(Adjusted::Stored, Adjusted::Set);
        Self { stack, adjusted, reframe: Reframe::NONE, target: Target::File(output) }
    }

    /// The render exported under what it was saved with in the Library:
    /// its adjustments and reframe, into [`Self::exported`]'s space. With
    /// none set it draws as [`Self::exported`] with no slider set.
    pub fn saved(stack: &'a CompletedStack, saved: &'a RenderAdjustments) -> Self {
        let exported = Self::exported(stack, Some(&saved.adjustments));
        Self { reframe: saved.reframe, ..exported }
    }

    /// The render exported under `adjustments` and `reframe` into
    /// `output`, always through the adjusted path: the redraw command's,
    /// which shows that path before any slider is set.
    pub fn redrawn(
        stack: &'a CompletedStack,
        adjustments: &'a Adjustments,
        reframe: Reframe,
        output: OutputSpace,
    ) -> Self {
        let (adjusted, target) = (Adjusted::Redrawn(adjustments), Target::File(output));
        Self { stack, adjusted, reframe, target }
    }

    /// The render on screen in Adjust under `adjustments` and `reframe`,
    /// always through the adjusted path, so every tick and the picture it
    /// settles on draw alike (ADR-138).
    pub fn adjusting(
        stack: &'a CompletedStack,
        adjustments: &'a Adjustments,
        reframe: Reframe,
    ) -> Self {
        Self { stack, adjusted: Adjusted::Redrawn(adjustments), reframe, target: Target::Display }
    }

    /// The shader's constants it draws with, when it draws on the GPU (an
    /// Adjust look always does): for benches and probes.
    pub fn constants(&self) -> Option<Constants> {
        match self.resolve() {
            Draw::Rendered(constants) => Some(*constants),
            Draw::Stored { .. } => None,
        }
    }

    /// Whether a slider is off its default: such an export can take 16
    /// bits from an 8-bit stack.
    pub fn is_adjusted(&self) -> bool {
        match self.adjusted {
            Adjusted::Stored => false,
            Adjusted::Set(adjustments) | Adjusted::Redrawn(adjustments) => {
                !adjustments.is_neutral()
            }
        }
    }

    pub fn reframe(&self) -> Reframe {
        self.reframe
    }

    pub fn stack(&self) -> &'a CompletedStack {
        self.stack
    }

    /// The space this look's colors go to, the screen's as it is now.
    pub fn output(&self) -> OutputSpace {
        match self.target {
            Target::Display => display_space(),
            Target::File(output) => output,
        }
    }

    /// How this look draws the stack's pixels, a vignette centered on the
    /// reframe's rectangle.
    pub(crate) fn resolve(&self) -> Draw<'a> {
        match (self.drawn(), self.reframe.crop) {
            (Draw::Rendered(constants), Some(crop)) => {
                Draw::Rendered(Box::new(constants.framed(crop)))
            }
            (draw, _) => draw,
        }
    }

    /// [`Self::resolve`] on the whole picture.
    fn drawn(&self) -> Draw<'a> {
        let (color, output) = (self.color(), self.output());
        let adjustments = match self.adjusted {
            Adjusted::Stored => return resolve(color, None, output),
            Adjusted::Set(adjustments) | Adjusted::Redrawn(adjustments) => adjustments,
        };
        if let Some(balanced) = self.balanced() {
            let color = Color::Linear(&balanced);
            return Draw::Rendered(resolve_adjusted(color, adjustments, output));
        }
        match self.adjusted {
            Adjusted::Redrawn(_) => Draw::Rendered(resolve_adjusted(color, adjustments, output)),
            _ => resolve(color, Some(adjustments), output),
        }
    }

    /// The white a camera RAW stack is set to, when not as shot: the DNG
    /// SDK's white point for its kelvins and tint.
    pub fn white(&self) -> Option<WhiteXy> {
        let (Adjusted::Set(adjustments) | Adjusted::Redrawn(adjustments)) = self.adjusted else {
            return None;
        };
        match (&self.stack.color, adjustments.white_balance) {
            (StackColor::Linear { .. }, WhiteBalance::Kelvin { temperature, tint }) => {
                let kelvin = slider::TEMPERATURE_KELVIN.clamp(temperature);
                let tint = slider::TINT_RAW.clamp(tint);
                Some(Temperature { kelvin, tint }.white())
            }
            _ => None,
        }
    }

    /// A camera RAW stack's color science at [`Self::white`], the stored
    /// one's if the SDK can't make it (logged).
    fn balanced(&self) -> Option<RenderParams> {
        let StackColor::Linear { metadata, .. } = &self.stack.color else { return None };
        let white = self.white()?;
        match metadata.render_params_at(Some(white)) {
            Ok(params) => Some(params),
            Err(e) => {
                tracing::warn!("white balance at {white:?} not rendered, drawn as shot: {e}");
                None
            }
        }
    }

    fn color(&self) -> Color<'a> {
        match &self.stack.color {
            StackColor::Linear { render_params, .. } => Color::Linear(render_params),
            StackColor::Srgb { profile, .. } => Color::Standard(profile.as_ref()),
        }
    }

    /// The ICC profile an export in this look carries: the stack's own when
    /// its values are drawn in it, a wide space's, else none (sRGB).
    pub(crate) fn icc(&self) -> Option<io::IccProfile> {
        match self.resolve().output() {
            OutputSpace::StackProfile => self.stack.profile().cloned(),
            OutputSpace::Srgb => None,
            wide @ (OutputSpace::DisplayP3 | OutputSpace::AdobeRgb | OutputSpace::ProPhoto) => {
                wide.wide_icc()
            }
        }
    }
}
