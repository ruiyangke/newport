//! Bounded statistics from accepted snapshots of one retained connection.
//! Missing readings stay missing; renderers can split a series at sequence gaps.
use newport_domain::containers::{ContainerState, ContainersSnapshot, valid_container_id};
use std::{
    collections::{HashSet, VecDeque},
    time::{Duration, Instant},
};

pub const HISTORY_LIMIT: usize = 60;
const CONTAINER_LIMIT: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatsSample<T> {
    /// Controller receipt time, never a render timestamp or assumed interval.
    pub received_at: Instant,
    /// Advances once for each accepted successful snapshot of this connection.
    /// Non-consecutive samples indicate an unavailable or non-running reading.
    pub sequence: u64,
    pub value: T,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatsHistory {
    cpu: Vec<StatsSample<f64>>,
    memory: Vec<StatsSample<u64>>,
}
impl StatsHistory {
    pub fn cpu(&self) -> &[StatsSample<f64>] {
        &self.cpu
    }
    pub fn memory(&self) -> &[StatsSample<u64>] {
        &self.memory
    }
}

/// Elapsed time between the oldest and newest retained readings in this series.
pub fn history_span<T>(samples: &[StatsSample<T>]) -> Duration {
    samples
        .first()
        .zip(samples.last())
        .map_or(Duration::ZERO, |(first, last)| {
            last.received_at
                .saturating_duration_since(first.received_at)
        })
}

/// Compact duration derived from real receipt times (for example `23s`, `2m`).
pub fn history_duration_label<T>(samples: &[StatsSample<T>]) -> String {
    let millis = history_span(samples).as_millis().min(i64::MAX as u128) as i64;
    newport_domain::services::duration(millis)
}

#[derive(Default)]
pub(crate) struct StatsCache {
    histories: VecDeque<(String, StatsHistory)>,
    sequence: u64,
}
impl StatsCache {
    pub(crate) fn get(&self, id: &str) -> Option<&StatsHistory> {
        self.histories
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, history)| history)
    }

    /// Called only after the controller accepts a successful, current response.
    pub(crate) fn record(&mut self, snapshot: &ContainersSnapshot, received_at: Instant) {
        self.sequence = self.sequence.wrapping_add(1);
        if !snapshot.truncated {
            let present: HashSet<_> = snapshot
                .containers
                .iter()
                .map(|container| container.id.as_str())
                .collect();
            self.histories
                .retain(|(id, _)| present.contains(id.as_str()));
        }
        let mut seen = HashSet::new();
        for container in &snapshot.containers {
            if container.state != ContainerState::Running
                || !valid_container_id(&container.id)
                || !seen.insert(&container.id)
            {
                continue;
            }
            let cpu = container.cpu.filter(|cpu| cpu.is_finite() && *cpu >= 0.);
            if cpu.is_none() && container.memory.is_none() {
                continue;
            }
            let mut history = self
                .histories
                .iter()
                .position(|(id, _)| id == &container.id)
                .and_then(|index| self.histories.remove(index))
                .map(|(_, history)| history)
                .unwrap_or_default();
            if let Some(value) = cpu {
                push(
                    &mut history.cpu,
                    StatsSample {
                        received_at,
                        sequence: self.sequence,
                        value,
                    },
                );
            }
            if let Some(value) = container.memory {
                push(
                    &mut history.memory,
                    StatsSample {
                        received_at,
                        sequence: self.sequence,
                        value,
                    },
                );
            }
            self.histories.push_back((container.id.clone(), history));
            while self.histories.len() > CONTAINER_LIMIT {
                self.histories.pop_front();
            }
        }
    }
}

