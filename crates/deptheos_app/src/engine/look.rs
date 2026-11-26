//! What a draw turns a stack's stored pixels into: the stack's color, the
//! render's adjustments and the output space, as one value every draw
//! takes. Source frames, the depth map, Retouch and aligned frames show the
//! stored picture, through [`Look::stored`] and [`Look::exported`] with no
//! adjustments, so a draw with none says so.

use deptheos_core::io;
use deptheos_render::{Adjustments, Color, Draw, OutputSpace, resolve};

use crate::engine::present::display_space;
use crate::runs::session::{CompletedStack, StackColor};

#[derive(Clone, Copy)]
pub struct Look<'a> {
    stack: &'a CompletedStack,
    /// `None`: the stored picture.
    adjustments: Option<&'a Adjustments>,
    target: Target,
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
        Self { stack, adjustments: Some(adjustments), target: Target::Display }
    }

    /// The stored picture on screen, with no adjustments: source frames,
    /// the depth map and Retouch.
    pub fn stored(stack: &'a CompletedStack) -> Self {
        Self { stack, adjustments: None, target: Target::Display }
    }

    /// The render exported, under `adjustments` (`None`: the stored
    /// picture, as aligned frames export): a linear stack in sRGB, a
    /// standard one in its own profile.
    pub fn exported(stack: &'a CompletedStack, adjustments: Option<&'a Adjustments>) -> Self {
        let output = match stack.color {
            StackColor::Linear { .. } => OutputSpace::Srgb,
            StackColor::Srgb { .. } => OutputSpace::StackProfile,
        };
        Self { stack, adjustments, target: Target::File(output) }
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

    /// How this look draws the stack's pixels.
    pub(crate) fn resolve(&self) -> Draw<'a> {
        resolve(self.color(), self.adjustments, self.output())
    }

    /// [`Self::resolve`] through the adjusted path even with no
    /// adjustments or neutral ones: always on the GPU.
    pub fn resolve_adjusted(&self) -> Box<deptheos_render::dng_render::Constants> {
        let neutral = Adjustments::NEUTRAL;
        let adjustments = self.adjustments.unwrap_or(&neutral);
        deptheos_render::resolve_adjusted(self.color(), adjustments, self.output())
    }

    fn color(&self) -> Color<'a> {
        match &self.stack.color {
            StackColor::Linear { render_params, .. } => Color::Linear(render_params),
            StackColor::Srgb { profile, .. } => Color::Standard(profile.as_ref()),
        }
    }

    /// The ICC profile an export in this look carries: the stack's own when
    /// its values are drawn in it, else none (sRGB).
    pub(crate) fn icc(&self) -> Option<io::IccProfile> {
        match self.resolve().output() {
            OutputSpace::StackProfile => self.stack.profile().cloned(),
            OutputSpace::Srgb => None,
        }
    }
}
