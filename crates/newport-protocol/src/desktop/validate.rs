use super::*;

impl crate::desktop::account::Notice {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.message)?;

        Ok(())
    }
}

impl crate::desktop::account::License {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.state.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.plan.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.seat.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.valid_until.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.renews_at.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.ends_at.as_ref() {
            text(v)?;
        }

        if let Some(v) = self.message.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::account::Status {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.email.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.name.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.license.as_ref() {
            v.validate()?;
        }

        if let Some(v) = self.notice.as_ref() {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::account::RemoteLicense {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.machine)?;

        Ok(())
    }
}

impl crate::desktop::configuration::Proxy {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Command(v) => {
                text(v)?;
            }
            Self::Jump(_) => {}
        }

        Ok(())
    }
}

impl crate::desktop::configuration::Server {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.name)?;
        text(&self.ssh_user)?;
        text(&self.ssh_host)?;

        if let Some(v) = self.identity_file.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.agent_source.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.agent_key_fingerprint.as_ref() {
            text(v)?;
        }

        if let Some(v) = self.proxy.as_ref() {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::configuration::Tunnel {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.name)?;

        text(&self.remote_host)?;

        if let Some(v) = self.project.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::configuration::Config {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.servers.len())?;
        for v in &self.servers {
            v.validate()?;
        }
        count(self.tunnels.len())?;
        for v in &self.tunnels {
            v.validate()?;
        }
        unique(self.servers.iter().map(|v| v.id))?;
        unique(self.tunnels.iter().map(|v| v.id))?;
        Ok(())
    }
}

impl crate::desktop::configuration::ConnectionSettings {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.ssh_host)?;
        text(&self.ssh_user)?;

        if let Some(v) = self.identity_file.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.agent_source.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.agent_key_fingerprint.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.proxy.as_ref() {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::configuration::ConnectionScope {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.settings.validate()?;

        Ok(())
    }
}

impl crate::desktop::configuration::Transport {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::configuration::RuntimeInstance {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.build_sha256)?;

        Ok(())
    }
}

impl crate::desktop::configuration::RuntimeSnapshot {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.error.as_ref() {
            text(v)?;
        }

        count(self.instances.len())?;
        for v in &self.instances {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::configuration::PermissionsSnapshot {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.desired
            .validate()
            .map_err(|_| Invalid("desired permission revision"))?;
        if self.effective.revision == 0 && (self.effective.clipboard || self.effective.browser) {
            return Err(Invalid("unacknowledged permission grant"));
        }
        if let Some(v) = self.error.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::configuration::UpdateOutcome {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.from.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.to.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.error.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::configuration::InstallationStatus {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.installed_hash.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.bundled_hash.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.installed_version.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.bundled_version.as_ref() {
            text(v)?;
        }

        if let Some(v) = self.error.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.automatic_update.as_ref() {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::configuration::InstallProgress {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.error.as_ref() {
            text(v)?;
        }

        if self
            .bytes_per_second
            .is_some_and(|value| !value.is_finite())
        {
            return Err(Invalid("non-finite number"));
        }
        if self.sent > self.total || self.bytes_per_second.is_some_and(|v| v < 0.0) {
            return Err(Invalid("install progress"));
        }
        Ok(())
    }
}

impl crate::desktop::configuration::ConnectionState {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.error_message.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::configuration::DestinationHealth {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.message.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::configuration::DestinationHealthList {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.values.len())?;
        for v in &self.values {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::configuration::TransportRow {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.value.validate()?;

        Ok(())
    }
}

impl crate::desktop::configuration::RuntimeRow {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.value.validate()?;

        Ok(())
    }
}

impl crate::desktop::configuration::PermissionsRow {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.value.validate()?;

        Ok(())
    }
}

impl crate::desktop::configuration::InstallRow {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.value.validate()?;

        Ok(())
    }
}

impl crate::desktop::configuration::InstallationRow {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.value.validate()?;

        Ok(())
    }
}

impl crate::desktop::configuration::ConnectionRow {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.value.validate()?;

        Ok(())
    }
}

impl crate::desktop::configuration::HealthRow {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.value.validate()?;

        Ok(())
    }
}

impl crate::desktop::configuration::MessageRow {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.value)?;

        Ok(())
    }
}

impl crate::desktop::configuration::RevisionRow {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::configuration::Runtime {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.agent_transport.len())?;
        for v in &self.agent_transport {
            v.validate()?;
        }
        count(self.agent_runtime.len())?;
        for v in &self.agent_runtime {
            v.validate()?;
        }
        count(self.agent_permissions.len())?;
        for v in &self.agent_permissions {
            v.validate()?;
        }
        count(self.agent_status.len())?;
        for v in &self.agent_status {
            v.validate()?;
        }
        count(self.agent_installation.len())?;
        for v in &self.agent_installation {
            v.validate()?;
        }
        count(self.tunnels.len())?;
        for v in &self.tunnels {
            v.validate()?;
        }
        count(self.tunnel_health.len())?;
        for v in &self.tunnel_health {
            v.validate()?;
        }
        count(self.clipboard.len())?;
        for v in &self.clipboard {
            v.validate()?;
        }
        count(self.clipboard_messages.len())?;
        for v in &self.clipboard_messages {
            v.validate()?;
        }
        count(self.connection_revisions.len())?;
        for v in &self.connection_revisions {
            v.validate()?;
        }
        count(self.awaiting_agent.len())?;
        unique(self.agent_transport.iter().map(|v| v.id))?;
        unique(self.agent_runtime.iter().map(|v| v.id))?;
        unique(self.agent_permissions.iter().map(|v| v.id))?;
        unique(self.agent_status.iter().map(|v| v.id))?;
        unique(self.agent_installation.iter().map(|v| v.id))?;
        unique(self.tunnels.iter().map(|v| v.id))?;
        unique(self.tunnel_health.iter().map(|v| v.id))?;
        unique(self.clipboard.iter().map(|v| v.id))?;
        unique(self.clipboard_messages.iter().map(|v| v.id))?;
        unique(self.connection_revisions.iter().map(|v| v.id))?;
        unique(self.awaiting_agent.iter().copied())?;
        Ok(())
    }
}

