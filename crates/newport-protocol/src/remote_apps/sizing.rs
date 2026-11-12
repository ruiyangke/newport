//! Physical ICCCM sizing rules. No capability or wire field is enabled here.
use crate::remote_apps::v1::Scale;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelConstraints {
    pub min: Option<[u32; 2]>,
    pub max: Option<[u32; 2]>,
    pub base: Option<[u32; 2]>,
    pub increment: Option<[u32; 2]>,
    /// Minimum and maximum width/height fractions, numerator first.
    pub aspect: Option<[[u32; 2]; 2]>,
}
impl PixelConstraints {
    pub fn validate(self) -> Result<(), String> {
        let positive = |n: u32| n > 0 && n <= i32::MAX as u32;
        if [self.min, self.max, self.increment]
            .into_iter()
            .flatten()
            .flatten()
            .any(|n| !positive(n))
            || self.base.into_iter().flatten().any(|n| n > i32::MAX as u32)
        {
            return Err("Invalid physical size constraint".into());
        }
        let min = self.minimum();
        if self
            .max
            .is_some_and(|max| min[0] > max[0] || min[1] > max[1])
        {
            return Err("Inverted physical size bounds".into());
        }
        if let Some([lo, hi]) = self.aspect
            && (lo.into_iter().chain(hi).any(|n| !positive(n))
                || u64::from(lo[0]) * u64::from(hi[1]) > u64::from(hi[0]) * u64::from(lo[1]))
        {
            return Err("Invalid physical aspect range".into());
        }
        Ok(())
    }
    fn minimum(self) -> [u32; 2] {
        let min = self.min.or(self.base).unwrap_or([1, 1]);
        [min[0].max(1), min[1].max(1)]
    }
    fn axis_accepts(self, axis: usize, pixels: u32) -> bool {
        if pixels < self.minimum()[axis] || self.max.is_some_and(|max| pixels > max[axis]) {
            return false;
        }
        if let Some(increment) = self.increment {
            let base = self.base.or(self.min).unwrap_or([0, 0])[axis];
            return pixels
                .checked_sub(base)
                .is_some_and(|delta| delta % increment[axis] == 0);
        }
        true
    }
    pub fn accepts(self, pixels: [u32; 2]) -> bool {
        if self.validate().is_err() || !(0..2).all(|axis| self.axis_accepts(axis, pixels[axis])) {
            return false;
        }
        if let Some([lo, hi]) = self.aspect {
            // PMinSize is a base fallback for increments, NOT aspect ratios.
            let base = self.base.unwrap_or([0, 0]);
            let Some(w) = pixels[0].checked_sub(base[0]).filter(|n| *n > 0) else {
                return false;
            };
            let Some(h) = pixels[1].checked_sub(base[1]).filter(|n| *n > 0) else {
                return false;
            };
            return u64::from(w) * u64::from(lo[1]) >= u64::from(h) * u64::from(lo[0])
                && u64::from(w) * u64::from(hi[1]) <= u64::from(h) * u64::from(hi[0]);
        }
        true
    }
    /// Closest integral logical size representable at this scale, measured by
    /// squared logical distance. Ties prefer smaller width, then smaller height.
    /// None means the valid hints have no solution within the surface limits.
    /// At most 4096 height entries and 4096 width probes; never a 4096² scan.
    pub fn nearest_logical(
        self,
        requested: [u32; 2],
        scale: Scale,
    ) -> Result<Option<[u32; 2]>, String> {
        self.nearest_logical_preserving(requested, scale, [false, false])
    }
    /// As above, but pinned axes must retain their requested logical dimension.
    /// Used for single-axis maximize without altering ICCCM base/aspect semantics.
    pub fn nearest_logical_preserving(
        self,
        requested: [u32; 2],
        scale: Scale,
        pinned: [bool; 2],
    ) -> Result<Option<[u32; 2]>, String> {
        self.validate()?;
        if requested.iter().any(|n| !(1..=4096).contains(n))
            || scale.numerator == 0
            || scale.denominator == 0
            || scale.numerator > 64
            || scale.denominator > 64
            || u32::from(scale.numerator) > u32::from(scale.denominator) * 4
        {
            return Err("Invalid sizing request or scale".into());
        }
        let physical =
            |n: u32| (n * u32::from(scale.numerator)).div_ceil(u32::from(scale.denominator));
        let heights: Vec<_> = (1..=4096)
            .filter(|n| !pinned[1] || *n == requested[1])
            .map(|n| (n, physical(n)))
            .filter(|(_, pixels)| *pixels <= 4096 && self.axis_accepts(1, *pixels))
            .collect();
        let mut best = None;
        let base = self.base.unwrap_or([0, 0]);
        for width in 1..=4096 {
            let pw = physical(width);
            if (pinned[0] && width != requested[0]) || pw > 4096 || !self.axis_accepts(0, pw) {
                continue;
            }
            let (start, end) = if let Some([lo, hi]) = self.aspect {
                let Some(w) = pw.checked_sub(base[0]).filter(|n| *n > 0) else {
                    continue;
                };
                let start = heights.partition_point(|(_, ph)| {
                    *ph <= base[1]
                        || u64::from(w) * u64::from(hi[1])
                            > u64::from(ph - base[1]) * u64::from(hi[0])
                });
                let end = heights.partition_point(|(_, ph)| {
                    *ph <= base[1]
                        || u64::from(w) * u64::from(lo[1])
                            >= u64::from(ph - base[1]) * u64::from(lo[0])
                });
                (start, end)
            } else {
                (0, heights.len())
            };
            if start >= end {
                continue;
            }
            let candidates = &heights[start..end];
            let insertion = candidates.partition_point(|(height, _)| *height < requested[1]);
            for index in [
                insertion.checked_sub(1),
                (insertion < candidates.len()).then_some(insertion),
            ]
            .into_iter()
            .flatten()
            {
                let height = candidates[index].0;
                let dx = i64::from(width) - i64::from(requested[0]);
                let dy = i64::from(height) - i64::from(requested[1]);
                let candidate = (dx * dx + dy * dy, width, height);
                if best.is_none_or(|old| candidate < old) {
                    best = Some(candidate);
                }
            }
        }
        Ok(best.map(|(_, w, h)| [w, h]))
    }
}

