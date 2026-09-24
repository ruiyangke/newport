//! Atomic, bounded downsampling. Raw readings are immutable once summarized.
use super::{now_ms, MetricsDb, RETENTION_MS};
use serde_json::{json, Value};
use sqlx::Connection;
use std::time::Duration;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const MINUTE_MS: i64 = 60_000;
const BATCH: i64 = 16;

fn summarize(rows: &[(i64, String)]) -> Result<Value, String> {
    let samples: Vec<(i64, Value)> = rows
        .iter()
        .map(|(at, data)| {
            serde_json::from_str(data)
                .map(|value| (*at, value))
                .map_err(|e| format!("Invalid stored metric: {e}"))
        })
        .collect::<Result<_, _>>()?;
    let mut result = samples.last().ok_or("Empty metric bucket")?.1.clone();
    let mut peaks = serde_json::Map::new();
    for key in ["cpu", "memoryUsed", "memoryTotal", "swapUsed", "swapTotal"] {
        let values: Vec<f64> = samples
            .iter()
            .filter_map(|(_, s)| s[key].as_f64())
            .collect();
        if !values.is_empty() {
            result[key] = json!(values.iter().sum::<f64>() / values.len() as f64);
            peaks.insert(
                key.into(),
                json!(values.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
            );
        }
    }
    let mut loads = Vec::new();
    let mut load_peaks = Vec::new();
    for index in 0..3 {
        let values: Vec<f64> = samples
            .iter()
            .filter_map(|(_, s)| s["load"][index].as_f64())
            .collect();
        loads.push(if values.is_empty() {
            Value::Null
        } else {
            json!(values.iter().sum::<f64>() / values.len() as f64)
        });
        load_peaks.push(if values.is_empty() {
            Value::Null
        } else {
            json!(values.iter().copied().fold(f64::NEG_INFINITY, f64::max))
        });
    }
    result["load"] = json!(loads);
    peaks.insert("load".into(), json!(load_peaks));
    // Keep final counters for inspection; explicit rates never bridge a reset or gap.
    if let Some(interfaces) = result["network"].as_array_mut() {
        for interface in interfaces {
            let name = interface["name"].as_str().unwrap_or("").to_owned();
            for (counter, rate) in [("received", "rx"), ("sent", "tx")] {
                let mut bytes = 0.0;
                let mut seconds = 0.0;
                let mut peak: f64 = 0.0;
                let mut invalid = false;
                for pair in samples.windows(2) {
                    let elapsed = (pair[1].0 - pair[0].0) as f64 / 1000.0;
                    let value = |sample: &Value| {
                        sample["network"]
                            .as_array()?
                            .iter()
                            .find(|n| n["name"].as_str() == Some(&name))?[counter]
                            .as_f64()
                    };
                    match (value(&pair[0].1), value(&pair[1].1)) {
                        (Some(a), Some(b))
                            if elapsed > 0.0
                                && elapsed <= 30.0
                                && b >= a
                                && pair[1].1["uptime"]
                                    .as_f64()
                                    .zip(pair[0].1["uptime"].as_f64())
                                    .is_some_and(|(b, a)| b >= a) =>
                        {
                            bytes += b - a;
                            seconds += elapsed;
                            peak = peak.max((b - a) / elapsed);
                        }
                        _ => invalid = true,
                    }
                }
                interface[rate] = if !invalid && seconds > 0.0 {
                    json!(bytes / seconds)
                } else {
                    Value::Null
                };
                interface[format!("{rate}Peak")] = if !invalid && seconds > 0.0 {
                    json!(peak)
                } else {
                    Value::Null
                };
            }
        }
    }
    result["resolutionMs"] = json!(MINUTE_MS);
    result["sampleCount"] = json!(samples.len());
    result["peaks"] = json!(peaks);
    // Partial minutes remain isolated chart points, never a line across missing readings.
    result["gap"] =
        json!(samples.len() < 6 || samples.windows(2).any(|p| p[1].0 - p[0].0 > 30_000));
    Ok(result)
}

impl MetricsDb {
    pub async fn maintain(self) {
        let mut tick = tokio::time::interval(Duration::from_secs(15 * 60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await; // First tick runs immediately, outside app startup.
            if let Err(error) = self.compact_history(now_ms()).await {
                log::warn!("Metric history maintenance failed: {error}");
            }
        }
    }

    pub(super) async fn compact_history(&self, now: i64) -> Result<(), String> {
        // Only complete minutes entirely older than 24h are eligible.
        let cutoff = (now - DAY_MS).div_euclid(MINUTE_MS) * MINUTE_MS;
        loop {
            let count = self.compact_batch(now, cutoff).await?;
            if count == 0 {
                break;
            }
            // Release the maintenance lock between batches for live reads/writes.
            tokio::task::yield_now().await;
        }
        let _guard = self.maintenance.write().await;
        let mut connection = self.connection().await?;
        let free: i64 = sqlx::query_scalar("PRAGMA freelist_count")
            .fetch_one(&mut *connection)
            .await
            .map_err(|e| e.to_string())?;
        let pages: i64 = sqlx::query_scalar("PRAGMA page_count")
            .fetch_one(&mut *connection)
            .await
            .map_err(|e| e.to_string())?;
        // Reclaim meaningful amounts only; avoid a full rewrite every maintenance tick.
        if free >= 256 && free * 4 >= pages {
            sqlx::query("VACUUM")
                .execute(&mut *connection)
                .await
                .map_err(|e| e.to_string())?;
        }
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&mut *connection)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn compact_batch(&self, now: i64, cutoff: i64) -> Result<usize, String> {
        let _guard = self.maintenance.write().await;
        let mut connection = self.connection().await?;
        let mut tx = connection.begin().await.map_err(|e| e.to_string())?;
        sqlx::query("DELETE FROM samples WHERE at < ?1")
            .bind(now - RETENTION_MS)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let buckets: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT server_id,endpoint,(at/60000)*60000 AS minute FROM (SELECT server_id,endpoint,at FROM samples WHERE resolution=10000 AND at<?1 ORDER BY at LIMIT 96) GROUP BY server_id,endpoint,minute ORDER BY minute LIMIT ?2")
            .bind(cutoff).bind(BATCH).fetch_all(&mut *tx).await.map_err(|e| e.to_string())?;
        for (server, endpoint, minute) in &buckets {
            let rows: Vec<(i64, String)> = sqlx::query_as("SELECT at,data FROM samples WHERE server_id=?1 AND endpoint=?2 AND at>=?3 AND at<?4 ORDER BY at")
                .bind(server).bind(endpoint).bind(minute).bind(minute + MINUTE_MS).fetch_all(&mut *tx).await.map_err(|e| e.to_string())?;
            let summary = summarize(&rows)?;
            sqlx::query(
                "DELETE FROM samples WHERE server_id=?1 AND endpoint=?2 AND at>=?3 AND at<?4",
            )
            .bind(server)
            .bind(endpoint)
            .bind(minute)
            .bind(minute + MINUTE_MS)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
            sqlx::query("INSERT INTO samples(server_id,endpoint,at,data,resolution) VALUES(?1,?2,?3,?4,60000)")
                .bind(server).bind(endpoint).bind(minute).bind(summary.to_string()).execute(&mut *tx).await.map_err(|e| e.to_string())?;
        }
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(buckets.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reading(at: i64, bytes: i64, uptime: i64) -> (i64, String) {
        (at, json!({"cpu":10,"load":[1,2,3],"uptime":uptime,"network":[{"name":"eth0","received":bytes,"sent":bytes}],"disks":[{"used":bytes}]}).to_string())
    }
    #[test]
    fn reset_reboot_and_missing_readings_never_become_network_spikes() {
        for rows in [
            vec![reading(0, 100, 100), reading(10_000, 10, 110)],
            vec![reading(0, 100, 100), reading(10_000, 200, 1)],
            vec![reading(0, 100, 100), reading(50_000, 500, 150)],
            vec![reading(0, 100, 100)],
        ] {
            let value = summarize(&rows).unwrap();
            assert!(value["network"][0]["rx"].is_null());
            assert!(value["network"][0]["tx"].is_null());
            assert_eq!(value["gap"], true);
        }
        let value = summarize(&[reading(0, 100, 100), reading(10_000, 200, 110)]).unwrap();
        assert_eq!(value["disks"][0]["used"], 200);
        assert_eq!(value["uptime"], 110);
    }
}