impl crate::desktop::configuration::Snapshot {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.config.validate()?;
        self.runtime.validate()?;
        if let Some(v) = self.load_error.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::configuration::Tunnels {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.tunnels.len())?;
        for v in &self.tunnels {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::configuration::SaveServerArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.server.validate()?;

        Ok(())
    }
}

impl crate::desktop::configuration::DeleteServerArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::configuration::ForwardDiscoveredPortArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.address)?;

        Ok(())
    }
}

impl crate::desktop::configuration::ForwardTunnelBatchArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.create.len())?;
        for v in &self.create {
            v.validate()?;
        }
        count(self.connect.len())?;
        for v in &self.connect {
            v.validate()?;
        }
        if let Some(v) = self.expected.as_ref() {
            v.validate()?;
        }
        if self
            .expected
            .as_ref()
            .is_some_and(|scope| scope.settings.server_id != self.server)
        {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::configuration::SaveTunnelArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.tunnel.validate()?;

        Ok(())
    }
}

impl crate::desktop::configuration::UpdateTunnelArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.tunnel.validate()?;
        self.expected.validate()?;

        Ok(())
    }
}

impl crate::desktop::configuration::DeleteTunnelArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::configuration::DeleteTunnelsArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.expected.len())?;
        for v in &self.expected {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::configuration::SetTunnelConnectedArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::configuration::SetClipboardEnabledArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.expected.validate()?;
        if self.expected.settings.server_id != self.server {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::configuration::SetIntegrationEnabledArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.expected.validate()?;
        if self.expected.settings.server_id != self.server {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::configuration::OpenTunnelArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.expected.as_ref() {
            v.validate()?;
        }
        if let Some(v) = self.expected_connection.as_ref() {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::configuration::OpenTerminalLinkArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.url)?;

        Ok(())
    }
}

impl crate::desktop::connection::SetupInspection {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.os)?;
        text(&self.architecture)?;

        Ok(())
    }
}

impl crate::desktop::connection::OptionalNotice {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.notice.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::connection::AgentKey {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.source)?;
        text(&self.comment)?;
        text(&self.fingerprint)?;
        text(&self.algorithm)?;

        Ok(())
    }
}

impl crate::desktop::connection::AgentKeyList {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.keys.len())?;
        for v in &self.keys {
            v.validate()?;
        }
        count(self.warnings.len())?;
        for v in &self.warnings {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::connection::ConfigHost {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.alias)?;
        text(&self.host)?;
        text(&self.user)?;

        if let Some(v) = self.identity_file.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.proxy.as_ref() {
            v.validate()?;
        }
        count(self.through.len())?;
        for v in &self.through {
            text(v)?;
        }

        if let Some(v) = self.note.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::connection::ConfigHosts {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.path)?;
        count(self.hosts.len())?;
        for v in &self.hosts {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::connection::PortsObservation {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.data.as_ref() {
            v.validate().map_err(|_| Invalid("discovery observation"))?;
        }
        if let Some(v) = self.error.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::connection::UninstallOutput {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.text)?;

        Ok(())
    }
}

impl crate::desktop::connection::SetupInspectArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.expected_server.validate()?;

        Ok(())
    }
}

impl crate::desktop::connection::SetupInstallArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.expected_server.validate()?;

        Ok(())
    }
}

impl crate::desktop::connection::SetupFinishArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.expected_server.validate()?;

        Ok(())
    }
}

impl crate::desktop::connection::SetupSkipArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.expected_server.validate()?;

        Ok(())
    }
}

impl crate::desktop::connection::TestConnectionArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::connection::ConnectionsQueryArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::connection::ReinstallAgentArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::connection::UninstallAgentArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.expected_server.validate()?;

        Ok(())
    }
}

impl crate::desktop::connection::SshResolveArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.host)?;

        Ok(())
    }
}

impl crate::desktop::connection::SshResolved {
    pub fn validate(&self) -> Result<(), Invalid> {
        for v in [
            &self.hostname,
            &self.user,
            &self.identity_file,
            &self.proxy_command,
            &self.proxy_jump,
        ]
        .into_iter()
        .flatten()
        {
            text(v)?;
        }
        if self.port == Some(0) {
            return Err(Invalid("ssh port"));
        }

        Ok(())
    }
}

impl crate::desktop::connection::InspectAgentArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.expected_server.validate()?;

        Ok(())
    }
}

