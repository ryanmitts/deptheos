//! What a draw turns a stack's stored pixels into: the stack's color, the
//! render's adjustments and the output space, as one value every draw
//! takes. Source frames, the depth map, Retouch and aligned frames show the
//! stored picture, through [`Look::stored`] and [`Look::exported`] with no
//! adjustments, so a draw with none says so.

use deptheos_core::io;
use deptheos_render::{Adjustments, Color, Draw, OutputSpace, Reframe, resolve};

use crate::engine::present::display_space;
use crate::runs::session::{CompletedStack, StackColor};

#[derive(Clone, Copy)]
pub struct Look<'a> {
    stack: &'a CompletedStack,
    adjusted: Adjusted<'a>,
    /// The window drawn, applied last; none but the redraw command's yet.
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
    /// the GPU: the redraw command's.
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

    /// How this look draws the stack's pixels.
    pub(crate) fn resolve(&self) -> Draw<'a> {
        let (color, output) = (self.color(), self.output());
        match self.adjusted {
            Adjusted::Stored => resolve(color, None, output),
            Adjusted::Set(adjustments) => resolve(color, Some(adjustments), output),
            Adjusted::Redrawn(adjustments) => {
                Draw::Rendered(deptheos_render::resolve_adjusted(color, adjustments, output))
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
    /// its values are drawn in it, else none (sRGB).
    pub(crate) fn icc(&self) -> Option<io::IccProfile> {
        match self.resolve().output() {
            OutputSpace::StackProfile => self.stack.profile().cloned(),
            OutputSpace::Srgb => None,
        }
    }
}
