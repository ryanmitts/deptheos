//! The windows' color space (ADR-172). On macOS the windows are drawn for
//! the main window's screen: Display P3 on a wide-gamut one, else sRGB.
//! GPUI leaves the Metal layer untagged, so Core Animation would pass
//! values through; tagged with the space, macOS converts per screen. With
//! Color Management off the layer stays untagged and everything is sRGB,
//! values shown as they are. Windows and Linux take output as sRGB.
//!
//! [`shown`] is the space a picture must be in on screen: the engine draws
//! photos in it, `crate::images` converts the rest at hand-off, and the
//! theme and [`brand`] colors are generated in both.

use deptheos_app::ScreenSpace;
use deptheos_app::engine::present;
use gpui::{App, Context, Global, Window};

use crate::brand::{self, Brand};

/// The gamut of the main window's screen, as the app's global.
///
/// Lifecycle: `Srgb` or `DisplayP3` ([`ScreenSpace`]), changed only by
/// [`follow`]: `Srgb` → `DisplayP3` when the main window opens on or moves
/// to a wide-gamut screen, back when it moves to one that is not.
/// [`set_color_managed`] sets it again unchanged, so the views holding
/// pictures (`cx.observe_global::<Screen>`) convert them to [`shown`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Screen(pub ScreenSpace);

impl Global for Screen {}

/// The space pictures and colors are shown in: the screen's while Color
/// Management is on, else sRGB.
pub fn shown() -> ScreenSpace {
    match present::color_managed() {
        true => present::screen(),
        false => ScreenSpace::Srgb,
    }
}

/// The brand colors in the space shown.
pub fn brand() -> &'static Brand {
    match shown() {
        ScreenSpace::Srgb => &brand::SRGB,
        ScreenSpace::DisplayP3 => &brand::DISPLAY_P3,
    }
}

/// An sRGB `0xrrggbb` color in the space shown.
pub fn srgb(rgb: u32) -> u32 {
    srgba(rgb << 8 | 0xff) >> 8
}

/// An sRGB `0xrrggbbaa` color in the space shown, its alpha kept.
pub fn srgba(rgba: u32) -> u32 {
    let space = shown();
    if space == ScreenSpace::Srgb {
        return rgba;
    }
    let [r, g, b, a] = rgba.to_be_bytes();
    let mut pixel = [b, g, r, a];
    present::convert_display(&mut pixel, ScreenSpace::Srgb, space);
    let [b, g, r, a] = pixel;
    u32::from_be_bytes([r, g, b, a])
}

/// Tag `window`'s drawing surface with the screen's space while Color
/// Management is on, or leave it untagged as GPUI does. Does nothing
/// outside macOS.
pub fn tag_window(window: &Window) {
    #[cfg(target_os = "macos")]
    if let Err(e) = macos::tag(window, present::color_managed().then(present::screen)) {
        tracing::warn!("could not set the window's color space: {e}");
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// Follow the screen of the main window, `window`: now, and as it moves,
/// changes screen or comes to the front (a display reconfigured).
pub fn follow_main_window<V: 'static>(window: &mut Window, cx: &mut Context<V>) {
    follow(window, cx);
    cx.observe_window_bounds(window, |_, window, cx| follow(window, cx)).detach();
    cx.observe_window_activation(window, |_, window, cx| follow(window, cx)).detach();
}

/// Turn Color Management on or off from `window`, every window following.
pub fn set_color_managed(on: bool, window: &Window, cx: &mut App) {
    present::set_color_managed(on);
    apply(window, cx);
}

/// The main window, `window`, is on the screen it is: applied if its
/// gamut changed.
fn follow(window: &Window, cx: &mut App) {
    let gamut = gamut(window);
    let was = cx.try_global::<Screen>().copied().unwrap_or_default();
    if was.0 == gamut {
        return;
    }
    tracing::debug!("screen: {:?} -> {gamut:?}", was.0);
    present::set_screen(gamut);
    apply(window, cx);
}

/// Draw every window for [`shown`]: tagged, its theme reinstalled and
/// redrawn, and the views told to convert their pictures. `window` is the
/// one being updated, which can't be reached through its handle.
fn apply(window: &Window, cx: &mut App) {
    let space = shown();
    tracing::debug!("windows shown in {space:?}");
    tag_window(window);
    for handle in cx.windows() {
        if handle.window_id() != window.window_handle().window_id() {
            handle.update(cx, |_, window, _| tag_window(window)).ok();
        }
    }
    crate::install_brand_theme(space, cx);
    crate::look::apply_appearance(None, true, cx);
    cx.set_global(Screen(present::screen()));
    cx.refresh_windows();
}

/// The gamut of `window`'s screen; sRGB outside macOS or with no screen.
fn gamut(window: &Window) -> ScreenSpace {
    #[cfg(target_os = "macos")]
    return macos::gamut(window).unwrap_or_else(|e| {
        tracing::warn!("could not read the screen's gamut: {e}");
        ScreenSpace::Srgb
    });
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        ScreenSpace::Srgb
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use deptheos_app::ScreenSpace;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_core_graphics::{CGColorSpace, kCGColorSpaceDisplayP3, kCGColorSpaceSRGB};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    /// `NSDisplayGamutP3`.
    const DISPLAY_GAMUT_P3: isize = 2;

    /// The window's `NSView`.
    fn view(window: &gpui::Window) -> Result<&AnyObject, String> {
        let handle = HasWindowHandle::window_handle(window).map_err(|e| e.to_string())?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return Err("not an AppKit window".into());
        };
        // SAFETY: the handle's view lives as long as the window, which the
        // caller borrows; GPUI calls us on the main thread.
        Ok(unsafe { appkit.ns_view.cast().as_ref() })
    }

    /// GPUI's view draws into a `CAMetalLayer`, its backing layer: set that
    /// layer's `colorspace` to `space`, or back to none.
    pub fn tag(window: &gpui::Window, space: Option<ScreenSpace>) -> Result<(), String> {
        let view = view(window)?;
        // SAFETY: AppKit and Core Animation calls on the main thread, on
        // objects the window keeps alive.
        unsafe {
            let layer: Option<&AnyObject> = msg_send![view, layer];
            let layer = layer.ok_or("the window's view has no layer")?;
            let is_metal: bool = msg_send![layer, isKindOfClass: class!(CAMetalLayer)];
            if !is_metal {
                return Err("the window's layer is not a Metal layer".into());
            }
            let space = space.and_then(|space| {
                CGColorSpace::with_name(Some(match space {
                    ScreenSpace::Srgb => kCGColorSpaceSRGB,
                    ScreenSpace::DisplayP3 => kCGColorSpaceDisplayP3,
                }))
            });
            let _: () = msg_send![layer, setColorspace: space.as_deref()];
        }
        Ok(())
    }

    /// Display P3 when the window's screen can show it, else sRGB.
    pub fn gamut(window: &gpui::Window) -> Result<ScreenSpace, String> {
        let view = view(window)?;
        // SAFETY: AppKit calls on the main thread, on objects the window
        // keeps alive.
        unsafe {
            let ns_window: Option<&AnyObject> = msg_send![view, window];
            let ns_window = ns_window.ok_or("the view has no window")?;
            let screen: Option<&AnyObject> = msg_send![ns_window, screen];
            let Some(screen) = screen else { return Ok(ScreenSpace::Srgb) };
            let p3: bool = msg_send![screen, canRepresentDisplayGamut: DISPLAY_GAMUT_P3];
            Ok(if p3 { ScreenSpace::DisplayP3 } else { ScreenSpace::Srgb })
        }
    }
}
