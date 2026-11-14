//! Typed desktop metrics contracts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Overview,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub sample: crate::monitor::metrics::Snapshot,
    pub sampled_at: Option<i64>,
    pub collection_error: Option<String>,
    pub history_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NumericMeasures {
    pub cpu: Option<f64>,
    pub memory_used: Option<f64>,
    pub memory_total: Option<f64>,
    pub swap_used: Option<f64>,
    pub swap_total: Option<f64>,
    pub load_one: Option<f64>,
    pub load_five: Option<f64>,
    pub load_fifteen: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetworkHistory {
    pub name: String,
    pub received: Option<u64>,
    pub sent: Option<u64>,
    pub rx: Option<f64>,
    pub tx: Option<f64>,
    pub rx_peak: Option<f64>,
    pub tx_peak: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChartBucket {
    pub resolution_ms: u64,
    pub sample_count: u64,
    pub gap: bool,
    pub means: crate::desktop::metrics::NumericMeasures,
    pub peaks: crate::desktop::metrics::NumericMeasures,
    pub network: Vec<crate::desktop::metrics::NetworkHistory>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoricalDisk {
    pub mount: Option<String>,
    pub device: Option<String>,
    pub total: Option<u64>,
    pub used: Option<u64>,
    pub available: Option<i64>,
}

/// Persisted raw history intentionally omits host and process metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoricalRaw {
    /// CPU is required; all other recorded measures may be absent.
    pub measures: NumericMeasures,
    pub uptime: Option<f64>,
    pub network: Vec<NetworkHistory>,
    /// Absence differs from a recorded empty list.
    pub disks: Option<Vec<HistoricalDisk>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MinuteRollup {
    pub summary: ChartBucket,
    pub uptime: Option<f64>,
    pub disks: Option<Vec<HistoricalDisk>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SampleValue {
    Raw(Box<crate::desktop::metrics::HistoricalRaw>),
    Minute(Box<crate::desktop::metrics::MinuteRollup>),
    Chart(Box<crate::desktop::metrics::ChartBucket>),
}

impl SampleValue {
    /// The sample's measures; a bucket's are its means.
    pub fn measures(&self) -> &NumericMeasures {
        match self {
            Self::Raw(raw) => &raw.measures,
            Self::Minute(rollup) => &rollup.summary.means,
            Self::Chart(bucket) => &bucket.means,
        }
    }
    pub fn network(&self) -> &[NetworkHistory] {
        match self {
            Self::Raw(raw) => &raw.network,
            Self::Minute(rollup) => &rollup.summary.network,
            Self::Chart(bucket) => &bucket.network,
        }
    }
    /// How long a bucket spans; a raw sample has none.
    pub fn resolution_ms(&self) -> Option<u64> {
        match self {
            Self::Raw(_) => None,
            Self::Minute(rollup) => Some(rollup.summary.resolution_ms),
            Self::Chart(bucket) => Some(bucket.resolution_ms),
        }
    }
    /// Whether a bucket spans a break in collection.
    pub fn gap(&self) -> bool {
        match self {
            Self::Raw(_) => false,
            Self::Minute(rollup) => rollup.summary.gap,
            Self::Chart(bucket) => bucket.gap,
        }
    }
    /// The host's uptime when recorded; chart buckets don't record it.
    pub fn uptime(&self) -> Option<f64> {
        match self {
            Self::Raw(raw) => raw.uptime,
            Self::Minute(rollup) => rollup.uptime,
            Self::Chart(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub at: i64,
    pub value: crate::desktop::metrics::SampleValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct History {
    pub samples: Vec<crate::desktop::metrics::Sample>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheInfo {
    pub bytes: u64,
    pub samples: u64,
}

#[derive(Clone, Debug)]
pub struct CollectArgs {
    pub server: uuid::Uuid,
    /// Defaults to false when absent on the wire.
    pub refresh: bool,
    pub section: crate::desktop::metrics::Section,
}

#[derive(Clone, Debug)]
pub struct HistoryArgs {
    pub server: uuid::Uuid,
    pub minutes: Option<u32>,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Collect(Box<crate::desktop::metrics::CollectArgs>),
    History(Box<crate::desktop::metrics::HistoryArgs>),
    Cache(Box<crate::common::Present>),
    ClearCache(Box<crate::common::Present>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    Collect(Box<crate::desktop::metrics::Reading>),
    History(Box<crate::desktop::metrics::History>),
    Cache(Box<crate::desktop::metrics::CacheInfo>),
    ClearCache(Box<crate::desktop::metrics::CacheInfo>),
}
