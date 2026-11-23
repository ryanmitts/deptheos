//! The window's color space. Display images are sRGB
//! ([`deptheos_app::engine::present`] converts wide gamut). On macOS GPUI
//! leaves the Metal layer untagged, so Core Animation passes values through
//! and P3 screens oversaturate; tagged, macOS converts per screen. Windows
//! and Linux already take output as sRGB.

use gpui::Window;

/// Tag `window`'s drawing surface as sRGB (`managed`), or leave it
/// untagged as GPUI does (to compare). Does nothing outside macOS.
pub fn tag_window(window: &Window, managed: bool) {
    #[cfg(target_os = "macos")]
    if let Err(e) = macos::tag(window, managed) {
        tracing::warn!("could not set the window's color space: {e}");
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (window, managed);
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_core_graphics::{CGColorSpace, kCGColorSpaceSRGB};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    /// GPUI's view draws into a `CAMetalLayer`, its backing layer: set that
    /// layer's `colorspace` to sRGB, or back to none.
    pub fn tag(window: &gpui::Window, managed: bool) -> Result<(), String> {
        let handle = HasWindowHandle::window_handle(window).map_err(|e| e.to_string())?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return Err("not an AppKit window".into());
        };
        // SAFETY: the handle's view lives as long as the window, which we
        // borrow; GPUI calls us on the main thread.
        unsafe {
            let view: &AnyObject = appkit.ns_view.cast().as_ref();
            let layer: Option<&AnyObject> = msg_send![view, layer];
            let layer = layer.ok_or("the window's view has no layer")?;
            let is_metal: bool = msg_send![layer, isKindOfClass: class!(CAMetalLayer)];
            if !is_metal {
                return Err("the window's layer is not a Metal layer".into());
            }
            let space =
                if managed { CGColorSpace::with_name(Some(kCGColorSpaceSRGB)) } else { None };
            let _: () = msg_send![layer, setColorspace: space.as_deref()];
        }
        Ok(())
    }
}
