//! Resolve the shared appearance preference against the native window state.
use gpui_kit::WindowAppearance;
pub use newport_client::appearance::Appearance;

/// Native appearance interpretation belongs to the UI; storage and preference
/// values live in the UI-independent client crate.
pub trait ResolveAppearance {
    fn is_dark(&self, system: WindowAppearance) -> bool;
}

impl ResolveAppearance for Appearance {
    fn is_dark(&self, system: WindowAppearance) -> bool {
        match self {
            Self::System => matches!(
                system,
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ),
            Self::Light => false,
            Self::Dark => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_system_variants_and_keeps_explicit_choices() {
        for (system, dark) in [
            (WindowAppearance::Light, false),
            (WindowAppearance::VibrantLight, false),
            (WindowAppearance::Dark, true),
            (WindowAppearance::VibrantDark, true),
        ] {
            assert_eq!(Appearance::System.is_dark(system), dark);
            assert!(!Appearance::Light.is_dark(system));
            assert!(Appearance::Dark.is_dark(system));
        }
    }
}
