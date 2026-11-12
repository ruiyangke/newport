use super::*;
impl Convert for crate::desktop::metrics::Section {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Overview => 1,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Overview,
            _ => return Err(Invalid("Section").into()),
        })
    }
}

impl Convert for crate::desktop::metrics::Reading {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::Reading;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::metrics::Reading {
                sample: Some(self.sample.to_proto()?),
                sampled_at: self
                    .sampled_at
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                collection_error: self
                    .collection_error
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                history_error: self
                    .history_error
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            sample: <crate::monitor::metrics::Snapshot as Convert>::from_proto(required(
                value.sample,
            )?)?,
            sampled_at: value
                .sampled_at
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            collection_error: value
                .collection_error
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            history_error: value
                .history_error
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::metrics::NumericMeasures {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::NumericMeasures;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::metrics::NumericMeasures {
                cpu: self
                    .cpu
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                memory_used: self
                    .memory_used
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                memory_total: self
                    .memory_total
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                swap_used: self
                    .swap_used
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                swap_total: self
                    .swap_total
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                load_one: self
                    .load_one
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                load_five: self
                    .load_five
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                load_fifteen: self
                    .load_fifteen
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            cpu: value
                .cpu
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            memory_used: value
                .memory_used
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            memory_total: value
                .memory_total
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            swap_used: value
                .swap_used
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            swap_total: value
                .swap_total
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            load_one: value
                .load_one
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            load_five: value
                .load_five
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            load_fifteen: value
                .load_fifteen
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::metrics::NetworkHistory {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::NetworkHistory;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::metrics::NetworkHistory {
                name: Some(self.name.clone()),
                received: self
                    .received
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                sent: self
                    .sent
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                rx: self
                    .rx
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                tx: self
                    .tx
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                rx_peak: self
                    .rx_peak
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                tx_peak: self
                    .tx_peak
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            name: required(value.name)?,
            received: value
                .received
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            sent: value
                .sent
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            rx: value
                .rx
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            tx: value
                .tx
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            rx_peak: value
                .rx_peak
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            tx_peak: value
                .tx_peak
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::metrics::ChartBucket {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::ChartBucket;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::metrics::ChartBucket {
                resolution_ms: Some(self.resolution_ms),
                sample_count: Some(self.sample_count),
                gap: Some(self.gap),
                means: Some(self.means.to_proto()?),
                peaks: Some(self.peaks.to_proto()?),
                network: self
                    .network
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            resolution_ms: required(value.resolution_ms)?,
            sample_count: required(value.sample_count)?,
            gap: required(value.gap)?,
            means: <crate::desktop::metrics::NumericMeasures as Convert>::from_proto(required(
                value.means,
            )?)?,
            peaks: <crate::desktop::metrics::NumericMeasures as Convert>::from_proto(required(
                value.peaks,
            )?)?,
            network: value
                .network
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::metrics::NetworkHistory as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::metrics::HistoricalDisk {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::HistoricalDisk;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(Self::Proto {
            mount: self.mount.clone(),
            device: self.device.clone(),
            total: self.total,
            used: self.used,
            available: self.available,
        })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            mount: value.mount,
            device: value.device,
            total: value.total,
            used: value.used,
            available: value.available,
        })
    }
}

fn historical_disks_to_proto(
    value: &Option<Vec<crate::desktop::metrics::HistoricalDisk>>,
) -> Result<
    Option<crate::encoding::protobuf::generated::newport::desktop::metrics::HistoricalDisks>,
    Error,
> {
    value
        .as_ref()
        .map(|items| {
            Ok(
                crate::encoding::protobuf::generated::newport::desktop::metrics::HistoricalDisks {
                    items: items
                        .iter()
                        .map(Convert::to_proto)
                        .collect::<Result<_, _>>()?,
                },
            )
        })
        .transpose()
}

fn historical_disks_from_proto(
    value: Option<crate::encoding::protobuf::generated::newport::desktop::metrics::HistoricalDisks>,
) -> Result<Option<Vec<crate::desktop::metrics::HistoricalDisk>>, Error> {
    value
        .map(|list| {
            list.items
                .into_iter()
                .map(crate::desktop::metrics::HistoricalDisk::from_proto)
                .collect()
        })
        .transpose()
}

impl Convert for crate::desktop::metrics::HistoricalRaw {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::HistoricalRaw;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(Self::Proto {
            measures: Some(self.measures.to_proto()?),
            uptime: self.uptime,
            network: Some(crate::encoding::protobuf::generated::newport::desktop::metrics::HistoricalNetwork {
                items: self.network.iter().map(Convert::to_proto).collect::<Result<_, _>>()?,
            }),
            disks: historical_disks_to_proto(&self.disks)?,
        })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            measures: crate::desktop::metrics::NumericMeasures::from_proto(required(
                value.measures,
            )?)?,
            uptime: value.uptime,
            network: required(value.network)?
                .items
                .into_iter()
                .map(crate::desktop::metrics::NetworkHistory::from_proto)
                .collect::<Result<_, _>>()?,
            disks: historical_disks_from_proto(value.disks)?,
        })
    }
}

impl Convert for crate::desktop::metrics::MinuteRollup {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::MinuteRollup;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(Self::Proto {
            summary: Some(self.summary.to_proto()?),
            uptime: self.uptime,
            disks: historical_disks_to_proto(&self.disks)?,
        })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            summary: crate::desktop::metrics::ChartBucket::from_proto(required(value.summary)?)?,
            uptime: value.uptime,
            disks: historical_disks_from_proto(value.disks)?,
        })
    }
}