impl crate::desktop::files::Entry {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.name)?;
        text(&self.path)?;

        if let Some(v) = self.permissions.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::files::Listing {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.path)?;
        count(self.entries.len())?;
        for v in &self.entries {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::files::Content {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Text(v) => {
                bulk(v.len(), MAX_DOCUMENT)?;
            }
            Self::Binary(v) => {
                bulk(v.len(), MAX_DOCUMENT)?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::files::Preview {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.mime)?;
        self.content.validate()?;

        if !matches!(
            (&self.kind, &self.content),
            (
                crate::desktop::files::PreviewKind::Text,
                crate::desktop::files::Content::Text(_)
            ) | (
                crate::desktop::files::PreviewKind::Image,
                crate::desktop::files::Content::Binary(_)
            )
        ) {
            return Err(Invalid("preview content kind"));
        }
        Ok(())
    }
}

impl crate::desktop::files::Page {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::files::Document {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.path)?;
        bulk(self.content.len(), MAX_DOCUMENT)?;
        text(&self.revision)?;
        if let Some(v) = self.page.as_ref() {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::files::Index {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.paths.len())?;
        for v in &self.paths {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::files::WatchReply {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.cursor.as_ref() {
            text(v)?;
        }
        count(self.paths.len())?;
        for v in &self.paths {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::files::TransferProgress {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.name)?;

        Ok(())
    }
}

impl crate::desktop::files::ProgressLookup {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.progress.as_ref() {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::files::NativePath {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Unix(v) => {
                bulk(v.len(), MAX_DOCUMENT)?;
            }
            Self::Windows(v) => {
                bulk(v.len(), MAX_PATH)?;
                if v.contains(&0) {
                    return Err(Invalid("path NUL"));
                }
            }
        }
        match self {
            Self::Unix(v) if v.contains(&0) => return Err(Invalid("path NUL")),
            Self::Unix(_) | Self::Windows(_) => {}
        }
        Ok(())
    }
}

impl crate::desktop::files::TransferPath {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Remote(v) => {
                text(v)?;
            }
            Self::Local(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::files::TransferResult {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.path.as_ref() {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::files::ListArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.path)?;
        if let Some(v) = self.expected.as_ref() {
            v.validate()?;
        }

        if let Some(v) = self.project_root.as_ref() {
            text(v)?;
        }
        if self
            .expected
            .as_ref()
            .is_some_and(|scope| scope.settings.server_id != self.server)
        {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::files::WatchArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.expected.validate()?;
        text(&self.root)?;

        count(self.directories.len())?;
        for v in &self.directories {
            text(v)?;
        }
        if let Some(v) = self.cursor.as_ref() {
            text(v)?;
        }
        if self.directories.len() > 64 {
            return Err(Invalid("watched directory count"));
        }
        if self.expected.settings.server_id != self.server {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::files::PreviewArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.path)?;
        if let Some(v) = self.expected.as_ref() {
            v.validate()?;
        }
        if self
            .expected
            .as_ref()
            .is_some_and(|scope| scope.settings.server_id != self.server)
        {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::files::ReadTextArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.root)?;
        text(&self.path)?;
        self.expected.validate()?;
        if let Some(v) = self.revision.as_ref() {
            text(v)?;
        }

        if self.expected.settings.server_id != self.server {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::files::ProjectIndexArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.root)?;
        self.expected.validate()?;
        if self.expected.settings.server_id != self.server {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::files::WriteTextArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.root)?;
        text(&self.path)?;
        self.expected.validate()?;
        bulk(self.content.len(), MAX_DOCUMENT)?;
        text(&self.revision)?;
        if self.expected.settings.server_id != self.server {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::files::CancelArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::files::ProgressArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::files::UploadArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.path)?;
        self.local_path.validate()?;
        self.expected.validate()?;
        if self.expected.settings.server_id != self.server {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::files::DownloadArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.path)?;
        self.local_path.validate()?;
        self.expected.validate()?;
        if self.expected.settings.server_id != self.server {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::metrics::Reading {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.collection_error.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.history_error.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::metrics::NumericMeasures {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.cpu.is_some_and(|value| !value.is_finite()) {
            return Err(Invalid("non-finite number"));
        }
        if self.memory_used.is_some_and(|value| !value.is_finite()) {
            return Err(Invalid("non-finite number"));
        }
        if self.memory_total.is_some_and(|value| !value.is_finite()) {
            return Err(Invalid("non-finite number"));
        }
        if self.swap_used.is_some_and(|value| !value.is_finite()) {
            return Err(Invalid("non-finite number"));
        }
        if self.swap_total.is_some_and(|value| !value.is_finite()) {
            return Err(Invalid("non-finite number"));
        }
        if self.load_one.is_some_and(|value| !value.is_finite()) {
            return Err(Invalid("non-finite number"));
        }
        if self.load_five.is_some_and(|value| !value.is_finite()) {
            return Err(Invalid("non-finite number"));
        }
        if self.load_fifteen.is_some_and(|value| !value.is_finite()) {
            return Err(Invalid("non-finite number"));
        }

        Ok(())
    }
}

impl crate::desktop::metrics::NetworkHistory {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.name)?;

        if self.rx.is_some_and(|value| !value.is_finite()) {
            return Err(Invalid("non-finite number"));
        }
        if self.tx.is_some_and(|value| !value.is_finite()) {
            return Err(Invalid("non-finite number"));
        }
        if self.rx_peak.is_some_and(|value| !value.is_finite()) {
            return Err(Invalid("non-finite number"));
        }
        if self.tx_peak.is_some_and(|value| !value.is_finite()) {
            return Err(Invalid("non-finite number"));
        }

        Ok(())
    }
}

impl crate::desktop::metrics::ChartBucket {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.means.validate()?;
        self.peaks.validate()?;
        count(self.network.len())?;
        for v in &self.network {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::metrics::HistoricalDisk {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(value) = &self.mount {
            text(value)?;
        }
        if let Some(value) = &self.device {
            text(value)?;
        }
        Ok(())
    }
}

fn historical_disks(
    value: &Option<Vec<crate::desktop::metrics::HistoricalDisk>>,
) -> Result<(), Invalid> {
    if let Some(disks) = value {
        if disks.len() > crate::monitor::metrics::MAX_DISKS {
            return Err(Invalid("historical disks"));
        }
        for disk in disks {
            disk.validate()?;
        }
    }
    Ok(())
}

impl crate::desktop::metrics::HistoricalRaw {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.measures.validate()?;
        if self.measures.cpu.is_none() {
            return Err(Invalid("historical CPU"));
        }
        if self.uptime.is_some_and(|v| !v.is_finite()) {
            return Err(Invalid("non-finite uptime"));
        }
        if self.network.len() > crate::monitor::metrics::MAX_INTERFACES {
            return Err(Invalid("historical network"));
        }
        for interface in &self.network {
            interface.validate()?;
        }
        historical_disks(&self.disks)
    }
}

impl crate::desktop::metrics::MinuteRollup {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.summary.validate()?;
        if self.summary.means.cpu.is_none() {
            return Err(Invalid("historical CPU"));
        }
        if self.uptime.is_some_and(|v| !v.is_finite()) {
            return Err(Invalid("non-finite uptime"));
        }
        historical_disks(&self.disks)
    }
}

impl crate::desktop::metrics::SampleValue {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Raw(v) => {
                v.validate()?;
            }
            Self::Minute(v) => {
                v.validate()?;
            }
            Self::Chart(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::metrics::Sample {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.value.validate()?;

        Ok(())
    }
}

impl crate::desktop::metrics::History {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.samples.len())?;
        for v in &self.samples {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::metrics::CacheInfo {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::metrics::CollectArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::metrics::HistoryArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::services::SnapshotArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::services::LogsArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::services::UnitFileArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.unit)?;

        Ok(())
    }
}

impl crate::desktop::services::ActionArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.unit)?;

        Ok(())
    }
}

impl crate::desktop::containers::SnapshotArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::containers::InspectArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.container)?;

        Ok(())
    }
}

