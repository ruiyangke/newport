//! Chart-sized history responses. Stored samples are never changed by this projection.
use super::SavedSample;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

#[derive(Default)]
struct Average {
    sum: f64,
    weight: f64,
    peak: f64,
}
impl Average {
    fn add(&mut self, value: Option<f64>, weight: f64, peak: Option<f64>) {
        if let Some(value) = value.filter(|v| v.is_finite()) {
            self.sum += value * weight;
            self.weight += weight;
            self.peak = self.peak.max(peak.unwrap_or(value));
        }
    }
    fn value(&self) -> Value {
        if self.weight > 0. {
            json!(self.sum / self.weight)
        } else {
            Value::Null
        }
    }
}
#[derive(Default)]
struct Network {
    rx: Average,
    tx: Average,
    invalid: bool,
    received: Value,
    sent: Value,
}
#[derive(Default)]
struct Bucket {
    metrics: BTreeMap<&'static str, Average>,
    loads: [Average; 3],
    network: BTreeMap<String, Network>,
    gap: bool,
    count: u64,
}
fn resolution(data: &Value) -> f64 {
    data["resolutionMs"]
        .as_f64()
        .filter(|v| *v > 0.)
        .unwrap_or(10_000.)
}
impl Bucket {
    fn push(&mut self, sample: &SavedSample, previous: Option<&SavedSample>) {
        let data = &sample.data;
        let weight = resolution(data);
        let elapsed = previous.map(|p| (sample.at - p.at) as f64).unwrap_or(0.);
        let discontinuity = data["gap"] == true
            || previous.is_some_and(|p| {
                elapsed > 30_000f64.max(weight).max(resolution(&p.data))
                    || data["uptime"]
                        .as_f64()
                        .zip(p.data["uptime"].as_f64())
                        .is_some_and(|(a, b)| a < b)
            });
        self.gap |= discontinuity;
        self.count += data["sampleCount"].as_u64().unwrap_or(1);
        for key in ["cpu", "memoryUsed", "memoryTotal", "swapUsed", "swapTotal"] {
            self.metrics.entry(key).or_default().add(
                data[key].as_f64(),
                weight,
                data["peaks"][key].as_f64(),
            );
        }
        for (index, average) in self.loads.iter_mut().enumerate() {
            average.add(
                data["load"][index].as_f64(),
                weight,
                data["peaks"]["load"][index].as_f64(),
            );
        }
        for interface in data["network"].as_array().into_iter().flatten() {
            let Some(name) = interface["name"].as_str() else {
                continue;
            };
            let network = self.network.entry(name.to_owned()).or_default();
            network.received = interface["received"].clone();
            network.sent = interface["sent"].clone();
            let old = previous
                .and_then(|p| p.data["network"].as_array())
                .and_then(|list| list.iter().find(|n| n["name"] == name));
            for (counter, rate, average) in [
                ("received", "rx", &mut network.rx),
                ("sent", "tx", &mut network.tx),
            ] {
                let (value, duration) = if data["resolutionMs"] == 60000 {
                    (interface[rate].as_f64(), weight)
                } else if previous.is_some_and(|p| p.data["resolutionMs"] == 60000)
                    || discontinuity
                    || elapsed <= 0.
                    || elapsed > 30_000.
                {
                    (None, elapsed)
                } else {
                    (
                        old.and_then(|old| {
                            Some(interface[counter].as_f64()? - old[counter].as_f64()?)
                        })
                        .filter(|delta| *delta >= 0.)
                        .map(|delta| delta * 1000. / elapsed),
                        elapsed,
                    )
                };
                // The first raw reading has no rate; a later reset/missing interface
                // must not become a spike or a fabricated zero.
                if value.is_none() && previous.is_some() {
                    network.invalid = true;
                }
                average.add(value, duration, interface[format!("{rate}Peak")].as_f64());
            }
        }
    }
    fn finish(self, interval: i64, at: i64) -> SavedSample {
        let mut data = json!({
            "historyBucket": true, "resolutionMs": interval, "sampleCount": self.count,
            "gap": self.gap, "load": self.loads.iter().map(Average::value).collect::<Vec<_>>(),
            "peaks": {"load": self.loads.iter().map(|v| v.peak).collect::<Vec<_>>()},
            "network": self.network.into_iter().map(|(name,n)| json!({
                "name":name,"received":n.received,"sent":n.sent,
                "rx":if n.invalid { Value::Null } else { n.rx.value() },
                "tx":if n.invalid { Value::Null } else { n.tx.value() },
                "rxPeak":n.rx.peak,"txPeak":n.tx.peak
            })).collect::<Vec<_>>()
        });
        for (key, average) in self.metrics {
            data[key] = average.value();
            data["peaks"][key] = json!(average.peak);
        }
        SavedSample { at, data }
    }
}