impl Convert for crate::desktop::metrics::SampleValue {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::SampleValue;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::metrics::SampleValue { value: Some(match self { crate::desktop::metrics::SampleValue::Raw(v) => crate::encoding::protobuf::generated::newport::desktop::metrics::sample_value::Value::Raw(Box::new(v.as_ref().to_proto()?)), crate::desktop::metrics::SampleValue::Minute(v) => crate::encoding::protobuf::generated::newport::desktop::metrics::sample_value::Value::Minute(Box::new(v.as_ref().to_proto()?)), crate::desktop::metrics::SampleValue::Chart(v) => crate::encoding::protobuf::generated::newport::desktop::metrics::sample_value::Value::Chart(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::metrics::sample_value::Value::Raw(v) => crate::desktop::metrics::SampleValue::Raw(Box::new(<crate::desktop::metrics::HistoricalRaw as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::metrics::sample_value::Value::Minute(v) => crate::desktop::metrics::SampleValue::Minute(Box::new(<crate::desktop::metrics::MinuteRollup as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::metrics::sample_value::Value::Chart(v) => crate::desktop::metrics::SampleValue::Chart(Box::new(<crate::desktop::metrics::ChartBucket as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::metrics::Sample {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::Sample;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::metrics::Sample {
                at: Some(self.at),
                value: Some(self.value.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            at: required(value.at)?,
            value: <crate::desktop::metrics::SampleValue as Convert>::from_proto(required(
                value.value,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::metrics::History {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::History;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::metrics::History {
                samples: self
                    .samples
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            samples: value
                .samples
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::metrics::Sample as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::metrics::CacheInfo {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::CacheInfo;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::metrics::CacheInfo {
                bytes: Some(self.bytes),
                samples: Some(self.samples),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            bytes: required(value.bytes)?,
            samples: required(value.samples)?,
        })
    }
}

impl Convert for crate::desktop::metrics::CollectArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::CollectArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::metrics::CollectArgs {
                server: Some(self.server.to_string()),
                refresh: Some(self.refresh),
                section: Some(self.section.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            refresh: value.refresh.unwrap_or(false),
            section: <crate::desktop::metrics::Section as Convert>::from_proto(required(
                value.section,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::metrics::HistoryArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::HistoryArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::metrics::HistoryArgs {
                server: Some(self.server.to_string()),
                minutes: self
                    .minutes
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            minutes: value
                .minutes
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::metrics::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::metrics::Operation { value: Some(match self { crate::desktop::metrics::Operation::Collect(v) => crate::encoding::protobuf::generated::newport::desktop::metrics::operation::Value::Collect(Box::new(v.as_ref().to_proto()?)), crate::desktop::metrics::Operation::History(v) => crate::encoding::protobuf::generated::newport::desktop::metrics::operation::Value::History(Box::new(v.as_ref().to_proto()?)), crate::desktop::metrics::Operation::Cache(v) => crate::encoding::protobuf::generated::newport::desktop::metrics::operation::Value::Cache(Box::new(v.as_ref().to_proto()?)), crate::desktop::metrics::Operation::ClearCache(v) => crate::encoding::protobuf::generated::newport::desktop::metrics::operation::Value::ClearCache(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::metrics::operation::Value::Collect(v) => crate::desktop::metrics::Operation::Collect(Box::new(<crate::desktop::metrics::CollectArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::metrics::operation::Value::History(v) => crate::desktop::metrics::Operation::History(Box::new(<crate::desktop::metrics::HistoryArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::metrics::operation::Value::Cache(v) => crate::desktop::metrics::Operation::Cache(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::metrics::operation::Value::ClearCache(v) => crate::desktop::metrics::Operation::ClearCache(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::metrics::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::metrics::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::metrics::Reply { value: Some(match self { crate::desktop::metrics::Reply::Collect(v) => crate::encoding::protobuf::generated::newport::desktop::metrics::reply::Value::Collect(Box::new(v.as_ref().to_proto()?)), crate::desktop::metrics::Reply::History(v) => crate::encoding::protobuf::generated::newport::desktop::metrics::reply::Value::History(Box::new(v.as_ref().to_proto()?)), crate::desktop::metrics::Reply::Cache(v) => crate::encoding::protobuf::generated::newport::desktop::metrics::reply::Value::Cache(Box::new(v.as_ref().to_proto()?)), crate::desktop::metrics::Reply::ClearCache(v) => crate::encoding::protobuf::generated::newport::desktop::metrics::reply::Value::ClearCache(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::metrics::reply::Value::Collect(v) => crate::desktop::metrics::Reply::Collect(Box::new(<crate::desktop::metrics::Reading as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::metrics::reply::Value::History(v) => crate::desktop::metrics::Reply::History(Box::new(<crate::desktop::metrics::History as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::metrics::reply::Value::Cache(v) => crate::desktop::metrics::Reply::Cache(Box::new(<crate::desktop::metrics::CacheInfo as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::metrics::reply::Value::ClearCache(v) => crate::desktop::metrics::Reply::ClearCache(Box::new(<crate::desktop::metrics::CacheInfo as Convert>::from_proto(*v)?)) })
    }
}

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod tests;
