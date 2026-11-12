//! Runtime client display metrics, independent of any particular monitor.
use crate::remote_apps::v1::Scale;

/// Usable screen rectangle relative to a toplevel's captured content origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PopupBounds {
    /// Signed logical points from captured content's top-left, with y down.
    /// This deliberately omits the native desktop's absolute coordinates.
    pub origin: [i32; 2],
    /// Usable screen extent, not a buffer allocation or popup size request.
    pub size: [u32; 2],
}

impl PopupBounds {
    pub fn validate(self) -> Result<(), String> {
        if self.origin.iter().any(|v| v.unsigned_abs() > 1_048_576)
            || self.size.iter().any(|v| !(1..=65_535).contains(v))
        {
            return Err("Invalid popup work area".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayProfile {
    pub scale: Scale,
    /// Usable screen size in logical points, excluding system UI.
    pub work_area: [u32; 2],
}

impl DisplayProfile {
    pub fn validate(self) -> Result<(), String> {
        let Scale {
            numerator: n,
            denominator: d,
        } = self.scale;
        if n == 0
            || d == 0
            || n > 64
            || d > 64
            || u32::from(n) > 4 * u32::from(d)
            || self.work_area.iter().any(|v| !(1..=65535).contains(v))
        {
            return Err("Invalid client display profile".into());
        }
        Ok(())
    }

    pub fn from_metrics(factor: f64, work_area: [u32; 2]) -> Result<Self, String> {
        if !factor.is_finite() || !(1. / 64. ..=4.).contains(&factor) {
            return Err("Unsupported client display scale".into());
        }
        let mut closest = (f64::INFINITY, 1, 1);
        for d in 1..=64u16 {
            let n = (factor * f64::from(d)).round() as u16;
            if n == 0 || n > 64 || u32::from(n) > 4 * u32::from(d) {
                continue;
            }
            let error = (factor - f64::from(n) / f64::from(d)).abs();
            if error < closest.0 {
                closest = (error, n, d);
            }
        }
        let profile = Self {
            scale: Scale {
                numerator: closest.1,
                denominator: closest.2,
            },
            work_area,
        };
        profile.validate()?;
        Ok(profile)
    }
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    #[test]
    fn popup_bounds_are_bounded_signed_content_coordinates() {
        for origin in [[-1_048_576, 1_048_576], [0, 0], [-123, -456]] {
            assert!(
                PopupBounds {
                    origin,
                    size: [65_535, 1]
                }
                .validate()
                .is_ok()
            );
        }
        for bounds in [
            PopupBounds {
                origin: [i32::MIN, 0],
                size: [800, 600],
            },
            PopupBounds {
                origin: [1_048_577, 0],
                size: [800, 600],
            },
            PopupBounds {
                origin: [0, 0],
                size: [0, 600],
            },
            PopupBounds {
                origin: [0, 0],
                size: [800, 65_536],
            },
        ] {
            assert!(bounds.validate().is_err());
        }
        for wire in [
            r#"{"origin":[null,0],"size":[800,600]}"#,
            r#"{"origin":[0,0],"size":[-1,600]}"#,
            r#"{"origin":[0,0],"size":[800,600],"extra":1}"#,
        ] {
            assert!(serde_json::from_str::<PopupBounds>(wire).is_err());
        }
    }

    #[test]
    fn popup_extension_keeps_the_display_wire_shape() {
        use crate::remote_apps::v1::Operation;
        let operation = Operation::SetDisplay {
            profile: DisplayProfile {
                scale: Scale {
                    numerator: 1,
                    denominator: 1,
                },
                work_area: [800, 600],
            },
        };
        assert_eq!(
            serde_json::to_value(operation).unwrap(),
            serde_json::json!({
                "action": "set_display", "profile": {"scale": {"numerator": 1, "denominator": 1}, "work_area": [800, 600]}
            })
        );
        let bounds = PopupBounds {
            origin: [-400, -20],
            size: [1600, 900],
        };
        let wire = serde_json::to_string(&Operation::SetPopupBounds { bounds }).unwrap();
        assert!(
            matches!(serde_json::from_str::<Operation>(&wire).unwrap(), Operation::SetPopupBounds { bounds: value } if value == bounds)
        );
    }
    #[test]
    fn scales_follow_runtime_metrics_including_fractional_displays() {
        for (factor, n, d) in [
            (1., 1, 1),
            (1.25, 5, 4),
            (1.5, 3, 2),
            (2., 2, 1),
            (3., 3, 1),
        ] {
            let profile = DisplayProfile::from_metrics(factor, [1600, 900]).unwrap();
            assert_eq!(
                profile.scale,
                Scale {
                    numerator: n,
                    denominator: d
                }
            );
        }
    }
    #[test]
    fn invalid_metrics_are_rejected() {
        for factor in [0., -1., f64::NAN, f64::INFINITY, 5.] {
            assert!(DisplayProfile::from_metrics(factor, [1600, 900]).is_err());
        }
        assert!(DisplayProfile::from_metrics(1., [0, 900]).is_err());
    }
    #[test]
    fn hello_round_trips_metrics_and_their_absence() {
        use crate::remote_apps::v1::ClientMessage;
        let hello = |display| ClientMessage::Hello {
            performance: None,
            versions: vec![1],
            features: Default::default(),
            display,
            license: None,
        };
        let read = |message: ClientMessage| {
            let wire = crate::remote_apps::wire::encode_client(&message).unwrap();
            crate::remote_apps::wire::read_client(&mut wire.as_slice())
                .unwrap()
                .0
        };
        assert!(matches!(
            read(hello(None)),
            ClientMessage::Hello { display: None, .. }
        ));
        let profile = DisplayProfile::from_metrics(1.5, [1920, 1080]).unwrap();
        let parsed = read(hello(Some(profile)));
        assert!(
            matches!(parsed, ClientMessage::Hello { display: Some(value), .. } if value == profile)
        );
    }
}