fn push<T>(samples: &mut Vec<StatsSample<T>>, sample: StatsSample<T>) {
    if samples.len() == HISTORY_LIMIT {
        samples.remove(0);
    }
    samples.push(sample);
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_domain::containers::Container;

    fn container(id: u32, cpu: Option<f64>, memory: Option<u64>) -> Container {
        Container {
            id: format!("{id:064x}"),
            state: ContainerState::Running,
            cpu,
            memory,
            ..Default::default()
        }
    }
    fn snapshot(containers: Vec<Container>) -> ContainersSnapshot {
        ContainersSnapshot {
            containers,
            ..Default::default()
        }
    }

    #[test]
    fn independent_metrics_preserve_unknowns_and_mark_non_running_gaps() {
        let mut cache = StatsCache::default();
        let at = Instant::now();
        cache.record(&snapshot(vec![container(1, Some(5.), None)]), at);
        cache.record(
            &snapshot(vec![container(1, None, Some(0))]),
            at + Duration::from_secs(7),
        );
        let mut paused = container(1, Some(90.), Some(80));
        paused.state = ContainerState::Paused;
        cache.record(&snapshot(vec![paused]), at + Duration::from_secs(12));
        cache.record(
            &snapshot(vec![container(1, Some(f64::NAN), Some(9))]),
            at + Duration::from_secs(21),
        );
        cache.record(
            &snapshot(vec![container(1, Some(-1.), None)]),
            at + Duration::from_secs(22),
        );
        cache.record(
            &snapshot(vec![container(1, Some(150.), Some(12))]),
            at + Duration::from_secs(29),
        );
        let history = cache.get(&format!("{:064x}", 1)).unwrap();
        assert_eq!(
            history
                .cpu()
                .iter()
                .map(|sample| (sample.sequence, sample.value))
                .collect::<Vec<_>>(),
            [(1, 5.), (6, 150.)]
        );
        assert_eq!(
            history
                .memory()
                .iter()
                .map(|sample| (sample.sequence, sample.value))
                .collect::<Vec<_>>(),
            [(2, 0), (4, 9), (6, 12)]
        );
        assert_eq!(history_duration_label(history.cpu()), "29s");
        assert_eq!(history_span(history.memory()), Duration::from_secs(22));
    }

    #[test]
    fn keeps_sixty_real_readings_per_metric_and_uses_receipt_time() {
        let mut cache = StatsCache::default();
        let at = Instant::now();
        for value in 0..70 {
            cache.record(
                &snapshot(vec![container(
                    1,
                    Some(value as f64),
                    (value % 2 == 0).then_some(value),
                )]),
                at + Duration::from_secs(value * 7),
            );
        }
        let history = cache.get(&format!("{:064x}", 1)).unwrap();
        assert_eq!(history.cpu().len(), HISTORY_LIMIT);
        assert_eq!(history.cpu()[0].value, 10.);
        assert_eq!(history.cpu()[59].value, 69.);
        assert_eq!(history_span(history.cpu()), Duration::from_secs(59 * 7));
        assert_eq!(history.memory().len(), 35);
        assert_eq!(history_span(history.memory()), Duration::from_secs(68 * 7));
        assert_eq!(history_span::<f64>(&[]), Duration::ZERO);
    }

    #[test]
    fn removes_absent_ids_retains_truncated_ones_and_bounds_identity_cache() {
        let mut cache = StatsCache::default();
        let at = Instant::now();
        for id in 0..(CONTAINER_LIMIT + 10) as u32 {
            let mut reading = snapshot(vec![container(id, Some(1.), None)]);
            reading.truncated = true;
            cache.record(&reading, at);
        }
        assert_eq!(cache.histories.len(), CONTAINER_LIMIT);
        assert!(cache.get(&format!("{:064x}", 0)).is_none());
        cache.record(&snapshot(vec![container(999, Some(2.), Some(3))]), at);
        assert_eq!(cache.histories.len(), 1);
        assert!(cache.get(&format!("{:064x}", 999)).is_some());
        cache.record(&snapshot(vec![]), at);
        assert!(cache.histories.is_empty());
    }

    #[test]
    fn duplicates_and_invalid_ids_cannot_create_duplicate_samples() {
        let mut cache = StatsCache::default();
        let current = container(1, Some(2.), Some(3));
        let mut invalid = current.clone();
        invalid.id = "short-name".into();
        cache.record(
            &snapshot(vec![current.clone(), current, invalid]),
            Instant::now(),
        );
        assert_eq!(cache.histories.len(), 1);
        let history = cache.get(&format!("{:064x}", 1)).unwrap();
        assert_eq!(history.cpu().len(), 1);
        assert_eq!(history.memory().len(), 1);
    }
}