impl crate::desktop::containers::LogsArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::containers::ActionArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.container)?;

        Ok(())
    }
}

impl crate::desktop::containers::ProjectActionArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.project)?;

        count(self.expected_ids.len())?;
        for v in &self.expected_ids {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::containers::RemoveArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.containers.len())?;
        for v in &self.containers {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::containers::DiskArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::containers::PruneArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::containers::ComposeProjectArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.project)?;

        Ok(())
    }
}

impl crate::desktop::containers::ComposeRunArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.project)?;

        Ok(())
    }
}

impl crate::desktop::containers::ComposeJobArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.job)?;

        Ok(())
    }
}

impl crate::desktop::containers::ComposeCancelArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.job)?;

        Ok(())
    }
}

impl crate::desktop::containers::ComposeJobsArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::preferences::WindowPatch {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.sidebar_width.is_some_and(|value| !value.is_finite()) {
            return Err(Invalid("non-finite number"));
        }

        if self
            .sidebar_width
            .is_some_and(|v| !(176.0..=300.0).contains(&v))
        {
            return Err(Invalid("sidebar width"));
        }
        Ok(())
    }
}

impl crate::desktop::preferences::Project {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.name)?;

        Ok(())
    }
}

impl crate::desktop::preferences::Projects {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.projects.len())?;
        for v in &self.projects {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::preferences::Settings {
    pub fn validate(&self) -> Result<(), Invalid> {
        if ![15, 30, 60].contains(&self.fps_goal) {
            return Err(Invalid("frame rate"));
        }
        Ok(())
    }
}

impl crate::desktop::preferences::Installed {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.servers.len())?;

        Ok(())
    }
}

impl crate::desktop::preferences::SetWindowArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.patch.validate()?;

        Ok(())
    }
}

impl crate::desktop::preferences::ProjectsListArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::preferences::ProjectsSaveArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.project.validate()?;

        Ok(())
    }
}

impl crate::desktop::preferences::ProjectsRemoveArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::preferences::SettingsSaveArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.settings.validate()?;

        Ok(())
    }
}

impl crate::desktop::preferences::InstalledSaveArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::git::CollectedDiff {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.snapshot)?;

        Ok(())
    }
}

impl crate::desktop::git::GitResult {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Reply(_) => {}
            Self::Diff(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::git::Refreshed {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.repo_id)?;

        Ok(())
    }
}

impl crate::desktop::git::Receipt {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.action)?;

        Ok(())
    }
}

impl crate::desktop::git::Receipts {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.receipts.len())?;
        for v in &self.receipts {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::git::RequestArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.retry_connection && !matches!(self.request, crate::git::Request::Open { .. }) {
            return Err(Invalid("retry is only allowed for repository open"));
        }
        Ok(())
    }
}

impl crate::desktop::git::RefreshRepositoryArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.repo_id)?;

        Ok(())
    }
}

impl crate::desktop::git::PendingOperationsArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::git::AcknowledgeOperationArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::git::ReviewOperationArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::applications::Runtime {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.build_sha256)?;

        Ok(())
    }
}

impl crate::desktop::applications::Identity {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.runtime.validate()?;

        Ok(())
    }
}

impl crate::desktop::applications::Command {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.argv.len())?;
        for v in &self.argv {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::applications::Desktop {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.id)?;
        count(self.uris.len())?;
        for v in &self.uris {
            text(v)?;
        }
        if let Some(v) = self.action.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::applications::Target {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Command(v) => {
                v.validate()?;
            }
            Self::Desktop(v) => {
                v.validate()?;
            }
        }
        let words: Vec<&str> = match self {
            Self::Command(v) => {
                if v.argv.is_empty() || v.argv[0].is_empty() || v.argv.len() > 128 {
                    return Err(Invalid("application arguments"));
                }
                v.argv.iter().map(String::as_str).collect()
            }
            Self::Desktop(v) => {
                if v.id.is_empty()
                    || v.uris.len() > 64
                    || v.action.as_ref().is_some_and(String::is_empty)
                {
                    return Err(Invalid("desktop target"));
                }
                std::iter::once(v.id.as_str())
                    .chain(v.uris.iter().map(String::as_str))
                    .chain(v.action.as_deref())
                    .collect()
            }
        };
        if words.iter().any(|v| v.len() > 8192)
            || words.iter().map(|v| v.len()).sum::<usize>() > 16384
        {
            return Err(Invalid("application argument bytes"));
        }
        Ok(())
    }
}

impl crate::desktop::applications::Outcome {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Accepted(_) => {}
            Self::Rejected(v) => {
                text(v)?;
            }
            Self::Unknown(v) => {
                text(v)?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::applications::Application {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.display.validate()?;

        Ok(())
    }
}

impl crate::desktop::applications::Launched {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.application.validate()?;
        self.outcome.validate()?;

        Ok(())
    }
}

impl crate::desktop::applications::OptionalDetail {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.detail.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::applications::Ending {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Failed(v) => {
                text(v)?;
            }
            Self::NoWindow(v) => {
                v.validate()?;
            }
            Self::HandedOff(_) => {}
            Self::DisplayStopped(_) => {}
            Self::ShownElsewhere(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::applications::Observation {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.application.validate()?;

        if let Some(v) = self.error.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.ending.as_ref() {
            v.validate()?;
        }
        if self.ending.is_some() && self.state != Some(crate::desktop::applications::State::Ended) {
            return Err(Invalid("application ending state"));
        }
        Ok(())
    }
}

impl crate::desktop::applications::Observations {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.observations.len())?;
        for v in &self.observations {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::applications::Shown {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.app_id)?;

        Ok(())
    }
}

impl crate::desktop::applications::ShownOn {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.computer.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.session.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::applications::Display {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.shown_on.as_ref() {
            v.validate()?;
        }
        count(self.applications.len())?;
        for v in &self.applications {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::applications::Stopped {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::applications::Finding {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Missing(_) => {}
            Self::Outdated(v) => {
                text(v)?;
            }
            Self::Inactive(_) => {}
            Self::Unverified(_) => {}
        }

        Ok(())
    }
}

impl crate::desktop::applications::Gap {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.finding.validate()?;