pub(super) struct ChartHistory {
    start: i64,
    interval: i64,
    current: Option<(i64, Bucket)>,
    previous: Option<SavedSample>,
    tail: VecDeque<SavedSample>,
    result: Vec<SavedSample>,
}
impl ChartHistory {
    pub(super) fn new(start: i64, minutes: u32) -> Self {
        // Match the overview's 5-minute/day and 30-minute/week chart buckets.
        Self {
            start,
            interval: if minutes <= 1440 { 300_000 } else { 1_800_000 },
            current: None,
            previous: None,
            tail: VecDeque::new(),
            result: Vec::new(),
        }
    }
    pub(super) fn push(&mut self, sample: SavedSample) {
        self.tail.push_back(sample);
        if self.tail.len() <= 2 {
            return;
        }
        let sample = self.tail.pop_front().unwrap();
        let index = (sample.at - self.start) / self.interval;
        if self.current.as_ref().is_some_and(|(i, _)| *i != index) {
            let (index, bucket) = self.current.take().unwrap();
            self.result
                .push(bucket.finish(self.interval, self.start + index * self.interval));
        }
        self.current
            .get_or_insert_with(|| (index, Bucket::default()))
            .1
            .push(&sample, self.previous.as_ref());
        self.previous = Some(sample);
    }
    pub(super) fn finish(mut self) -> Vec<SavedSample> {
        if let Some((index, bucket)) = self.current {
            self.result
                .push(bucket.finish(self.interval, self.start + index * self.interval));
        }
        // Keep the last pair unmodified for the live network-rate card.
        self.result.extend(self.tail);
        self.result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(at: i64, cpu: f64, counter: i64) -> SavedSample {
        SavedSample {
            at,
            data: json!({"cpu":cpu,"memoryUsed":20,"memoryTotal":100,
            "load":[1.,2.,3.],"uptime":at / 1000,
            "network":[{"name":"eth0","received":counter,"sent":counter}]}),
        }
    }
    #[test]
    fn week_is_bounded_and_preserves_range_averages_peaks_and_live_pair() {
        let mut history = ChartHistory::new(0, 10080);
        // A full week at three-second intervals exceeds the transport limit when
        // every reading includes a host's many virtual network interfaces.
        let mut last = 0;
        for at in (0..604_800_000).step_by(3000) {
            history.push(sample(at, if at == 3000 { 100. } else { 10. }, at));
            last = at;
        }
        let result = history.finish();
        assert!(result.len() <= 338);
        assert!(result[0].at < 1_800_000);
        assert_eq!(result.last().unwrap().at, last);
        assert_eq!(result[result.len() - 2].at, last - 3000);
        assert!(result.last().unwrap().data["historyBucket"].is_null());
        assert_eq!(result[0].data["peaks"]["cpu"], 100.);
        assert!((result[0].data["cpu"].as_f64().unwrap() - 10.15).abs() < 0.001);
        assert_eq!(result[0].data["network"][0]["rx"], 1000.);
        assert_eq!(result[0].data["gap"], false);
        assert!(rmp_serde::to_vec_named(&result).unwrap().len() < 256 * 1024);
    }
    #[test]
    fn summaries_are_duration_weighted_and_keep_existing_peaks_and_rates() {
        let mut history = ChartHistory::new(0, 1440);
        let mut summary = sample(0, 30., 0);
        summary.data["resolutionMs"] = json!(60000);
        summary.data["peaks"] = json!({"cpu":90.});
        summary.data["network"][0]["rx"] = json!(200.);
        summary.data["network"][0]["tx"] = json!(300.);
        history.push(summary);
        history.push(sample(60000, 10., 60000));
        history.push(sample(70000, 10., 70000));
        history.push(sample(80000, 10., 80000));
        let rows = history.finish();
        assert!((rows[0].data["cpu"].as_f64().unwrap() - 190. / 7.).abs() < 0.001);
        assert_eq!(rows[0].data["peaks"]["cpu"], 90.);
        // Never derive a counter delta from a minute summary to a raw sample.
        assert!(rows[0].data["network"][0]["rx"].is_null());
    }
    #[test]
    fn resets_gaps_and_missing_interfaces_do_not_create_network_spikes() {
        for second in [
            sample(10000, 10., 1),
            sample(90000, 10., 90000),
            SavedSample {
                at: 10000,
                data: json!({"cpu":10,"network":[{"name":"new0","received":999999,"sent":999999}]}),
            },
        ] {
            let mut history = ChartHistory::new(0, 1440);
            history.push(sample(0, 10., 100));
            history.push(second);
            history.push(sample(200000, 10., 200000));
            history.push(sample(210000, 10., 210000));
            let result = history.finish();
            assert!(
                result[0].data["network"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|n| n["rx"].is_null())
            );
        }
    }
    #[test]
    fn empty_intervals_stay_empty_and_latest_pair_stays_exact() {
        let mut history = ChartHistory::new(0, 10080);
        for at in [0, 3 * 86400000, 604790000, 604799000] {
            history.push(sample(at, 10., at));
        }
        let rows = history.finish();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].at, 3 * 86400000);
        assert_eq!(rows[1].data["gap"], true);
        assert_eq!(rows[3].data, sample(604799000, 10., 604799000).data);
    }
}
