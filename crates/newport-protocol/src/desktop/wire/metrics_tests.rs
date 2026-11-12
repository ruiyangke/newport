use super::*;
use crate::desktop::metrics::{
    ChartBucket, HistoricalDisk, HistoricalRaw, MinuteRollup, NumericMeasures,
};

fn measures() -> NumericMeasures {
    NumericMeasures {
        cpu: Some(42.0),
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
fn minimal_persisted_raw_history_needs_no_host_metadata() {
    let value = HistoricalRaw {
        measures: measures(),
        uptime: None,
        network: Vec::new(),
        disks: None,
    };
    value.validate().unwrap();
    let decoded = HistoricalRaw::from_proto(value.to_proto().unwrap()).unwrap();
    assert_eq!(decoded.measures.cpu, Some(42.0));
    assert!(decoded.network.is_empty());
    assert!(decoded.disks.is_none());
    assert!(decoded.uptime.is_none());
}

#[test]
fn history_preserves_absent_empty_and_partial_disks() {
    for disks in [
        None,
        Some(Vec::new()),
        Some(vec![HistoricalDisk {
            mount: None,
            device: None,
            total: None,
            used: Some(7),
            available: Some(-1),
        }]),
    ] {
        let value = HistoricalRaw {
            measures: measures(),
            uptime: None,
            network: Vec::new(),
            disks,
        };
        let decoded = HistoricalRaw::from_proto(value.to_proto().unwrap()).unwrap();
        assert_eq!(
            decoded.disks.as_ref().map(Vec::len),
            value.disks.as_ref().map(Vec::len)
        );
        if let Some(disk) = decoded.disks.as_ref().and_then(|v| v.first()) {
            assert_eq!(disk.used, Some(7));
            assert_eq!(disk.available, Some(-1));
            assert!(disk.mount.is_none());
            assert!(disk.total.is_none());
        }
    }
}

#[test]
fn missing_network_or_cpu_cannot_become_valid_raw_history() {
    let value = HistoricalRaw {
        measures: measures(),
        uptime: None,
        network: Vec::new(),
        disks: None,
    };
    let mut encoded = value.to_proto().unwrap();
    encoded.network = None;
    assert!(HistoricalRaw::from_proto(encoded).is_err());
    let mut value = value;
    value.measures.cpu = None;
    assert!(value.validate().is_err());
    value.measures.cpu = Some(f64::NAN);
    assert!(value.validate().is_err());
}

#[test]
fn minute_rollup_preserves_only_recorded_history_fields() {
    let value = MinuteRollup {
        summary: ChartBucket {
            resolution_ms: 60_000,
            sample_count: 1,
            gap: true,
            means: measures(),
            peaks: measures(),
            network: Vec::new(),
        },
        uptime: None,
        disks: Some(Vec::new()),
    };
    value.validate().unwrap();
    let decoded = MinuteRollup::from_proto(value.to_proto().unwrap()).unwrap();
    assert_eq!(decoded.summary.resolution_ms, 60_000);
    assert_eq!(decoded.summary.sample_count, 1);
    assert!(decoded.summary.gap);
    assert!(decoded.uptime.is_none());
    assert_eq!(decoded.disks.unwrap().len(), 0);
}