        Ok(())
    }
}

impl crate::desktop::applications::Host {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::applications::LegacyAgent {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.revision)?;
        text(&self.agent)?;

        Ok(())
    }
}

impl crate::desktop::applications::SelectedRuntime {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.build_sha256)?;

        Ok(())
    }
}

impl crate::desktop::applications::Availability {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.agent.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.legacy_agent.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.legacy_revision.as_ref() {
            text(v)?;
        }
        count(self.legacy_agents.len())?;
        for v in &self.legacy_agents {
            v.validate()?;
        }
        if let Some(v) = self.system.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.architecture.as_ref() {
            text(v)?;
        }
        count(self.gaps.len())?;
        for v in &self.gaps {
            v.validate()?;
        }
        self.host.validate()?;
        text(&self.os_release)?;
        self.expected_server.validate()?;

        if let Some(v) = self.bundled_revision.as_ref() {
            text(v)?;
        }

        if let Some(v) = self.revision.as_ref() {
            text(v)?;
        }

        if let Some(v) = self.installation_error.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.selected_runtime.as_ref() {
            v.validate()?;
        }
        if let Some(v) = self.agent_error.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.automatic_update.as_ref() {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::applications::StatusArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::applications::CatalogueArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::applications::DisplayArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::applications::LaunchArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.expected.validate()?;
        self.target.validate()?;
        if self.expected.settings.server_id != self.server {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::applications::ShowArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.expected.validate()?;
        if self.expected.settings.server_id != self.server {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::applications::StatesArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.expected.validate()?;
        count(self.applications.len())?;
        for v in &self.applications {
            v.validate()?;
        }
        if self.applications.len() > 128 {
            return Err(Invalid("application observation count"));
        }
        if self.expected.settings.server_id != self.server {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::applications::StopArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.expected.validate()?;
        self.application.validate()?;
        if self.expected.settings.server_id != self.server {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::host::Startup {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::host::BuildStamp {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.commit.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::host::UpdateStatus {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.current_version)?;
        self.build.validate()?;

        if let Some(v) = self.version.as_ref() {
            text(v)?;
        }

        if let Some(v) = self.error.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::host::OpenRemoteAppArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.id)?;
        if let Some(v) = self.action.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::host::SetLaunchAtLoginArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::host::InstallUpdateArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.confirmed_version)?;

        Ok(())
    }
}

impl crate::desktop::events::GitCommand {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.repo_id.as_ref() {
            text(v)?;
        }

        Ok(())
    }
}

impl crate::desktop::events::Event {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::GitCommand(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::events::Batch {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.entries.len())?;
        for v in &self.entries {
            v.validate()?;
        }
        if self.entries.len() > 100 {
            return Err(Invalid("event count"));
        }
        Ok(())
    }
}

impl crate::desktop::events::PollArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::terminal::HeldShell {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.name.as_ref() {
            text(v)?;
        }
        if let Some(v) = self.title.as_ref() {
            text(v)?;
        }

        count(self.attached.len())?;

        Ok(())
    }
}

impl crate::desktop::terminal::ShellList {
    pub fn validate(&self) -> Result<(), Invalid> {
        count(self.shells.len())?;
        for v in &self.shells {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::terminal::HeldShells {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.desktop)?;

        if let Some(v) = self.shells.as_ref() {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::terminal::Renamed {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::terminal::Ended {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::terminal::CloseArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::terminal::ListArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::terminal::RenameArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.name)?;

        Ok(())
    }
}

impl crate::desktop::terminal::EndArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::stream::TerminalOpenArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.columns == 0 || self.rows == 0 {
            return Err(Invalid("terminal size"));
        }
        Ok(())
    }
}

impl crate::desktop::stream::CommandOpenArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.command)?;
        if self.command.trim().is_empty() || self.command.len() > stream::MAX_COMMAND_BYTES {
            return Err(Invalid("remote command"));
        }
        Ok(())
    }
}

impl crate::desktop::stream::RemoteAppsOpenArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.arguments.len() > stream::MAX_REMOTE_ARGUMENTS {
            return Err(Invalid("remote app argument count"));
        }
        let mut remaining = stream::MAX_REMOTE_ARGUMENT_TOTAL_BYTES;
        for v in &self.arguments {
            text(v)?;
            if v.len() > stream::MAX_REMOTE_ARGUMENT_BYTES {
                return Err(Invalid("remote app argument length"));
            }
            remaining = remaining
                .checked_sub(v.len())
                .ok_or(Invalid("remote app total argument length"))?;
        }

        Ok(())
    }
}

impl crate::desktop::stream::LspOpenArgs {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.expected.validate()?;
        text(&self.root)?;
        text(&self.server_name)?;

        // This path belongs to the Linux server, even on a Windows desktop.
        if !self.root.starts_with('/') || self.root.len() > stream::MAX_LSP_ROOT_BYTES {
            return Err(Invalid("language server worktree path"));
        }
        if self.server_name.is_empty()
            || self.server_name.len() > stream::MAX_LSP_SERVER_NAME_BYTES
            || !self
                .server_name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        {
            return Err(Invalid("language server identifier"));
        }
        if self.expected.settings.server_id != self.server {
            return Err(Invalid("connection scope server"));
        }
        Ok(())
    }
}

impl crate::desktop::stream::TerminalOpened {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.version != VERSION {
            return Err(Invalid("stream version"));
        }
        Ok(())
    }
}

impl crate::desktop::stream::CommandOpened {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.version != VERSION {
            return Err(Invalid("stream version"));
        }
        Ok(())
    }
}

impl crate::desktop::stream::RemoteAppsOpened {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.version != VERSION {
            return Err(Invalid("stream version"));
        }
        Ok(())
    }
}

impl crate::desktop::stream::LspOpened {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.version != VERSION {
            return Err(Invalid("stream version"));
        }
        Ok(())
    }
}

