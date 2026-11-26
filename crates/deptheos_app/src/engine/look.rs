//! What a draw turns a stack's stored pixels into: the stack's color, the
//! render's adjustments and the output space, as one value every draw
//! takes. Source frames, the depth map, Retouch and aligned frames show the
//! stored picture, through [`Look::stored`] and [`Look::exported`] with no
//! adjustments, so a draw with none says so.

use deptheos_core::io;
use deptheos_render::{Adjustments, Color, Draw, OutputSpace, resolve};

use crate::runs::session::{CompletedStack, StackColor};

#[derive(Clone, Copy)]
pub struct Look<'a> {
    stack: &'a CompletedStack,
    /// `None`: the stored picture.
    adjustments: Option<&'a Adjustments>,
    output: OutputSpace,
}

impl<'a> Look<'a> {
    /// The render on screen, under `adjustments`.
    pub fn shown(stack: &'a CompletedStack, adjustments: &'a Adjustments) -> Self {
        Self { stack, adjustments: Some(adjustments), output: OutputSpace::Display }
    }

    /// The stored picture on screen, with no adjustments: source frames,
    /// the depth map and Retouch.
    pub fn stored(stack: &'a CompletedStack) -> Self {
        Self { stack, adjustments: None, output: OutputSpace::Display }
    }

    /// The render exported, under `adjustments` (`None`: the stored
    /// picture, as aligned frames export): a linear stack in sRGB, a
    /// standard one in its own profile.
    pub fn exported(stack: &'a CompletedStack, adjustments: Option<&'a Adjustments>) -> Self {
        let output = match stack.color {
            StackColor::Linear { .. } => OutputSpace::Srgb,
            StackColor::Srgb { .. } => OutputSpace::StackProfile,
        };
        Self { stack, adjustments, output }
    }

    pub fn stack(&self) -> &'a CompletedStack {
        self.stack
    }

    pub fn output(&self) -> OutputSpace {
        self.output
    }

    /// How this look draws the stack's pixels.
    pub(crate) fn resolve(&self) -> Draw<'a> {
        let color = match &self.stack.color {
            StackColor::Linear { render_params, .. } => Color::Linear(render_params),
            StackColor::Srgb { profile, .. } => Color::Standard(profile.as_ref()),
        };
        resolve(color, self.adjustments, self.output)
    }

    /// The ICC profile an export in this look carries: the stack's own when
    /// its values pass through, else none (sRGB).
    pub(crate) fn icc(&self) -> Option<io::IccProfile> {
        match self.output {
            OutputSpace::StackProfile => self.stack.profile().cloned(),
            OutputSpace::Srgb | OutputSpace::Display => None,
        }
    }
}
