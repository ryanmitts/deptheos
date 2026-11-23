//! Type the desktop sets beyond the kit's defaults (`design/README.md`,
//! "System fonts in the apps").

use gpui::{FontFeatures, Styled};

/// Type settings for any element that styles text.
pub(crate) trait TypeStyled: Styled + Sized {
    /// Tabular digits (the system font's, not a monospace), so changing
    /// numbers hold still and columns line up.
    fn tabular(self) -> Self {
        self.font_features(tabular_digits())
    }
}

impl<E: Styled> TypeStyled for E {}

/// The OpenType feature for tabular digits.
fn tabular_digits() -> FontFeatures {
    FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}

#[cfg(test)]
mod tests {
    use gpui::div;

    use super::*;

    #[test]
    fn tabular_turns_on_tabular_digits() {
        let mut text = div().tabular();
        let features = text.text_style().font_features.clone().expect("features are set");
        assert_eq!(features.tag_value_list(), [("tnum".to_string(), 1)]);
    }
}