impl crate::desktop::stream::Opened {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Terminal(v) => {
                v.validate()?;
            }
            Self::Command(v) => {
                v.validate()?;
            }
            Self::RemoteApps(v) => {
                v.validate()?;
            }
            Self::Lsp(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::stream::CommandCompletion {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.signal.as_ref() {
            text(v)?;
        }

        bulk(self.stderr.len(), MAX_DOCUMENT)?;

        bulk(self.stderr.len(), STDERR_LIMIT)?;
        if self.status.is_some() && self.signal.is_some() {
            return Err(Invalid("conflicting command exit"));
        }
        if self.ending != crate::desktop::stream::Ending::Exited
            && (self.status.is_some() || self.signal.is_some())
        {
            return Err(Invalid("unconfirmed command exit"));
        }
        Ok(())
    }
}

impl crate::desktop::stream::LspCompletion {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::stream::Completion {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Command(v) => {
                v.validate()?;
            }
            Self::Lsp(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::stream::OpaqueClient {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Data(v) => {
                bulk(v.len(), MAX_DOCUMENT)?;
            }
            Self::InputComplete(_) => {}
        }
        if let Self::Data(v) = self {
            bulk(v.len(), OPAQUE_CHUNK)?;
        }
        Ok(())
    }
}

impl crate::desktop::stream::OpaqueServer {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Data(v) => {
                bulk(v.len(), MAX_DOCUMENT)?;
            }
            Self::Completion(v) => {
                v.validate()?;
            }
        }
        if let Self::Data(v) = self {
            bulk(v.len(), OPAQUE_CHUNK)?;
        }
        Ok(())
    }
}

impl crate::desktop::stream::Input {
    pub fn validate(&self) -> Result<(), Invalid> {
        bulk(self.data.len(), MAX_DOCUMENT)?;
        bulk(self.data.len(), MAX_DATA)?;
        Ok(())
    }
}

impl crate::desktop::stream::Resize {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.columns == 0 || self.rows == 0 {
            return Err(Invalid("terminal size"));
        }
        Ok(())
    }
}

impl crate::desktop::stream::Ack {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::stream::TerminalClient {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Input(v) => {
                v.validate()?;
            }
            Self::Resize(v) => {
                v.validate()?;
            }
            Self::Ack(v) => {
                v.validate()?;
            }
            Self::End(_) => {}
            Self::Activate(_) => {}
        }

        Ok(())
    }
}

impl crate::desktop::stream::Output {
    pub fn validate(&self) -> Result<(), Invalid> {
        bulk(self.data.len(), MAX_DOCUMENT)?;
        bulk(self.data.len(), MAX_DATA)?;
        Ok(())
    }
}

impl crate::desktop::stream::Taken {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::stream::Restore {
    pub fn validate(&self) -> Result<(), Invalid> {
        bulk(self.state.len(), MAX_DOCUMENT)?;

        Ok(())
    }
}

impl crate::desktop::stream::Reconnecting {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.message)?;

        Ok(())
    }
}

impl crate::desktop::stream::NotHeld {
    pub fn validate(&self) -> Result<(), Invalid> {
        text(&self.reason)?;

        Ok(())
    }
}

impl crate::desktop::stream::Exit {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::stream::Active {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::stream::TerminalEvent {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Ready(_) => {}
            Self::Persistent(_) => {}
            Self::Reconnecting(v) => {
                v.validate()?;
            }
            Self::Exit(v) => {
                v.validate()?;
            }
            Self::Error(v) => {
                v.validate()?;
            }
            Self::Ended(_) => {}
            Self::Active(v) => {
                v.validate()?;
            }
            Self::DisplayLost(_) => {}
            Self::NotHeld(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::stream::TerminalServer {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Output(v) => {
                v.validate()?;
            }
            Self::Taken(v) => {
                v.validate()?;
            }
            Self::Restore(v) => {
                v.validate()?;
            }
            Self::Event(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::Hello {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.version != VERSION {
            return Err(Invalid("desktop version"));
        }
        if self
            .contract_id
            .as_deref()
            .is_some_and(|id| !crate::common::valid_contract_id(id))
        {
            return Err(Invalid("desktop contract identifier"));
        }
        Ok(())
    }
}

impl crate::desktop::Ready {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.version != VERSION {
            return Err(Invalid("desktop version"));
        }
        if self
            .contract_id
            .as_deref()
            .is_some_and(|id| !crate::common::valid_contract_id(id))
        {
            return Err(Invalid("desktop contract identifier"));
        }
        Ok(())
    }
}

impl crate::desktop::FailureDetail {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::LicenseRefusal(_) => {}
        }

        Ok(())
    }
}

impl crate::desktop::Failure {
    pub fn validate(&self) -> Result<(), Invalid> {
        if let Some(v) = self.detail.as_ref() {
            v.validate()?;
        }

        Ok(())
    }
}

impl crate::desktop::AgentApproval {
    pub fn validate(&self) -> Result<(), Invalid> {
        Ok(())
    }
}

impl crate::desktop::ProgressState {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::AgentApproval(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::Progress {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.state.validate()?;

        Ok(())
    }
}

impl crate::desktop::account::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Status(_) => {}
            Self::SignIn(_) => {}
            Self::CancelSignIn(_) => {}
            Self::SignOut(_) => {}
            Self::OpenPortal(_) => {}
            Self::OpenBilling(_) => {}
            Self::Refresh(_) => {}
            Self::StartTrial(_) => {}
            Self::RemoteLicense(_) => {}
        }

        Ok(())
    }
}