#[cfg(all(test, feature = "protobuf"))]
mod tests {
    use super::*;
    #[test]
    fn fractional_scale_preserves_physical_grid() {
        let hints = PixelConstraints {
            min: Some([4, 4]),
            base: Some([2, 2]),
            increment: Some([8, 16]),
            ..Default::default()
        };
        let scale = Scale {
            numerator: 3,
            denominator: 2,
        };
        let result = hints.nearest_logical([101, 77], scale).unwrap().unwrap();
        assert!(hints.accepts(result.map(|n| (n * 3).div_ceil(2))));
        assert_eq!(hints.nearest_logical(result, scale).unwrap(), Some(result));
    }
    #[test]
    fn aspect_subtracts_only_explicit_base() {
        let mut hints = PixelConstraints {
            min: Some([10, 20]),
            aspect: Some([[1, 1], [1, 1]]),
            ..Default::default()
        };
        assert!(hints.accepts([30, 30]));
        assert!(!hints.accepts([30, 40]));
        hints.base = Some([10, 20]);
        assert!(hints.accepts([30, 40]));
        assert!(!hints.accepts([30, 30]));
        assert!(!hints.accepts([10, 20]));
    }
    #[test]
    fn bounded_solver_matches_independent_exhaustive_sizes() {
        for scale in [
            Scale {
                numerator: 1,
                denominator: 1,
            },
            Scale {
                numerator: 3,
                denominator: 2,
            },
            Scale {
                numerator: 2,
                denominator: 1,
            },
            Scale {
                numerator: 1,
                denominator: 2,
            },
        ] {
            for increment in [None, Some([3, 5])] {
                for base in [None, Some([2, 4])] {
                    for aspect in [None, Some([[1, 2], [3, 2]]), Some([[1, 1], [1, 1]])] {
                        let hints = PixelConstraints {
                            min: Some([3, 5]),
                            max: Some([16, 16]),
                            base,
                            increment,
                            aspect,
                        };
                        for requested in [[1, 1], [7, 13], [17, 3], [30, 30]] {
                            for pinned in
                                [[false, false], [true, false], [false, true], [true, true]]
                            {
                                let mut expected = None;
                                for w in 1u32..=32 {
                                    for h in 1u32..=32 {
                                        if (pinned[0] && w != requested[0])
                                            || (pinned[1] && h != requested[1])
                                        {
                                            continue;
                                        }
                                        let p = [w, h].map(|n| {
                                            (n * u32::from(scale.numerator))
                                                .div_ceil(u32::from(scale.denominator))
                                        });
                                        // Deliberately independent formula, not accepts().
                                        let b = base.or(hints.min).unwrap();
                                        if p[0] < 3 || p[1] < 5 || p[0] > 16 || p[1] > 16 {
                                            continue;
                                        }
                                        if increment.is_some_and(|inc| {
                                            (0..2)
                                                .any(|i| p[i] < b[i] || (p[i] - b[i]) % inc[i] != 0)
                                        }) {
                                            continue;
                                        }
                                        if let Some([lo, hi]) = aspect {
                                            let ab = base.unwrap_or([0, 0]);
                                            let x = i64::from(p[0]) - i64::from(ab[0]);
                                            let y = i64::from(p[1]) - i64::from(ab[1]);
                                            if x <= 0
                                                || y <= 0
                                                || x * i64::from(lo[1]) < y * i64::from(lo[0])
                                                || x * i64::from(hi[1]) > y * i64::from(hi[0])
                                            {
                                                continue;
                                            }
                                        }
                                        let dx = i64::from(w) - i64::from(requested[0]);
                                        let dy = i64::from(h) - i64::from(requested[1]);
                                        let candidate = (dx * dx + dy * dy, w, h);
                                        if expected.is_none_or(|old| candidate < old) {
                                            expected = Some(candidate);
                                        }
                                    }
                                }
                                assert_eq!(
                                    hints
                                        .nearest_logical_preserving(requested, scale, pinned)
                                        .unwrap(),
                                    expected.map(|(_, w, h)| [w, h]),
                                    "{hints:?} {scale:?} {requested:?} {pinned:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn invalid_or_unrepresentable_hints_do_not_panic_or_loop() {
        let scale = Scale {
            numerator: 2,
            denominator: 1,
        };
        let fixed_odd = PixelConstraints {
            min: Some([91, 51]),
            max: Some([91, 51]),
            ..Default::default()
        };
        assert_eq!(fixed_odd.nearest_logical([45, 25], scale).unwrap(), None);
        let invalid = PixelConstraints {
            increment: Some([0, 1]),
            ..Default::default()
        };
        assert!(invalid.nearest_logical([100, 100], scale).is_err());
        assert!(!invalid.accepts([100, 100]));
        let extreme = PixelConstraints {
            aspect: Some([[i32::MAX as u32, 1], [i32::MAX as u32, 1]]),
            ..Default::default()
        };
        assert_eq!(extreme.nearest_logical([100, 100], scale).unwrap(), None);
    }
}
