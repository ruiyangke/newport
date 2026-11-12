//! Appearance choices, independent of the native UI framework. They are saved
//! with the window's other preferences (see window_preferences.rs).
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Follow system",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_serialize_as_reference_values() {
        for (choice, expected) in [
            (Appearance::System, "\"system\""),
            (Appearance::Light, "\"light\""),
            (Appearance::Dark, "\"dark\""),
        ] {
            assert_eq!(serde_json::to_string(&choice).unwrap(), expected);
            assert_eq!(
                serde_json::from_str::<Appearance>(expected).unwrap(),
                choice
            );
        }
    }
}