impl crate::desktop::account::Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Status(v) => {
                v.validate()?;
            }
            Self::SignIn(v) => {
                v.validate()?;
            }
            Self::CancelSignIn(v) => {
                v.validate()?;
            }
            Self::SignOut(v) => {
                v.validate()?;
            }
            Self::OpenPortal(_) => {}
            Self::OpenBilling(_) => {}
            Self::Refresh(v) => {
                v.validate()?;
            }
            Self::StartTrial(v) => {
                v.validate()?;
            }
            Self::RemoteLicense(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::connection::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::SetupInspect(v) => {
                v.validate()?;
            }
            Self::SetupInstall(v) => {
                v.validate()?;
            }
            Self::SetupFinish(v) => {
                v.validate()?;
            }
            Self::SetupSkip(v) => {
                v.validate()?;
            }
            Self::SshAgentKeys(_) => {}
            Self::SshConfigHosts(_) => {}
            Self::TestConnection(v) => {
                v.validate()?;
            }
            Self::ConnectionsQuery(v) => {
                v.validate()?;
            }
            Self::ReinstallAgent(v) => {
                v.validate()?;
            }
            Self::UninstallAgent(v) => {
                v.validate()?;
            }
            Self::InspectAgent(v) => {
                v.validate()?;
            }
            Self::SshResolve(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::connection::Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::SetupInspect(v) => {
                v.validate()?;
            }
            Self::SetupInstall(_) => {}
            Self::SetupFinish(v) => {
                v.validate()?;
            }
            Self::SetupSkip(_) => {}
            Self::SshAgentKeys(v) => {
                v.validate()?;
            }
            Self::SshConfigHosts(v) => {
                v.validate()?;
            }
            Self::TestConnection(_) => {}
            Self::ConnectionsQuery(v) => {
                v.validate()?;
            }
            Self::ReinstallAgent(_) => {}
            Self::UninstallAgent(v) => {
                v.validate()?;
            }
            Self::InspectAgent(v) => {
                v.validate()?;
            }
            Self::SshResolve(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::configuration::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::SaveServer(v) => {
                v.validate()?;
            }
            Self::DeleteServer(v) => {
                v.validate()?;
            }
            Self::ForwardDiscoveredPort(v) => {
                v.validate()?;
            }
            Self::ForwardTunnelBatch(v) => {
                v.validate()?;
            }
            Self::SaveTunnel(v) => {
                v.validate()?;
            }
            Self::UpdateTunnel(v) => {
                v.validate()?;
            }
            Self::DeleteTunnel(v) => {
                v.validate()?;
            }
            Self::DeleteTunnels(v) => {
                v.validate()?;
            }
            Self::SetTunnelConnected(v) => {
                v.validate()?;
            }
            Self::SetClipboardEnabled(v) => {
                v.validate()?;
            }
            Self::SetIntegrationEnabled(v) => {
                v.validate()?;
            }
            Self::OpenTunnel(v) => {
                v.validate()?;
            }
            Self::OpenTerminalLink(v) => {
                v.validate()?;
            }
            Self::Snapshot(_) => {}
        }

        Ok(())
    }
}

impl crate::desktop::configuration::Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::SaveServer(_) => {}
            Self::DeleteServer(_) => {}
            Self::ForwardDiscoveredPort(_) => {}
            Self::ForwardTunnelBatch(v) => {
                v.validate()?;
            }
            Self::SaveTunnel(_) => {}
            Self::UpdateTunnel(_) => {}
            Self::DeleteTunnel(_) => {}
            Self::DeleteTunnels(_) => {}
            Self::SetTunnelConnected(_) => {}
            Self::SetClipboardEnabled(_) => {}
            Self::SetIntegrationEnabled(_) => {}
            Self::OpenTunnel(_) => {}
            Self::OpenTerminalLink(_) => {}
            Self::Snapshot(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::files::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::List(v) => {
                v.validate()?;
            }
            Self::Watch(v) => {
                v.validate()?;
            }
            Self::Preview(v) => {
                v.validate()?;
            }
            Self::ReadText(v) => {
                v.validate()?;
            }
            Self::ProjectIndex(v) => {
                v.validate()?;
            }
            Self::WriteText(v) => {
                v.validate()?;
            }
            Self::Cancel(v) => {
                v.validate()?;
            }
            Self::Progress(v) => {
                v.validate()?;
            }
            Self::Upload(v) => {
                v.validate()?;
            }
            Self::Download(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::files::Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::List(v) => {
                v.validate()?;
            }
            Self::Watch(v) => {
                v.validate()?;
            }
            Self::Preview(v) => {
                v.validate()?;
            }
            Self::ReadText(v) => {
                v.validate()?;
            }
            Self::ProjectIndex(v) => {
                v.validate()?;
            }
            Self::WriteText(v) => {
                v.validate()?;
            }
            Self::Cancel(_) => {}
            Self::Progress(v) => {
                v.validate()?;
            }
            Self::Upload(v) => {
                v.validate()?;
            }
            Self::Download(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::metrics::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Collect(v) => {
                v.validate()?;
            }
            Self::History(v) => {
                v.validate()?;
            }
            Self::Cache(_) => {}
            Self::ClearCache(_) => {}
        }

        Ok(())
    }
}

impl crate::desktop::metrics::Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Collect(v) => {
                v.validate()?;
            }
            Self::History(v) => {
                v.validate()?;
            }
            Self::Cache(v) => {
                v.validate()?;
            }
            Self::ClearCache(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::services::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Snapshot(v) => {
                v.validate()?;
            }
            Self::Logs(v) => {
                v.validate()?;
            }
            Self::UnitFile(v) => {
                v.validate()?;
            }
            Self::Action(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::services::Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Snapshot(_) => {}
            Self::Logs(_) => {}
            Self::UnitFile(_) => {}
            Self::Action(_) => {}
        }

        Ok(())
    }
}

impl crate::desktop::containers::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Snapshot(v) => {
                v.validate()?;
            }
            Self::Inspect(v) => {
                v.validate()?;
            }
            Self::Logs(v) => {
                v.validate()?;
            }
            Self::Action(v) => {
                v.validate()?;
            }
            Self::ProjectAction(v) => {
                v.validate()?;
            }
            Self::Remove(v) => {
                v.validate()?;
            }
            Self::Disk(v) => {
                v.validate()?;
            }
            Self::Prune(v) => {
                v.validate()?;
            }
            Self::ComposeProject(v) => {
                v.validate()?;
            }
            Self::ComposeRun(v) => {
                v.validate()?;
            }
            Self::ComposeJob(v) => {
                v.validate()?;
            }
            Self::ComposeCancel(v) => {
                v.validate()?;
            }
            Self::ComposeJobs(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::containers::Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Snapshot(_) => {}
            Self::Inspect(_) => {}
            Self::Logs(_) => {}
            Self::Action(_) => {}
            Self::ProjectAction(_) => {}
            Self::Remove(_) => {}
            Self::Disk(_) => {}
            Self::Prune(_) => {}
            Self::ComposeProject(_) => {}
            Self::ComposeRun(_) => {}
            Self::ComposeJob(_) => {}
            Self::ComposeCancel(_) => {}
            Self::ComposeJobs(_) => {}
        }

        Ok(())
    }
}

