//! Flattened chart/read models preserve optional observations and signed dates.
use super::monitor_metrics as sample;
use newport_protocol::desktop::metrics::*;
use serde_json::{Value, json};
fn measures(value: NumericMeasures) -> Value {
    let mut body = json!({"cpu":value.cpu,"memoryUsed":value.memory_used,"memoryTotal":value.memory_total,"swapUsed":value.swap_used,"swapTotal":value.swap_total,"load":[value.load_one,value.load_five,value.load_fifteen]});
    if let Some(fields) = body.as_object_mut() {
        fields.retain(|_, value| !value.is_null());
    }
    body
}
fn bucket(value: ChartBucket) -> Value {
    let mut body = measures(value.means);
    body["resolutionMs"] = json!(value.resolution_ms);
    body["sampleCount"] = json!(value.sample_count);
    body["gap"] = json!(value.gap);
    body["peaks"] = measures(value.peaks);
    body["network"] = Value::Array(value.network.into_iter().map(|row| {
        let mut body=json!({"name":row.name,"rx":row.rx,"tx":row.tx,"rxPeak":row.rx_peak,"txPeak":row.tx_peak});
        if let Some(value)=row.received { body["received"]=json!(value); }
        if let Some(value)=row.sent { body["sent"]=json!(value); }
        body
    }).collect());
    body
}
fn reading(value: Reading) -> Value {
    let mut body = sample::snapshot(value.sample);
    body["sampledAt"] = json!(value.sampled_at);
    body["collectionError"] = json!(value.collection_error);
    body["historyError"] = json!(value.history_error);
    body
}
fn history_network(value: Vec<NetworkHistory>) -> Value {
    Value::Array(
        value
            .into_iter()
            .map(|row| {
                let mut body = json!({"name":row.name});
                for (key, value) in [
                    ("received", row.received.map(|v| json!(v))),
                    ("sent", row.sent.map(|v| json!(v))),
                    ("rx", row.rx.map(|v| json!(v))),
                    ("tx", row.tx.map(|v| json!(v))),
                    ("rxPeak", row.rx_peak.map(|v| json!(v))),
                    ("txPeak", row.tx_peak.map(|v| json!(v))),
                ] {
                    if let Some(value) = value {
                        body[key] = value;
                    }
                }
                body
            })
            .collect(),
    )
}
fn historical_optional(body: &mut Value, uptime: Option<f64>, disks: Option<Vec<HistoricalDisk>>) {
    if let Some(uptime) = uptime {
        body["uptime"] = json!(uptime);
    }
    if let Some(disks) = disks {
        body["disks"] = Value::Array(
            disks
                .into_iter()
                .map(|disk| {
                    let mut body = json!({});
                    for (key, value) in [
                        ("mount", disk.mount.map(|v| json!(v))),
                        ("device", disk.device.map(|v| json!(v))),
                        ("total", disk.total.map(|v| json!(v))),
                        ("used", disk.used.map(|v| json!(v))),
                        ("available", disk.available.map(|v| json!(v))),
                    ] {
                        if let Some(value) = value {
                            body[key] = value;
                        }
                    }
                    body
                })
                .collect(),
        );
    }
}
fn historical(value: HistoricalRaw) -> Value {
    let mut body = measures(value.measures);
    if let Some(fields) = body.as_object_mut() {
        fields.retain(|_, value| !value.is_null());
    }
    if body["load"]
        .as_array()
        .is_some_and(|items| items.iter().all(Value::is_null))
        && let Some(fields) = body.as_object_mut()
    {
        fields.remove("load");
    }
    body["network"] = history_network(value.network);
    historical_optional(&mut body, value.uptime, value.disks);
    body
}
fn saved(value: Sample) -> Value {
    let body = match value.value {
        SampleValue::Raw(value) => historical(*value),
        SampleValue::Chart(value) => {
            let mut body = bucket(*value);
            body["historyBucket"] = json!(true);
            body
        }
        SampleValue::Minute(value) => {
            let mut body = bucket(value.summary);
            historical_optional(&mut body, value.uptime, value.disks);
            body
        }
    };
    json!({"at":value.at,"data":body})
}
pub(super) fn project(reply: Reply) -> Value {
    match reply {
        Reply::Collect(value) => reading(*value),
        Reply::History(value) => Value::Array(value.samples.into_iter().map(saved).collect()),
        Reply::Cache(value) | Reply::ClearCache(value) => {
            json!({"bytes":value.bytes,"samples":value.samples})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn measures() -> NumericMeasures {
        NumericMeasures {
            cpu: Some(0.),
            memory_used: None,
            memory_total: None,
            swap_used: None,
            swap_total: None,
            load_one: None,
            load_five: None,
            load_fifteen: None,
        }
    }
    #[test]
    fn history_preserves_absent_empty_signed_and_rollup_observations() {
        let raw = historical(HistoricalRaw {
            measures: measures(),
            uptime: None,
            network: vec![],
            disks: None,
        });
        assert_eq!(raw, json!({"cpu":0.,"network":[]}));
        let raw = historical(HistoricalRaw {
            measures: measures(),
            uptime: Some(0.),
            network: vec![],
            disks: Some(vec![]),
        });
        assert_eq!(raw, json!({"cpu":0.,"uptime":0.,"network":[],"disks":[]}));
        let raw = historical(HistoricalRaw {
            measures: measures(),
            uptime: None,
            network: vec![],
            disks: Some(vec![HistoricalDisk {
                mount: None,
                device: Some(String::new()),
                total: Some(0),
                used: None,
                available: Some(-1),
            }]),
        });
        assert_eq!(
            raw["disks"],
            json!([{"device":"","total":0,"available":-1}])
        );
        let chart = bucket(ChartBucket {
            resolution_ms: 60_000,
            sample_count: 1,
            gap: false,
            means: measures(),
            peaks: measures(),
            network: vec![NetworkHistory {
                name: "eth0".into(),
                received: None,
                sent: Some(0),
                rx: None,
                tx: None,
                rx_peak: None,
                tx_peak: None,
            }],
        });
        assert!(chart.get("memoryUsed").is_none());
        assert_eq!(chart["load"], json!([null, null, null]));
        assert_eq!(
            chart["network"],
            json!([{"name":"eth0","sent":0,"rx":null,"tx":null,"rxPeak":null,"txPeak":null}])
        );
        let saved = saved(Sample {
            at: -1,
            value: SampleValue::Raw(Box::new(HistoricalRaw {
                measures: measures(),
                uptime: None,
                network: vec![],
                disks: None,
            })),
        });
        assert_eq!(saved["at"], -1);
        assert!(saved["data"].get("hostname").is_none());
    }
}
