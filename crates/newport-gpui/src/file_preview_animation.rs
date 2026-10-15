//! Finite raster playback, retaining the decoded image and GPU frame cache.
use super::*;
use std::sync::Arc;

pub(super) type Clock = Rc<RefCell<Playback>>;

#[derive(Default)]
pub(super) struct Playback {
    image_id: Option<usize>,
    elapsed: Duration,
    last: Option<Instant>,
}

impl Playback {
    fn tick(&mut self, image_id: usize, now: Instant, active: bool) -> Duration {
        if self.image_id != Some(image_id) {
            *self = Self {
                image_id: Some(image_id),
                ..Self::default()
            };
        }
        if active {
            if let Some(last) = self.last.replace(now) {
                self.elapsed = self
                    .elapsed
                    .saturating_add(now.saturating_duration_since(last));
            }
        } else {
            // Like GPUI's image element, pause when inactive or reducing motion.
            self.last = None;
        }
        self.elapsed
    }
}

/// Cumulative deadlines avoid an unbounded catch-up loop after a delayed paint.
fn frame_at(deadlines: &[Duration], plays: u64, elapsed: Duration) -> (usize, bool) {
    let Some(cycle) = deadlines.last().copied() else {
        return (0, false);
    };
    let cycle_ns = cycle.as_nanos();
    if cycle_ns == 0 || elapsed.as_nanos() / cycle_ns >= u128::from(plays) {
        return (deadlines.len() - 1, false);
    }
    let phase = elapsed.as_nanos() % cycle_ns;
    (
        deadlines.partition_point(|end| end.as_nanos() <= phase),
        true,
    )
}

pub(super) fn raster(image: Arc<RenderImage>, plays: Option<u64>, clock: Clock) -> AnyElement {
    let Some(plays) = plays.filter(|plays| *plays > 0 && image.frame_count() > 1) else {
        return img(image.clone())
            .id(("file-preview-raster", image.id.0))
            .size_full()
            .object_fit(ObjectFit::Contain)
            .into_any_element();
    };
    let mut total = Duration::ZERO;
    let deadlines: Vec<_> = (0..image.frame_count())
        .map(|index| {
            // A zero delay must still make progress, without a tight paint loop.
            total = total
                .saturating_add(Duration::from(image.delay(index)).max(Duration::from_millis(10)));
            total
        })
        .collect();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let active = window.is_window_active() && !cx.reduce_motion();
            let elapsed = clock.borrow_mut().tick(image.id.0, Instant::now(), active);
            let (frame, running) = frame_at(&deadlines, plays, elapsed);
            let fitted = ObjectFit::Contain.get_bounds(bounds, image.size(frame));
            if let Err(error) = window.paint_image(
                bounds,
                fitted,
                Corners::default(),
                image.clone(),
                frame,
                false,
            ) {
                eprintln!("Image preview paint failed: {error}");
                return;
            }
            if active && running {
                window.request_animation_frame();
            }
        },
    )
    .size_full()
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Playback, frame_at};
    use std::time::{Duration, Instant};

    #[test]
    fn finite_playback_observes_delays_and_stops_on_last_frame() {
        let ends = [Duration::from_millis(400), Duration::from_millis(1000)];
        for (ms, expected) in [
            (0, (0, true)),
            (399, (0, true)),
            (400, (1, true)),
            (999, (1, true)),
            (1000, (0, true)),
            (1400, (1, true)),
            (2000, (1, false)),
            (u64::MAX, (1, false)),
        ] {
            assert_eq!(frame_at(&ends, 2, Duration::from_millis(ms)), expected);
        }
        assert_eq!(frame_at(&ends, 1, Duration::from_secs(1)), (1, false));
    }

    #[test]
    fn playback_pauses_and_only_new_images_reset_the_clock() {
        let mut clock = Playback::default();
        let start = Instant::now();
        assert_eq!(clock.tick(1, start, true), Duration::ZERO);
        assert_eq!(
            clock.tick(1, start + Duration::from_millis(450), true),
            Duration::from_millis(450)
        );
        clock.tick(1, start + Duration::from_secs(1), false);
        assert_eq!(
            clock.tick(1, start + Duration::from_secs(20), true),
            Duration::from_millis(450)
        );
        assert_eq!(
            clock.tick(1, start + Duration::from_millis(20200), true),
            Duration::from_millis(650)
        );
        assert_eq!(
            clock.tick(2, start + Duration::from_secs(21), true),
            Duration::ZERO
        );
    }
}