impl crate::desktop::preferences::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::SetWindow(v) => {
                v.validate()?;
            }
            Self::ProjectsList(v) => {
                v.validate()?;
            }
            Self::ProjectsSave(v) => {
                v.validate()?;
            }
            Self::ProjectsRemove(v) => {
                v.validate()?;
            }
            Self::Settings(_) => {}
            Self::SettingsSave(v) => {
                v.validate()?;
            }
            Self::Installed(_) => {}
            Self::InstalledSave(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::preferences::Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::SetWindow(_) => {}
            Self::ProjectsList(v) => {
                v.validate()?;
            }
            Self::ProjectsSave(v) => {
                v.validate()?;
            }
            Self::ProjectsRemove(_) => {}
            Self::Settings(v) => {
                v.validate()?;
            }
            Self::SettingsSave(v) => {
                v.validate()?;
            }
            Self::Installed(v) => {
                v.validate()?;
            }
            Self::InstalledSave(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::git::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Request(v) => {
                v.validate()?;
            }
            Self::RefreshRepository(v) => {
                v.validate()?;
            }
            Self::PendingOperations(v) => {
                v.validate()?;
            }
            Self::AcknowledgeOperation(v) => {
                v.validate()?;
            }
            Self::ReviewOperation(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::git::Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Request(v) => {
                v.validate()?;
            }
            Self::RefreshRepository(v) => {
                v.validate()?;
            }
            Self::PendingOperations(v) => {
                v.validate()?;
            }
            Self::AcknowledgeOperation(_) => {}
            Self::ReviewOperation(_) => {}
        }

        Ok(())
    }
}

impl crate::desktop::applications::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Status(v) => {
                v.validate()?;
            }
            Self::Catalogue(v) => {
                v.validate()?;
            }
            Self::Display(v) => {
                v.validate()?;
            }
            Self::Launch(v) => {
                v.validate()?;
            }
            Self::Show(v) => {
                v.validate()?;
            }
            Self::States(v) => {
                v.validate()?;
            }
            Self::Stop(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::applications::Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Status(v) => {
                v.validate()?;
            }
            Self::Catalogue(_) => {}
            Self::Display(v) => {
                v.validate()?;
            }
            Self::Launch(v) => {
                v.validate()?;
            }
            Self::Show(_) => {}
            Self::States(v) => {
                v.validate()?;
            }
            Self::Stop(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::host::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Activate(_) => {}
            Self::OpenRemoteApp(v) => {
                v.validate()?;
            }
            Self::StartupSettings(_) => {}
            Self::SetLaunchAtLogin(v) => {
                v.validate()?;
            }
            Self::UpdateStatus(_) => {}
            Self::CheckForUpdates(_) => {}
            Self::InstallUpdate(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::host::Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Activate(_) => {}
            Self::OpenRemoteApp(_) => {}
            Self::StartupSettings(v) => {
                v.validate()?;
            }
            Self::SetLaunchAtLogin(v) => {
                v.validate()?;
            }
            Self::UpdateStatus(v) => {
                v.validate()?;
            }
            Self::CheckForUpdates(_) => {}
            Self::InstallUpdate(_) => {}
        }

        Ok(())
    }
}

impl crate::desktop::events::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Poll(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::events::Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Poll(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::terminal::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Close(v) => {
                v.validate()?;
            }
            Self::List(v) => {
                v.validate()?;
            }
            Self::Rename(v) => {
                v.validate()?;
            }
            Self::End(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::terminal::Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Close(_) => {}
            Self::List(v) => {
                v.validate()?;
            }
            Self::Rename(v) => {
                v.validate()?;
            }
            Self::End(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::stream::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::TerminalOpen(v) => {
                v.validate()?;
            }
            Self::CommandOpen(v) => {
                v.validate()?;
            }
            Self::RemoteAppsOpen(v) => {
                v.validate()?;
            }
            Self::LspOpen(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::Operation {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Account(v) => {
                v.validate()?;
            }
            Self::Connection(v) => {
                v.validate()?;
            }
            Self::Configuration(v) => {
                v.validate()?;
            }
            Self::Files(v) => {
                v.validate()?;
            }
            Self::Metrics(v) => {
                v.validate()?;
            }
            Self::Services(v) => {
                v.validate()?;
            }
            Self::Containers(v) => {
                v.validate()?;
            }
            Self::Preferences(v) => {
                v.validate()?;
            }
            Self::Git(v) => {
                v.validate()?;
            }
            Self::Applications(v) => {
                v.validate()?;
            }
            Self::Host(v) => {
                v.validate()?;
            }
            Self::Events(v) => {
                v.validate()?;
            }
            Self::Terminal(v) => {
                v.validate()?;
            }
            Self::Stream(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::Reply {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Account(v) => {
                v.validate()?;
            }
            Self::Connection(v) => {
                v.validate()?;
            }
            Self::Configuration(v) => {
                v.validate()?;
            }
            Self::Files(v) => {
                v.validate()?;
            }
            Self::Metrics(v) => {
                v.validate()?;
            }
            Self::Services(v) => {
                v.validate()?;
            }
            Self::Containers(v) => {
                v.validate()?;
            }
            Self::Preferences(v) => {
                v.validate()?;
            }
            Self::Git(v) => {
                v.validate()?;
            }
            Self::Applications(v) => {
                v.validate()?;
            }
            Self::Host(v) => {
                v.validate()?;
            }
            Self::Events(v) => {
                v.validate()?;
            }
            Self::Terminal(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::Request {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.operation.validate()?;

        Ok(())
    }
}

impl crate::desktop::Outcome {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Completed(v) => {
                v.validate()?;
            }
            Self::Failed(v) => {
                v.validate()?;
            }
            Self::Opened(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::Response {
    pub fn validate(&self) -> Result<(), Invalid> {
        self.outcome.validate()?;

        Ok(())
    }
}

impl crate::desktop::ClientMessage {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Hello(v) => {
                v.validate()?;
            }
            Self::Request(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}

impl crate::desktop::ServerMessage {
    pub fn validate(&self) -> Result<(), Invalid> {
        match self {
            Self::Ready(v) => {
                v.validate()?;
            }
            Self::Refused(_) => {}
            Self::Progress(v) => {
                v.validate()?;
            }
            Self::Response(v) => {
                v.validate()?;
            }
        }

        Ok(())
    }
}
