use super::*;

#[derive(Default)]
struct Budget {
    text: usize,
    items: usize,
}
impl Budget {
    fn text<'a>(&mut self, values: impl IntoIterator<Item = &'a str>) -> Result<(), Invalid> {
        for value in values {
            self.text = self.text.saturating_add(value.len());
            if value.len() > MAX_TEXT || self.text > MAX_REPLY_TEXT {
                return Err(Invalid("container text"));
            }
        }
        Ok(())
    }
    fn items(&mut self, count: usize) -> Result<(), Invalid> {
        self.items = self.items.saturating_add(count);
        if self.items > MAX_ITEMS {
            Err(Invalid("container metadata count"))
        } else {
            Ok(())
        }
    }
    fn strings(&mut self, values: &[String]) -> Result<(), Invalid> {
        self.items(values.len())?;
        self.text(values.iter().map(String::as_str))
    }
}
impl ContainersSnapshot {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.containers.len() > MAX_CONTAINERS {
            return Err(Invalid("container count"));
        }
        let mut budget = Budget::default();
        budget.text([
            self.engine.version.as_str(),
            &self.engine.api_version,
            &self.engine.os,
        ])?;
        let mut ids = std::collections::BTreeSet::new();
        for row in &self.containers {
            if !valid_id(&row.id)
                || !valid_container(&row.name)
                || !ids.insert(&row.id)
                || row.cpu.is_some_and(|cpu| !cpu.is_finite() || cpu < 0.0)
            {
                return Err(Invalid("container observation"));
            }
            budget.text(
                [
                    row.id.as_str(),
                    &row.name,
                    &row.image,
                    &row.image_id,
                    &row.command,
                    &row.status,
                ]
                .into_iter()
                .chain(row.error.as_deref())
                .chain(row.restart_policy.as_deref()),
            )?;
            budget.items(row.ports.len())?;
            for port in &row.ports {
                if port.container_port == 0
                    || port.host_port == Some(0)
                    || port
                        .ip
                        .as_ref()
                        .is_some_and(|ip| ip.parse::<std::net::IpAddr>().is_err())
                {
                    return Err(Invalid("container port"));
                }
                budget.text(port.ip.as_deref())?;
            }
            if let Some(compose) = &row.compose {
                budget.text(
                    [compose.project.as_str(), &compose.service]
                        .into_iter()
                        .chain(compose.working_dir.as_deref())
                        .chain(compose.config_files.as_deref()),
                )?;
            }
        }
        Ok(())
    }
}
impl ContainerInspect {
    pub fn validate(&self) -> Result<(), Invalid> {
        if !valid_id(&self.id) || !valid_container(&self.name) {
            return Err(Invalid("container identity"));
        }
        let mut budget = Budget::default();
        budget.text(
            [self.id.as_str(), &self.name, &self.image]
                .into_iter()
                .chain(self.working_dir.as_deref())
                .chain(self.user.as_deref())
                .chain(self.hostname.as_deref())
                .chain(self.restart_policy.as_deref()),
        )?;
        for args in [&self.entrypoint, &self.cmd].into_iter().flatten() {
            budget.strings(args)?;
        }
        budget.items(self.env.len())?;
        for entry in &self.env {
            budget.text([entry.name.as_str(), &entry.value])?;
        }
        budget.items(self.mounts.len())?;
        for mount in &self.mounts {
            budget.text(
                [mount.kind.as_str(), &mount.destination]
                    .into_iter()
                    .chain(mount.source.as_deref())
                    .chain(mount.mode.as_deref())
                    .chain(mount.name.as_deref()),
            )?;
        }
        budget.items(self.networks.len())?;
        for network in &self.networks {
            budget.text(
                std::iter::once(network.name.as_str()).chain(network.ip_address.as_deref()),
            )?;
            budget.strings(&network.aliases)?;
        }
        budget.items(self.labels.len())?;
        for (key, value) in &self.labels {
            budget.text([key.as_str(), value])?;
        }
        if let Some(check) = &self.healthcheck {
            budget.strings(&check.test)?;
        }
        budget.items(self.last_health.len())?;
        budget.text(self.last_health.iter().map(|entry| entry.output.as_str()))
    }
}
impl ContainerLogPage {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.entries.len() > MAX_LOG_ENTRIES
            || self.cursor.as_ref().is_some_and(|c| !valid_cursor(c))
        {
            return Err(Invalid("container log page"));
        }
        let mut budget = Budget::default();
        budget.text(self.cursor.as_deref())?;
        for entry in &self.entries {
            budget
                .text(std::iter::once(entry.message.as_str()).chain(entry.container.as_deref()))?;
        }
        Ok(())
    }
}
impl DockerDisk {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.images.len() > MAX_DISK_ROWS || self.volumes.len() > MAX_DISK_ROWS {
            return Err(Invalid("container disk rows"));
        }
        let mut budget = Budget::default();
        for image in &self.images {
            budget.text([image.id.as_str()])?;
            budget.strings(&image.tags)?;
        }
        for volume in &self.volumes {
            budget.text(
                [volume.name.as_str(), &volume.driver]
                    .into_iter()
                    .chain(volume.compose.as_deref()),
            )?;
        }
        for total in [
            self.totals.images,
            self.totals.containers,
            self.totals.volumes,
            self.totals.build_cache,
        ] {
            if total.reclaimable > total.size {
                return Err(Invalid("container reclaimable size"));
            }
        }
        Ok(())
    }
}
