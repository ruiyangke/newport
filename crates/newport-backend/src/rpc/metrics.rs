//! Metrics projections retain missing samples and fractional rollups without
//! altering the persisted database schema.
use super::{
    failure::{invalid_result, operation},
    features::Services,
    projection as p,
};
use newport_protocol::{
    desktop::{Failure, metrics as dto},
    monitor::metrics as sample,
};
use serde_json::Value;
fn small(value: &Value) -> Result<u32, Failure> {
    u32::try_from(p::unsigned(value)?).map_err(|_| invalid_result("u32"))
}
fn optional_number(value: Option<&Value>) -> Result<Option<f64>, Failure> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => p::number(value).map(Some),
    }
}
fn core(value: &Value) -> Result<sample::Core, Failure> {
    Ok(sample::Core {
        id: small(p::field(value, "id")?)?,
        usage: p::number(p::field(value, "usage")?)?,
        history: p::optional(value, "history", |value| p::list(value, p::number))?
            .unwrap_or_default(),
    })
}
fn disk(value: &Value) -> Result<sample::Disk, Failure> {
    Ok(sample::Disk {
        mount: p::text(p::field(value, "mount")?)?,
        device: p::text(p::field(value, "device")?)?,
        total: p::unsigned(p::field(value, "total")?)?,
        used: p::unsigned(p::field(value, "used")?)?,
        available: p::signed(p::field(value, "available")?)?,
    })
}
fn network(value: &Value) -> Result<sample::Network, Failure> {
    Ok(sample::Network {
        name: p::text(p::field(value, "name")?)?,
        received: p::unsigned(p::field(value, "received")?)?,
        sent: p::unsigned(p::field(value, "sent")?)?,
    })
}
fn process(value: &Value) -> Result<sample::Process, Failure> {
    Ok(sample::Process {
        pid: small(p::field(value, "pid")?)?,
        user: p::text(p::field(value, "user")?)?,
        cpu: p::number(p::field(value, "cpu")?)?,
        memory: p::unsigned(p::field(value, "memory")?)?,
        state: p::text(p::field(value, "state")?)?,
        name: p::text(p::field(value, "name")?)?,
        elapsed: p::optional(value, "elapsed", p::unsigned)?,
        command: p::optional(value, "command", p::text)?,
    })
}
fn snapshot(value: &Value) -> Result<sample::Snapshot, Failure> {
    Ok(sample::Snapshot {
        hostname: p::text(p::field(value, "hostname")?)?,
        os: p::text(p::field(value, "os")?)?,
        kernel: p::text(p::field(value, "kernel")?)?,
        cores: small(p::field(value, "cores")?)?,
        uptime: p::number(p::field(value, "uptime")?)?,
        load: p::list(p::field(value, "load")?, p::number)?
            .try_into()
            .map_err(|_| invalid_result("load array"))?,
        cpu: p::number(p::field(value, "cpu")?)?,
        cpus: p::list(p::field(value, "cpus")?, core)?,
        memory_total: p::unsigned(p::field(value, "memoryTotal")?)?,
        memory_used: p::unsigned(p::field(value, "memoryUsed")?)?,
        swap_total: p::unsigned(p::field(value, "swapTotal")?)?,
        swap_used: p::unsigned(p::field(value, "swapUsed")?)?,
        disks: p::list(p::field(value, "disks")?, disk)?,
        network: p::list(p::field(value, "network")?, network)?,
        processes: p::list(p::field(value, "processes")?, process)?,
        process_count: small(p::field(value, "processCount")?)?,
    })
}
fn reading(value: &Value) -> Result<dto::Reading, Failure> {
    Ok(dto::Reading {
        sample: snapshot(value)?,
        sampled_at: p::optional(value, "sampledAt", p::signed)?,
        collection_error: p::optional(value, "collectionError", p::text)?,
        history_error: p::optional(value, "historyError", p::text)?,
    })
}
fn measures(value: &Value) -> Result<dto::NumericMeasures, Failure> {
    let load = match value.get("load") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let rows = value
                .as_array()
                .filter(|rows| rows.len() == 3)
                .ok_or_else(|| invalid_result("load measures"))?;
            Some(rows)
        }
    };
    Ok(dto::NumericMeasures {
        cpu: p::optional(value, "cpu", p::number)?,
        memory_used: p::optional(value, "memoryUsed", p::number)?,
        memory_total: p::optional(value, "memoryTotal", p::number)?,
        swap_used: p::optional(value, "swapUsed", p::number)?,
        swap_total: p::optional(value, "swapTotal", p::number)?,
        load_one: optional_number(load.and_then(|load| load.first()))?,
        load_five: optional_number(load.and_then(|load| load.get(1)))?,
        load_fifteen: optional_number(load.and_then(|load| load.get(2)))?,
    })
}
fn bucket(value: &Value) -> Result<dto::ChartBucket, Failure> {
    Ok(dto::ChartBucket {
        resolution_ms: p::unsigned(p::field(value, "resolutionMs")?)?,
        sample_count: p::unsigned(p::field(value, "sampleCount")?)?,
        gap: p::bool_value(p::field(value, "gap")?)?,
        means: measures(value)?,
        peaks: measures(p::field(value, "peaks")?)?,
        network: p::list(p::field(value, "network")?, |value| {
            Ok(dto::NetworkHistory {
                name: p::text(p::field(value, "name")?)?,
                received: p::optional(value, "received", p::unsigned)?,
                sent: p::optional(value, "sent", p::unsigned)?,
                rx: p::optional(value, "rx", p::number)?,
                tx: p::optional(value, "tx", p::number)?,
                rx_peak: p::optional(value, "rxPeak", p::number)?,
                tx_peak: p::optional(value, "txPeak", p::number)?,
            })
        })?,
    })
}
fn historical_disk(value: &Value) -> Result<dto::HistoricalDisk, Failure> {
    Ok(dto::HistoricalDisk {
        mount: p::optional(value, "mount", p::text)?,
        device: p::optional(value, "device", p::text)?,
        total: p::optional(value, "total", p::unsigned)?,
        used: p::optional(value, "used", p::unsigned)?,
        available: p::optional(value, "available", p::signed)?,
    })
}
fn historical_network(value: &Value) -> Result<dto::NetworkHistory, Failure> {
    Ok(dto::NetworkHistory {
        name: p::text(p::field(value, "name")?)?,
        received: p::optional(value, "received", p::unsigned)?,
        sent: p::optional(value, "sent", p::unsigned)?,
        rx: p::optional(value, "rx", p::number)?,
        tx: p::optional(value, "tx", p::number)?,
        rx_peak: p::optional(value, "rxPeak", p::number)?,
        tx_peak: p::optional(value, "txPeak", p::number)?,
    })
}
fn saved(value: crate::metrics_db::SavedSample) -> Result<dto::Sample, Failure> {
    let data = &value.data;
    let body = if p::optional(data, "historyBucket", p::bool_value)?.unwrap_or(false) {
        dto::SampleValue::Chart(Box::new(bucket(data)?))
    } else if data.get("resolutionMs").is_some() {
        dto::SampleValue::Minute(Box::new(dto::MinuteRollup {
            summary: bucket(data)?,
            uptime: p::optional(data, "uptime", p::number)?,
            disks: p::optional(data, "disks", |value| p::list(value, historical_disk))?,
        }))
    } else {
        dto::SampleValue::Raw(Box::new(dto::HistoricalRaw {
            measures: measures(data)?,
            uptime: p::optional(data, "uptime", p::number)?,
            network: p::list(p::field(data, "network")?, historical_network)?,
            disks: p::optional(data, "disks", |value| p::list(value, historical_disk))?,
        }))
    };
    Ok(dto::Sample {
        at: value.at,
        value: body,
    })
}
fn cache(value: crate::metrics_db::MetricsCache) -> dto::CacheInfo {
    dto::CacheInfo {
        bytes: value.bytes,
        samples: value.samples,
    }
}
pub async fn dispatch(
    command: &dto::Operation,
    state: &Services<'_>,
) -> Result<dto::Reply, Failure> {
    use dto::{Operation as O, Reply as R};
    Ok(match command {
        O::Collect(args) => R::Collect(Box::new(reading(
            &crate::commands::cockpit_collect(
                state.manager,
                state.metrics,
                Some(args.refresh),
                args.server,
                match args.section {
                    dto::Section::Overview => crate::cockpit::Section::Overview,
                },
            )
            .await
            .map_err(operation)?,
        )?)),
        O::History(args) => R::History(Box::new(dto::History {
            samples: crate::commands::cockpit_history(
                state.manager,
                state.metrics,
                args.server,
                args.minutes,
            )
            .await
            .map_err(operation)?
            .into_iter()
            .map(saved)
            .collect::<Result<_, _>>()?,
        })),
        O::Cache(_) => R::Cache(Box::new(cache(
            crate::commands::get_metrics_cache(state.metrics)
                .await
                .map_err(operation)?,
        ))),
        O::ClearCache(_) => R::ClearCache(Box::new(cache(
            crate::commands::clear_metrics_cache(state.manager, state.metrics)
                .await
                .map_err(operation)?,
        ))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn stored_history_preserves_missing_measurements_and_partial_disk_rows() {
        let value = saved(crate::metrics_db::SavedSample {
            at: 1,
            data: json!({"cpu":42,"network":[],"disks":[{"used":0}]}),
        })
        .unwrap();
        let dto::SampleValue::Raw(value) = value.value else {
            panic!("raw history")
        };
        assert_eq!(value.measures.cpu, Some(42.));
        assert_eq!(value.measures.memory_total, None);
        assert_eq!(value.uptime, None);
        let disk = &value.disks.unwrap()[0];
        assert_eq!(disk.used, Some(0));
        assert_eq!(disk.total, None);
        assert_eq!(disk.mount, None);
    }
    #[test]
    fn minute_history_keeps_fractional_means_null_load_and_empty_disks() {
        let value = saved(crate::metrics_db::SavedSample { at: 60_000, data: json!({"cpu":12.5,"memoryUsed":3.5,"network":[],"disks":[],"load":[null,1.25,null],"resolutionMs":60000,"sampleCount":2,"gap":true,"peaks":{"cpu":20,"memoryUsed":4,"load":[null,2,null]}}) }).unwrap();
        let dto::SampleValue::Minute(value) = value.value else {
            panic!("minute history")
        };
        assert_eq!(value.summary.means.memory_used, Some(3.5));
        assert_eq!(value.summary.means.load_one, None);
        assert_eq!(value.summary.means.load_five, Some(1.25));
        assert!(value.summary.gap);
        assert!(value.disks.is_some_and(|rows| rows.is_empty()));
        assert_eq!(value.uptime, None);
    }
}
