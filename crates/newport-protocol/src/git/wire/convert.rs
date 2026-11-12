use super::{Error, Invalid, pb};
use crate::git as contract;
use base64::{Engine, engine::general_purpose::STANDARD};

#[cfg(test)]
mod tests;

fn required<T>(value: Option<T>) -> Result<T, Error> {
    value.ok_or_else(|| Invalid("missing field").into())
}
pub(crate) fn path_to_wire(value: &contract::Path) -> Result<pb::Path, Error> {
    Ok(pb::Path {
        data: Some(
            STANDARD
                .decode(&value.bytes_b64)
                .map_err(|_| Invalid("binary text encoding"))?
                .into(),
        ),
        display: Some(value.display.clone()),
    })
}
pub(crate) fn path_from_wire(value: pb::Path) -> Result<contract::Path, Error> {
    Ok(contract::Path {
        bytes_b64: STANDARD.encode(required(value.data)?),
        display: required(value.display)?,
    })
}
fn side_to_wire(value: &contract::Side) -> i32 {
    match value {
        contract::Side::HeadToIndex => pb::Side::HeadToIndex as i32,
        contract::Side::IndexToWorktree => pb::Side::IndexToWorktree as i32,
        contract::Side::HeadToWorktree => pb::Side::HeadToWorktree as i32,
    }
}
fn side_from_wire(value: i32) -> Result<contract::Side, Error> {
    Ok(match pb::Side::try_from(value) {
        Ok(pb::Side::HeadToIndex) => contract::Side::HeadToIndex,
        Ok(pb::Side::IndexToWorktree) => contract::Side::IndexToWorktree,
        Ok(pb::Side::HeadToWorktree) => contract::Side::HeadToWorktree,
        Ok(pb::Side::Unspecified) | Err(_) => return Err(Invalid("enumeration").into()),
    })
}
fn reset_mode_to_wire(value: &contract::ResetMode) -> i32 {
    match value {
        contract::ResetMode::Soft => pb::ResetMode::Soft as i32,
        contract::ResetMode::Mixed => pb::ResetMode::Mixed as i32,
        contract::ResetMode::Hard => pb::ResetMode::Hard as i32,
    }
}
fn reset_mode_from_wire(value: i32) -> Result<contract::ResetMode, Error> {
    Ok(match pb::ResetMode::try_from(value) {
        Ok(pb::ResetMode::Soft) => contract::ResetMode::Soft,
        Ok(pb::ResetMode::Mixed) => contract::ResetMode::Mixed,
        Ok(pb::ResetMode::Hard) => contract::ResetMode::Hard,
        Ok(pb::ResetMode::Unspecified) | Err(_) => return Err(Invalid("enumeration").into()),
    })
}
fn discard_source_to_wire(value: &contract::DiscardSource) -> i32 {
    match value {
        contract::DiscardSource::Index => pb::DiscardSource::Index as i32,
        contract::DiscardSource::Head => pb::DiscardSource::Head as i32,
    }
}
fn discard_source_from_wire(value: i32) -> Result<contract::DiscardSource, Error> {
    Ok(match pb::DiscardSource::try_from(value) {
        Ok(pb::DiscardSource::Index) => contract::DiscardSource::Index,
        Ok(pb::DiscardSource::Head) => contract::DiscardSource::Head,
        Ok(pb::DiscardSource::Unspecified) | Err(_) => return Err(Invalid("enumeration").into()),
    })
}
fn conflict_side_to_wire(value: &contract::ConflictSide) -> i32 {
    match value {
        contract::ConflictSide::Base => pb::ConflictSide::Base as i32,
        contract::ConflictSide::Ours => pb::ConflictSide::Ours as i32,
        contract::ConflictSide::Theirs => pb::ConflictSide::Theirs as i32,
    }
}
fn conflict_side_from_wire(value: i32) -> Result<contract::ConflictSide, Error> {
    Ok(match pb::ConflictSide::try_from(value) {
        Ok(pb::ConflictSide::Base) => contract::ConflictSide::Base,
        Ok(pb::ConflictSide::Ours) => contract::ConflictSide::Ours,
        Ok(pb::ConflictSide::Theirs) => contract::ConflictSide::Theirs,
        Ok(pb::ConflictSide::Unspecified) | Err(_) => return Err(Invalid("enumeration").into()),
    })
}
fn diff_line_encoding_to_wire(value: &contract::DiffLineEncoding) -> i32 {
    match value {
        contract::DiffLineEncoding::TupleV1 => pb::DiffLineEncoding::TupleV1 as i32,
    }
}
fn diff_line_encoding_from_wire(value: i32) -> Result<contract::DiffLineEncoding, Error> {
    Ok(match pb::DiffLineEncoding::try_from(value) {
        Ok(pb::DiffLineEncoding::TupleV1) => contract::DiffLineEncoding::TupleV1,
        Ok(pb::DiffLineEncoding::Unspecified) | Err(_) => return Err(Invalid("enumeration").into()),
    })
}
fn operation_state_to_wire(value: &contract::OperationState) -> i32 {
    match value {
        contract::OperationState::Running => pb::OperationState::Running as i32,
        contract::OperationState::Succeeded => pb::OperationState::Succeeded as i32,
        contract::OperationState::Failed => pb::OperationState::Failed as i32,
        contract::OperationState::NeedsResolution => pb::OperationState::NeedsResolution as i32,
        contract::OperationState::OutcomeUnknown => pb::OperationState::OutcomeUnknown as i32,
        contract::OperationState::ReviewedUnknown => pb::OperationState::ReviewedUnknown as i32,
    }
}
fn operation_state_from_wire(value: i32) -> Result<contract::OperationState, Error> {
    Ok(match pb::OperationState::try_from(value) {
        Ok(pb::OperationState::Running) => contract::OperationState::Running,
        Ok(pb::OperationState::Succeeded) => contract::OperationState::Succeeded,
        Ok(pb::OperationState::Failed) => contract::OperationState::Failed,
        Ok(pb::OperationState::NeedsResolution) => contract::OperationState::NeedsResolution,
        Ok(pb::OperationState::OutcomeUnknown) => contract::OperationState::OutcomeUnknown,
        Ok(pb::OperationState::ReviewedUnknown) => contract::OperationState::ReviewedUnknown,
        Ok(pb::OperationState::Unspecified) | Err(_) => return Err(Invalid("enumeration").into()),
    })
}
pub(super) fn status_filter_to_wire(
    value: &contract::StatusFilter,
) -> Result<pb::StatusFilter, Error> {
    Ok(pb::StatusFilter {
        text: Some(value.text.clone()),
        group: value.group.clone(),
    })
}
pub(super) fn status_filter_from_wire(
    value: pb::StatusFilter,
) -> Result<contract::StatusFilter, Error> {
    Ok(contract::StatusFilter {
        text: required(value.text)?,
        group: value.group,
    })
}
pub(super) fn author_to_wire(value: &contract::Author) -> Result<pb::Author, Error> {
    Ok(pb::Author {
        name: Some(value.name.clone()),
        email: Some(value.email.clone()),
    })
}
pub(super) fn author_from_wire(value: pb::Author) -> Result<contract::Author, Error> {
    Ok(contract::Author {
        name: required(value.name)?,
        email: required(value.email)?,
    })
}
pub(super) fn tag_annotation_to_wire(
    value: &contract::TagAnnotation,
) -> Result<pb::TagAnnotation, Error> {
    Ok(pb::TagAnnotation {
        message: Some(value.message.clone()),
        author: value.author.as_ref().map(author_to_wire).transpose()?,
    })
}
pub(super) fn tag_annotation_from_wire(
    value: pb::TagAnnotation,
) -> Result<contract::TagAnnotation, Error> {
    Ok(contract::TagAnnotation {
        message: required(value.message)?,
        author: value.author.map(author_from_wire).transpose()?,
    })
}
pub(super) fn hunk_selection_to_wire(
    value: &contract::HunkSelection,
) -> Result<pb::HunkSelection, Error> {
    Ok(pb::HunkSelection {
        ids: value.ids.clone(),
        lines: value
            .lines
            .as_ref()
            .map(|item| {
                Ok::<_, Error>(pb::StringList {
                    items: item.clone(),
                })
            })
            .transpose()?,
        context_lines: Some(value.context_lines),
    })
}
pub(super) fn hunk_selection_from_wire(
    value: pb::HunkSelection,
) -> Result<contract::HunkSelection, Error> {
    Ok(contract::HunkSelection {
        ids: value.ids,
        lines: value
            .lines
            .map(|item| Ok::<_, Error>(item.items))
            .transpose()?,
        context_lines: required(value.context_lines)?,
    })
}
pub(crate) fn command_log_to_wire(value: &contract::CommandLog) -> Result<pb::CommandLog, Error> {
    Ok(pb::CommandLog {
        command: Some(value.command.clone()),
        duration_ms: Some(value.duration_ms),
        exit_code: value.exit_code,
        output: Some(value.output.clone()),
        interrupted: Some(value.interrupted),
    })
}
pub(crate) fn command_log_from_wire(value: pb::CommandLog) -> Result<contract::CommandLog, Error> {
    Ok(contract::CommandLog {
        command: required(value.command)?,
        duration_ms: required(value.duration_ms)?,
        exit_code: value.exit_code,
        output: required(value.output)?,
        interrupted: required(value.interrupted)?,
    })
}
pub(super) fn object_id_to_wire(value: &contract::ObjectId) -> Result<pb::ObjectId, Error> {
    Ok(pb::ObjectId {
        algorithm: Some(value.algorithm.clone()),
        hex: Some(value.hex.clone()),
    })
}
pub(super) fn object_id_from_wire(value: pb::ObjectId) -> Result<contract::ObjectId, Error> {
    Ok(contract::ObjectId {
        algorithm: required(value.algorithm)?,
        hex: required(value.hex)?,
    })
}
pub(super) fn tag_object_id_to_wire(
    value: &contract::TagObjectId,
) -> Result<pb::TagObjectId, Error> {
    Ok(pb::TagObjectId {
        format: Some(value.format.clone()),
        hex: Some(value.hex.clone()),
    })
}
pub(super) fn tag_object_id_from_wire(
    value: pb::TagObjectId,
) -> Result<contract::TagObjectId, Error> {
    Ok(contract::TagObjectId {
        format: required(value.format)?,
        hex: required(value.hex)?,
    })
}
pub(super) fn head_to_wire(value: &contract::Head) -> Result<pb::Head, Error> {
    Ok(pb::Head {
        oid: value.oid.as_ref().map(object_id_to_wire).transpose()?,
        name: value.name.as_ref().map(path_to_wire).transpose()?,
        detached: Some(value.detached),
        unborn: Some(value.unborn),
    })
}
pub(super) fn head_from_wire(value: pb::Head) -> Result<contract::Head, Error> {
    Ok(contract::Head {
        oid: value.oid.map(object_id_from_wire).transpose()?,
        name: value.name.map(path_from_wire).transpose()?,
        detached: required(value.detached)?,
        unborn: required(value.unborn)?,
    })
}
pub(super) fn integration_state_to_wire(
    value: &contract::IntegrationState,
) -> Result<pb::IntegrationState, Error> {
    Ok(pb::IntegrationState {
        kind: Some(value.kind.clone()),
        managed: Some(value.managed),
        can_continue: Some(value.can_continue),
        can_abort: Some(value.can_abort),
        can_skip: Some(value.can_skip),
    })
}
pub(super) fn integration_state_from_wire(
    value: pb::IntegrationState,
) -> Result<contract::IntegrationState, Error> {
    Ok(contract::IntegrationState {
        kind: required(value.kind)?,
        managed: required(value.managed)?,
        can_continue: required(value.can_continue)?,
        can_abort: required(value.can_abort)?,
        can_skip: required(value.can_skip)?,
    })
}
pub(super) fn repository_capabilities_to_wire(
    value: &contract::RepositoryCapabilities,
) -> Result<pb::RepositoryCapabilities, Error> {
    Ok(pb::RepositoryCapabilities {
        read_only: Some(value.read_only),
        working_tree: Some(value.working_tree),
    })
}
pub(super) fn repository_capabilities_from_wire(
    value: pb::RepositoryCapabilities,
) -> Result<contract::RepositoryCapabilities, Error> {
    Ok(contract::RepositoryCapabilities {
        read_only: required(value.read_only)?,
        working_tree: required(value.working_tree)?,
    })
}
pub(super) fn repository_to_wire(value: &contract::Repository) -> Result<pb::Repository, Error> {
    Ok(pb::Repository {
        repo_id: Some(value.repo_id.clone()),
        common_repo_id: Some(value.common_repo_id.clone()),
        root: Some(path_to_wire(&value.root)?),
        bare: Some(value.bare),
        object_format: Some(value.object_format.clone()),
        head: Some(head_to_wire(&value.head)?),
        operation_state: Some(value.operation_state.clone()),
        integration: value
            .integration
            .as_ref()
            .map(integration_state_to_wire)
            .transpose()?,
        capabilities: Some(repository_capabilities_to_wire(&value.capabilities)?),
    })
}
pub(super) fn repository_from_wire(value: pb::Repository) -> Result<contract::Repository, Error> {
    Ok(contract::Repository {
        repo_id: required(value.repo_id)?,
        common_repo_id: required(value.common_repo_id)?,
        root: path_from_wire(required(value.root)?)?,
        bare: required(value.bare)?,
        object_format: required(value.object_format)?,
        head: head_from_wire(required(value.head)?)?,
        operation_state: required(value.operation_state)?,
        integration: value
            .integration
            .map(integration_state_from_wire)
            .transpose()?,
        capabilities: repository_capabilities_from_wire(required(value.capabilities)?)?,
    })
}
pub(super) fn group_counts_to_wire(
    value: &contract::GroupCounts,
) -> Result<pb::GroupCounts, Error> {
    Ok(pb::GroupCounts {
        staged: Some(value.staged),
        unstaged: Some(value.unstaged),
        untracked: Some(value.untracked),
        conflicted: Some(value.conflicted),
    })
}
pub(super) fn group_counts_from_wire(
    value: pb::GroupCounts,
) -> Result<contract::GroupCounts, Error> {
    Ok(contract::GroupCounts {
        staged: required(value.staged)?,
        unstaged: required(value.unstaged)?,
        untracked: required(value.untracked)?,
        conflicted: required(value.conflicted)?,
    })
}
pub(super) fn upstream_to_wire(value: &contract::Upstream) -> Result<pb::Upstream, Error> {
    Ok(pb::Upstream {
        remote: Some(path_to_wire(&value.remote)?),
        reference: Some(path_to_wire(&value.reference)?),
    })
}
pub(super) fn upstream_from_wire(value: pb::Upstream) -> Result<contract::Upstream, Error> {
    Ok(contract::Upstream {
        remote: path_from_wire(required(value.remote)?)?,
        reference: path_from_wire(required(value.reference)?)?,
    })
}
pub(super) fn status_summary_to_wire(
    value: &contract::StatusSummary,
) -> Result<pb::StatusSummary, Error> {
    Ok(pb::StatusSummary {
        head: Some(head_to_wire(&value.head)?),
        operation_state: Some(value.operation_state.clone()),
        integration: value
            .integration
            .as_ref()
            .map(integration_state_to_wire)
            .transpose()?,
        ahead: value.ahead,
        behind: value.behind,
        basis: Some(value.basis.clone()),
        upstream_ref: value.upstream_ref.as_ref().map(path_to_wire).transpose()?,
        upstream: value.upstream.as_ref().map(upstream_to_wire).transpose()?,
        truncated: Some(value.truncated),
        total_entries: Some(value.total_entries),
        group_counts: Some(group_counts_to_wire(&value.group_counts)?),
        matched_entries: value.matched_entries,
    })
}
pub(super) fn status_summary_from_wire(
    value: pb::StatusSummary,
) -> Result<contract::StatusSummary, Error> {
    Ok(contract::StatusSummary {
        head: head_from_wire(required(value.head)?)?,
        operation_state: required(value.operation_state)?,
        integration: value
            .integration
            .map(integration_state_from_wire)
            .transpose()?,
        ahead: value.ahead,
        behind: value.behind,
        basis: required(value.basis)?,
        upstream_ref: value.upstream_ref.map(path_from_wire).transpose()?,
        upstream: value.upstream.map(upstream_from_wire).transpose()?,
        truncated: required(value.truncated)?,
        total_entries: required(value.total_entries)?,
        group_counts: group_counts_from_wire(required(value.group_counts)?)?,
        matched_entries: value.matched_entries,
    })
}
pub(super) fn conflict_entry_to_wire(
    value: &contract::ConflictEntry,
) -> Result<pb::ConflictEntry, Error> {
    Ok(pb::ConflictEntry {
        mode: Some(value.mode),
        oid: Some(object_id_to_wire(&value.oid)?),
        path: Some(path_to_wire(&value.path)?),
    })
}
pub(super) fn conflict_entry_from_wire(
    value: pb::ConflictEntry,
) -> Result<contract::ConflictEntry, Error> {
    Ok(contract::ConflictEntry {
        mode: required(value.mode)?,
        oid: object_id_from_wire(required(value.oid)?)?,
        path: path_from_wire(required(value.path)?)?,
    })
}
pub(super) fn conflict_to_wire(value: &contract::Conflict) -> Result<pb::Conflict, Error> {
    Ok(pb::Conflict {
        base: value
            .base
            .as_ref()
            .map(conflict_entry_to_wire)
            .transpose()?,
        ours: value
            .ours
            .as_ref()
            .map(conflict_entry_to_wire)
            .transpose()?,
        theirs: value
            .theirs
            .as_ref()
            .map(conflict_entry_to_wire)
            .transpose()?,
    })
}
pub(super) fn conflict_from_wire(value: pb::Conflict) -> Result<contract::Conflict, Error> {
    Ok(contract::Conflict {
        base: value.base.map(conflict_entry_from_wire).transpose()?,
        ours: value.ours.map(conflict_entry_from_wire).transpose()?,
        theirs: value.theirs.map(conflict_entry_from_wire).transpose()?,
    })
}
pub(super) fn status_entry_to_wire(
    value: &contract::StatusEntry,
) -> Result<pb::StatusEntry, Error> {
    Ok(pb::StatusEntry {
        entry_id: Some(value.entry_id.clone()),
        path: Some(path_to_wire(&value.path)?),
        old_path: value.old_path.as_ref().map(path_to_wire).transpose()?,
        flags: Some(value.flags),
        staged: Some(value.staged),
        unstaged: Some(value.unstaged),
        untracked: Some(value.untracked),
        conflicted: Some(value.conflicted),
        conflict: value.conflict.as_ref().map(conflict_to_wire).transpose()?,
    })
}
pub(super) fn status_entry_from_wire(
    value: pb::StatusEntry,
) -> Result<contract::StatusEntry, Error> {
    Ok(contract::StatusEntry {
        entry_id: required(value.entry_id)?,
        path: path_from_wire(required(value.path)?)?,
        old_path: value.old_path.map(path_from_wire).transpose()?,
        flags: required(value.flags)?,
        staged: required(value.staged)?,
        unstaged: required(value.unstaged)?,
        untracked: required(value.untracked)?,
        conflicted: required(value.conflicted)?,
        conflict: value.conflict.map(conflict_from_wire).transpose()?,
    })
}
pub(super) fn configuration_entry_to_wire(
    value: &contract::ConfigurationEntry,
) -> Result<pb::ConfigurationEntry, Error> {
    Ok(pb::ConfigurationEntry {
        value: value.value.as_ref().map(path_to_wire).transpose()?,
        level: Some(value.level.clone()),
        include_depth: Some(value.include_depth),
    })
}
pub(super) fn configuration_entry_from_wire(
    value: pb::ConfigurationEntry,
) -> Result<contract::ConfigurationEntry, Error> {
    Ok(contract::ConfigurationEntry {
        value: value.value.map(path_from_wire).transpose()?,
        level: required(value.level)?,
        include_depth: required(value.include_depth)?,
    })
}
pub(super) fn tracking_configuration_to_wire(
    value: &contract::TrackingConfiguration,
) -> Result<pb::TrackingConfiguration, Error> {
    Ok(pb::TrackingConfiguration {
        remote: value
            .remote
            .iter()
            .map(configuration_entry_to_wire)
            .collect::<Result<_, _>>()?,
        merge: value
            .merge
            .iter()
            .map(configuration_entry_to_wire)
            .collect::<Result<_, _>>()?,
    })
}
pub(super) fn tracking_configuration_from_wire(
    value: pb::TrackingConfiguration,
) -> Result<contract::TrackingConfiguration, Error> {
    Ok(contract::TrackingConfiguration {
        remote: value
            .remote
            .into_iter()
            .map(configuration_entry_from_wire)
            .collect::<Result<_, _>>()?,
        merge: value
            .merge
            .into_iter()
            .map(configuration_entry_from_wire)
            .collect::<Result<_, _>>()?,
    })
}
pub(super) fn tracking_to_wire(value: &contract::Tracking) -> Result<pb::Tracking, Error> {
    Ok(pb::Tracking {
        token: Some(value.token.clone()),
        editable: Some(value.editable),
        configuration: Some(tracking_configuration_to_wire(&value.configuration)?),
    })
}
pub(super) fn tracking_from_wire(value: pb::Tracking) -> Result<contract::Tracking, Error> {
    Ok(contract::Tracking {
        token: required(value.token)?,
        editable: required(value.editable)?,
        configuration: tracking_configuration_from_wire(required(value.configuration)?)?,
    })
}
pub(super) fn branch_to_wire(value: &contract::Branch) -> Result<pb::Branch, Error> {
    Ok(pb::Branch {
        name: Some(path_to_wire(&value.name)?),
        reference: Some(path_to_wire(&value.reference)?),
        oid: Some(object_id_to_wire(&value.oid)?),
        remote: Some(value.remote),
        current: Some(value.current),
        upstream: value.upstream.as_ref().map(path_to_wire).transpose()?,
        tracking: value.tracking.as_ref().map(tracking_to_wire).transpose()?,
    })
}
pub(super) fn branch_from_wire(value: pb::Branch) -> Result<contract::Branch, Error> {
    Ok(contract::Branch {
        name: path_from_wire(required(value.name)?)?,
        reference: path_from_wire(required(value.reference)?)?,
        oid: object_id_from_wire(required(value.oid)?)?,
        remote: required(value.remote)?,
        current: required(value.current)?,
        upstream: value.upstream.map(path_from_wire).transpose()?,
        tracking: value.tracking.map(tracking_from_wire).transpose()?,
    })
}
pub(super) fn total_entries_to_wire(
    value: &contract::TotalEntries,
) -> Result<pb::TotalEntries, Error> {
    Ok(pb::TotalEntries {
        total_entries: Some(value.total_entries),
    })
}
pub(super) fn total_entries_from_wire(
    value: pb::TotalEntries,
) -> Result<contract::TotalEntries, Error> {
    Ok(contract::TotalEntries {
        total_entries: required(value.total_entries)?,
    })
}
pub(super) fn worktree_to_wire(value: &contract::Worktree) -> Result<pb::Worktree, Error> {
    Ok(pb::Worktree {
        name: value.name.as_ref().map(path_to_wire).transpose()?,
        kind: Some(value.kind.clone()),
        path: Some(path_to_wire(&value.path)?),
        git_dir: Some(path_to_wire(&value.git_dir)?),
        current: Some(value.current),
        state: Some(value.state.clone()),
        head: Some(head_to_wire(&value.head)?),
        locked: Some(value.locked),
        lock_reason: value.lock_reason.as_ref().map(path_to_wire).transpose()?,
        prunable: Some(value.prunable),
    })
}
pub(super) fn worktree_from_wire(value: pb::Worktree) -> Result<contract::Worktree, Error> {
    Ok(contract::Worktree {
        name: value.name.map(path_from_wire).transpose()?,
        kind: required(value.kind)?,
        path: path_from_wire(required(value.path)?)?,
        git_dir: path_from_wire(required(value.git_dir)?)?,
        current: required(value.current)?,
        state: required(value.state)?,
        head: head_from_wire(required(value.head)?)?,
        locked: required(value.locked)?,
        lock_reason: value.lock_reason.map(path_from_wire).transpose()?,
        prunable: required(value.prunable)?,
    })
}
pub(super) fn worktree_metadata_to_wire(
    value: &contract::WorktreeMetadata,
) -> Result<pb::WorktreeMetadata, Error> {
    Ok(pb::WorktreeMetadata {
        list_token: Some(value.list_token.clone()),
        total_entries: Some(value.total_entries),
        matching_entries: Some(value.matching_entries),
        current: value.current.as_ref().map(worktree_to_wire).transpose()?,
        main: value.main.as_ref().map(worktree_to_wire).transpose()?,
    })
}
pub(super) fn worktree_metadata_from_wire(
    value: pb::WorktreeMetadata,
) -> Result<contract::WorktreeMetadata, Error> {
    Ok(contract::WorktreeMetadata {
        list_token: required(value.list_token)?,
        total_entries: required(value.total_entries)?,
        matching_entries: required(value.matching_entries)?,
        current: value.current.map(worktree_from_wire).transpose()?,
        main: value.main.map(worktree_from_wire).transpose()?,
    })
}
pub(super) fn remote_to_wire(value: &contract::Remote) -> Result<pb::Remote, Error> {
    Ok(pb::Remote {
        name: Some(value.name.clone()),
        url: value.url.clone(),
        push_url: value.push_url.clone(),
        token: Some(value.token.clone()),
    })
}
pub(super) fn remote_from_wire(value: pb::Remote) -> Result<contract::Remote, Error> {
    Ok(contract::Remote {
        name: required(value.name)?,
        url: value.url,
        push_url: value.push_url,
        token: required(value.token)?,
    })
}
pub(super) fn remote_authentication_to_wire(
    value: &contract::RemoteAuthentication,
) -> Result<pb::RemoteAuthentication, Error> {
    Ok(pb::RemoteAuthentication {
        ssh: Some(value.ssh.clone()),
        https: Some(value.https.clone()),
    })
}
pub(super) fn remote_authentication_from_wire(
    value: pb::RemoteAuthentication,
) -> Result<contract::RemoteAuthentication, Error> {
    Ok(contract::RemoteAuthentication {
        ssh: required(value.ssh)?,
        https: required(value.https)?,
    })
}
pub(super) fn repository_configuration_to_wire(
    value: &contract::RepositoryConfiguration,
) -> Result<pb::RepositoryConfiguration, Error> {
    Ok(pb::RepositoryConfiguration {
        fetch_prune: Some(value.fetch_prune),
        user_name: value.user_name.clone(),
        user_email: value.user_email.clone(),
        token: Some(value.token.clone()),
    })
}
pub(super) fn repository_configuration_from_wire(
    value: pb::RepositoryConfiguration,
) -> Result<contract::RepositoryConfiguration, Error> {
    Ok(contract::RepositoryConfiguration {
        fetch_prune: required(value.fetch_prune)?,
        user_name: value.user_name,
        user_email: value.user_email,
        token: required(value.token)?,
    })
}
pub(super) fn remotes_to_wire(value: &contract::Remotes) -> Result<pb::Remotes, Error> {
    Ok(pb::Remotes {
        entries: value
            .entries
            .iter()
            .map(remote_to_wire)
            .collect::<Result<_, _>>()?,
        authentication: Some(remote_authentication_to_wire(&value.authentication)?),
        configuration: Some(repository_configuration_to_wire(&value.configuration)?),
    })
}
pub(super) fn remotes_from_wire(value: pb::Remotes) -> Result<contract::Remotes, Error> {
    Ok(contract::Remotes {
        entries: value
            .entries
            .into_iter()
            .map(remote_from_wire)
            .collect::<Result<_, _>>()?,
        authentication: remote_authentication_from_wire(required(value.authentication)?)?,
        configuration: repository_configuration_from_wire(required(value.configuration)?)?,
    })
}
pub(super) fn remote_name_to_wire(value: &contract::RemoteName) -> Result<pb::RemoteName, Error> {
    Ok(pb::RemoteName {
        name: Some(value.name.clone()),
    })
}
pub(super) fn remote_name_from_wire(value: pb::RemoteName) -> Result<contract::RemoteName, Error> {
    Ok(contract::RemoteName {
        name: required(value.name)?,
    })
}
pub(super) fn remote_reference_to_wire(
    value: &contract::RemoteReference,
) -> Result<pb::RemoteReference, Error> {
    Ok(pb::RemoteReference {
        reference: Some(path_to_wire(&value.reference)?),
        kind: Some(value.kind.clone()),
        oid: Some(object_id_to_wire(&value.oid)?),
        symbolic_target: value
            .symbolic_target
            .as_ref()
            .map(path_to_wire)
            .transpose()?,
    })
}
pub(super) fn remote_reference_from_wire(
    value: pb::RemoteReference,
) -> Result<contract::RemoteReference, Error> {
    Ok(contract::RemoteReference {
        reference: path_from_wire(required(value.reference)?)?,
        kind: required(value.kind)?,
        oid: object_id_from_wire(required(value.oid)?)?,
        symbolic_target: value.symbolic_target.map(path_from_wire).transpose()?,
    })
}
pub(super) fn remote_reference_metadata_to_wire(
    value: &contract::RemoteReferenceMetadata,
) -> Result<pb::RemoteReferenceMetadata, Error> {
    Ok(pb::RemoteReferenceMetadata {
        remote: Some(value.remote.clone()),
        remote_token: Some(value.remote_token.clone()),
        for_push: Some(value.for_push),
        basis: Some(value.basis.clone()),
        truncated: Some(value.truncated),
        total_entries: Some(value.total_entries),
    })
}
pub(super) fn remote_reference_metadata_from_wire(
    value: pb::RemoteReferenceMetadata,
) -> Result<contract::RemoteReferenceMetadata, Error> {
    Ok(contract::RemoteReferenceMetadata {
        remote: required(value.remote)?,
        remote_token: required(value.remote_token)?,
        for_push: required(value.for_push)?,
        basis: required(value.basis)?,
        truncated: required(value.truncated)?,
        total_entries: required(value.total_entries)?,
    })
}
pub(super) fn blob_to_wire(value: &contract::Blob) -> Result<pb::Blob, Error> {
    Ok(pb::Blob {
        oid: Some(object_id_to_wire(&value.oid)?),
        size: Some(value.size),
        bytes_b64: value
            .bytes_b64
            .as_ref()
            .map(|item| Ok::<_, Error>(item.0.clone().into()))
            .transpose()?,
        truncated: Some(value.truncated),
    })
}
pub(super) fn blob_from_wire(value: pb::Blob) -> Result<contract::Blob, Error> {
    Ok(contract::Blob {
        oid: object_id_from_wire(required(value.oid)?)?,
        size: required(value.size)?,
        bytes_b64: value
            .bytes_b64
            .map(|item| Ok::<_, Error>(contract::Binary(item.to_vec())))
            .transpose()?,
        truncated: required(value.truncated)?,
    })
}
pub(super) fn blob_fragment_to_wire(
    value: &contract::BlobFragment,
) -> Result<pb::BlobFragment, Error> {
    Ok(pb::BlobFragment {
        offset: Some(value.offset),
        bytes_b64: Some(value.bytes_b64.0.clone().into()),
    })
}
pub(super) fn blob_fragment_from_wire(
    value: pb::BlobFragment,
) -> Result<contract::BlobFragment, Error> {
    Ok(contract::BlobFragment {
        offset: required(value.offset)?,
        bytes_b64: contract::Binary(required(value.bytes_b64)?.to_vec()),
    })
}
pub(super) fn blob_metadata_to_wire(
    value: &contract::BlobMetadata,
) -> Result<pb::BlobMetadata, Error> {
    Ok(pb::BlobMetadata {
        oid: Some(object_id_to_wire(&value.oid)?),
        size: Some(value.size),
    })
}
pub(super) fn blob_metadata_from_wire(
    value: pb::BlobMetadata,
) -> Result<contract::BlobMetadata, Error> {
    Ok(contract::BlobMetadata {
        oid: object_id_from_wire(required(value.oid)?)?,
        size: required(value.size)?,
    })
}
pub(super) fn stash_to_wire(value: &contract::Stash) -> Result<pb::Stash, Error> {
    Ok(pb::Stash {
        index: Some(value.index),
        oid: Some(value.oid.clone()),
        previous_oid: Some(value.previous_oid.clone()),
        message: Some(value.message.clone()),
        message_truncated: Some(value.message_truncated),
        time: Some(value.time),
    })
}
pub(super) fn stash_from_wire(value: pb::Stash) -> Result<contract::Stash, Error> {
    Ok(contract::Stash {
        index: required(value.index)?,
        oid: required(value.oid)?,
        previous_oid: required(value.previous_oid)?,
        message: required(value.message)?,
        message_truncated: required(value.message_truncated)?,
        time: required(value.time)?,
    })
}
pub(super) fn stash_metadata_to_wire(
    value: &contract::StashMetadata,
) -> Result<pb::StashMetadata, Error> {
    Ok(pb::StashMetadata {
        list_token: Some(value.list_token.clone()),
        total_entries: Some(value.total_entries),
    })
}
pub(super) fn stash_metadata_from_wire(
    value: pb::StashMetadata,
) -> Result<contract::StashMetadata, Error> {
    Ok(contract::StashMetadata {
        list_token: required(value.list_token)?,
        total_entries: required(value.total_entries)?,
    })
}
pub(super) fn commit_to_wire(value: &contract::Commit) -> Result<pb::Commit, Error> {
    Ok(pb::Commit {
        oid: Some(object_id_to_wire(&value.oid)?),
        parents: value
            .parents
            .iter()
            .map(object_id_to_wire)
            .collect::<Result<_, _>>()?,
        message: Some(path_to_wire(&value.message)?),
        message_truncated: Some(value.message_truncated),
        author: Some(author_to_wire(&value.author)?),
        time: Some(value.time),
        offset_minutes: Some(value.offset_minutes),
    })
}
pub(super) fn commit_from_wire(value: pb::Commit) -> Result<contract::Commit, Error> {
    Ok(contract::Commit {
        oid: object_id_from_wire(required(value.oid)?)?,
        parents: value
            .parents
            .into_iter()
            .map(object_id_from_wire)
            .collect::<Result<_, _>>()?,
        message: path_from_wire(required(value.message)?)?,
        message_truncated: required(value.message_truncated)?,
        author: author_from_wire(required(value.author)?)?,
        time: required(value.time)?,
        offset_minutes: required(value.offset_minutes)?,
    })
}
pub(super) fn history_metadata_to_wire(
    value: &contract::HistoryMetadata,
) -> Result<pb::HistoryMetadata, Error> {
    Ok(pb::HistoryMetadata {
        resolved_revision: value
            .resolved_revision
            .as_ref()
            .map(object_id_to_wire)
            .transpose()?,
        truncated: value.truncated,
    })
}
pub(super) fn history_metadata_from_wire(
    value: pb::HistoryMetadata,
) -> Result<contract::HistoryMetadata, Error> {
    Ok(contract::HistoryMetadata {
        resolved_revision: value
            .resolved_revision
            .map(object_id_from_wire)
            .transpose()?,
        truncated: value.truncated,
    })
}
pub(super) fn tagger_to_wire(value: &contract::Tagger) -> Result<pb::Tagger, Error> {
    Ok(pb::Tagger {
        name: Some(value.name.clone()),
        email: Some(value.email.clone()),
        time: Some(value.time),
        offset_minutes: Some(value.offset_minutes),
    })
}
pub(super) fn tagger_from_wire(value: pb::Tagger) -> Result<contract::Tagger, Error> {
    Ok(contract::Tagger {
        name: required(value.name)?,
        email: required(value.email)?,
        time: required(value.time)?,
        offset_minutes: required(value.offset_minutes)?,
    })
}
pub(super) fn tag_to_wire(value: &contract::Tag) -> Result<pb::Tag, Error> {
    Ok(pb::Tag {
        name: value.name.as_ref().map(path_to_wire).transpose()?,
        reference: value.reference.as_ref().map(path_to_wire).transpose()?,
        symbolic_target: value
            .symbolic_target
            .as_ref()
            .map(path_to_wire)
            .transpose()?,
        oid: Some(tag_object_id_to_wire(&value.oid)?),
        annotated: Some(value.annotated),
        details_omitted: Some(value.details_omitted),
        object_type: value.object_type.clone(),
        target_oid: value
            .target_oid
            .as_ref()
            .map(tag_object_id_to_wire)
            .transpose()?,
        peeled_oid: value
            .peeled_oid
            .as_ref()
            .map(tag_object_id_to_wire)
            .transpose()?,
        peeled_type: value.peeled_type.clone(),
        message: value.message.as_ref().map(path_to_wire).transpose()?,
        message_truncated: value.message_truncated,
        tagger: value.tagger.as_ref().map(tagger_to_wire).transpose()?,
    })
}
pub(super) fn tag_from_wire(value: pb::Tag) -> Result<contract::Tag, Error> {
    Ok(contract::Tag {
        name: value.name.map(path_from_wire).transpose()?,
        reference: value.reference.map(path_from_wire).transpose()?,
        symbolic_target: value.symbolic_target.map(path_from_wire).transpose()?,
        oid: tag_object_id_from_wire(required(value.oid)?)?,
        annotated: required(value.annotated)?,
        details_omitted: required(value.details_omitted)?,
        object_type: value.object_type,
        target_oid: value.target_oid.map(tag_object_id_from_wire).transpose()?,
        peeled_oid: value.peeled_oid.map(tag_object_id_from_wire).transpose()?,
        peeled_type: value.peeled_type,
        message: value.message.map(path_from_wire).transpose()?,
        message_truncated: value.message_truncated,
        tagger: value.tagger.map(tagger_from_wire).transpose()?,
    })
}
pub(super) fn delta_to_wire(value: &contract::Delta) -> Result<pb::Delta, Error> {
    Ok(pb::Delta {
        old_path: Some(path_to_wire(&value.old_path)?),
        new_path: Some(path_to_wire(&value.new_path)?),
        old_oid: value.old_oid.as_ref().map(object_id_to_wire).transpose()?,
        new_oid: value.new_oid.as_ref().map(object_id_to_wire).transpose()?,
        old_mode: Some(value.old_mode),
        new_mode: Some(value.new_mode),
        status: Some(value.status.clone()),
    })
}
pub(super) fn delta_from_wire(value: pb::Delta) -> Result<contract::Delta, Error> {
    Ok(contract::Delta {
        old_path: path_from_wire(required(value.old_path)?)?,
        new_path: path_from_wire(required(value.new_path)?)?,
        old_oid: value.old_oid.map(object_id_from_wire).transpose()?,
        new_oid: value.new_oid.map(object_id_from_wire).transpose()?,
        old_mode: required(value.old_mode)?,
        new_mode: required(value.new_mode)?,
        status: required(value.status)?,
    })
}
pub(super) fn diff_metadata_to_wire(
    value: &contract::DiffMetadata,
) -> Result<pb::DiffMetadata, Error> {
    Ok(pb::DiffMetadata {
        read_only: Some(value.read_only),
        commit_oid: value
            .commit_oid
            .as_ref()
            .map(object_id_to_wire)
            .transpose()?,
        parent_oid: value
            .parent_oid
            .as_ref()
            .map(object_id_to_wire)
            .transpose()?,
        parent_index: value.parent_index,
        parents: value
            .parents
            .as_ref()
            .map(|item| {
                Ok::<_, Error>(pb::ObjectIdList {
                    items: item
                        .iter()
                        .map(object_id_to_wire)
                        .collect::<Result<_, _>>()?,
                })
            })
            .transpose()?,
        source_snapshot: value.source_snapshot.clone(),
        entry_id: value.entry_id.clone(),
        side: value
            .side
            .as_ref()
            .map(|item| Ok::<_, Error>(side_to_wire(item)))
            .transpose()?,
        context_lines: value.context_lines,
        selected_path: value.selected_path.as_ref().map(path_to_wire).transpose()?,
        truncated: value.truncated,
        total_files: value.total_files,
        total_units: value.total_units,
        has_omissions: value.has_omissions,
    })
}
pub(super) fn diff_metadata_from_wire(
    value: pb::DiffMetadata,
) -> Result<contract::DiffMetadata, Error> {
    Ok(contract::DiffMetadata {
        read_only: required(value.read_only)?,
        commit_oid: value.commit_oid.map(object_id_from_wire).transpose()?,
        parent_oid: value.parent_oid.map(object_id_from_wire).transpose()?,
        parent_index: value.parent_index,
        parents: value
            .parents
            .map(|item| {
                item.items
                    .into_iter()
                    .map(object_id_from_wire)
                    .collect::<Result<_, _>>()
            })
            .transpose()?,
        source_snapshot: value.source_snapshot,
        entry_id: value.entry_id,
        side: value.side.map(side_from_wire).transpose()?,
        context_lines: value.context_lines,
        selected_path: value.selected_path.map(path_from_wire).transpose()?,
        truncated: value.truncated,
        total_files: value.total_files,
        total_units: value.total_units,
        has_omissions: value.has_omissions,
    })
}
pub(super) fn diff_line_to_wire(value: &contract::DiffLine) -> Result<pb::DiffLine, Error> {
    Ok(pb::DiffLine {
        id: value.id.clone(),
        origin: Some(value.origin.clone()),
        old_line: value.old_line,
        new_line: value.new_line,
        content: Some(path_to_wire(&value.content)?),
    })
}
pub(super) fn diff_line_from_wire(value: pb::DiffLine) -> Result<contract::DiffLine, Error> {
    Ok(contract::DiffLine {
        id: value.id,
        origin: required(value.origin)?,
        old_line: value.old_line,
        new_line: value.new_line,
        content: path_from_wire(required(value.content)?)?,
    })
}
pub(super) fn diff_hunk_to_wire(value: &contract::DiffHunk) -> Result<pb::DiffHunk, Error> {
    Ok(pb::DiffHunk {
        id: value.id.clone(),
        old_start: Some(value.old_start),
        old_lines: Some(value.old_lines),
        new_start: Some(value.new_start),
        new_lines: Some(value.new_lines),
        lines: value
            .lines
            .iter()
            .map(diff_line_to_wire)
            .collect::<Result<_, _>>()?,
    })
}
pub(super) fn diff_hunk_from_wire(value: pb::DiffHunk) -> Result<contract::DiffHunk, Error> {
    Ok(contract::DiffHunk {
        id: value.id,
        old_start: required(value.old_start)?,
        old_lines: required(value.old_lines)?,
        new_start: required(value.new_start)?,
        new_lines: required(value.new_lines)?,
        lines: value
            .lines
            .into_iter()
            .map(diff_line_from_wire)
            .collect::<Result<_, _>>()?,
    })
}
pub(super) fn diff_file_to_wire(value: &contract::DiffFile) -> Result<pb::DiffFile, Error> {
    Ok(pb::DiffFile {
        old_path: Some(path_to_wire(&value.old_path)?),
        new_path: Some(path_to_wire(&value.new_path)?),
        old_oid: value.old_oid.as_ref().map(object_id_to_wire).transpose()?,
        new_oid: value.new_oid.as_ref().map(object_id_to_wire).transpose()?,
        old_mode: Some(value.old_mode),
        new_mode: Some(value.new_mode),
        status: Some(value.status.clone()),
        binary: Some(value.binary),
        additions: Some(value.additions),
        deletions: Some(value.deletions),
        hunks: value
            .hunks
            .iter()
            .map(diff_hunk_to_wire)
            .collect::<Result<_, _>>()?,
    })
}
pub(super) fn diff_file_from_wire(value: pb::DiffFile) -> Result<contract::DiffFile, Error> {
    Ok(contract::DiffFile {
        old_path: path_from_wire(required(value.old_path)?)?,
        new_path: path_from_wire(required(value.new_path)?)?,
        old_oid: value.old_oid.map(object_id_from_wire).transpose()?,
        new_oid: value.new_oid.map(object_id_from_wire).transpose()?,
        old_mode: required(value.old_mode)?,
        new_mode: required(value.new_mode)?,
        status: required(value.status)?,
        binary: required(value.binary)?,
        additions: required(value.additions)?,
        deletions: required(value.deletions)?,
        hunks: value
            .hunks
            .into_iter()
            .map(diff_hunk_from_wire)
            .collect::<Result<_, _>>()?,
    })
}
pub(crate) fn diff_to_wire(value: &contract::Diff) -> Result<pb::Diff, Error> {
    Ok(pb::Diff {
        files: value
            .files
            .iter()
            .map(diff_file_to_wire)
            .collect::<Result<_, _>>()?,
        read_only: Some(value.read_only),
        commit_oid: value
            .commit_oid
            .as_ref()
            .map(object_id_to_wire)
            .transpose()?,
        parent_oid: value
            .parent_oid
            .as_ref()
            .map(object_id_to_wire)
            .transpose()?,
        parent_index: value.parent_index,
        parents: value
            .parents
            .as_ref()
            .map(|item| {
                Ok::<_, Error>(pb::ObjectIdList {
                    items: item
                        .iter()
                        .map(object_id_to_wire)
                        .collect::<Result<_, _>>()?,
                })
            })
            .transpose()?,
        source_snapshot: value.source_snapshot.clone(),
        entry_id: value.entry_id.clone(),
        side: value
            .side
            .as_ref()
            .map(|item| Ok::<_, Error>(side_to_wire(item)))
            .transpose()?,
        context_lines: value.context_lines,
        selected_path: value.selected_path.as_ref().map(path_to_wire).transpose()?,
        truncated: value.truncated,
        total_files: value.total_files,
        total_units: value.total_units,
        has_omissions: value.has_omissions,
    })
}
pub(crate) fn diff_from_wire(value: pb::Diff) -> Result<contract::Diff, Error> {
    Ok(contract::Diff {
        files: value
            .files
            .into_iter()
            .map(diff_file_from_wire)
            .collect::<Result<_, _>>()?,
        read_only: required(value.read_only)?,
        commit_oid: value.commit_oid.map(object_id_from_wire).transpose()?,
        parent_oid: value.parent_oid.map(object_id_from_wire).transpose()?,
        parent_index: value.parent_index,
        parents: value
            .parents
            .map(|item| {
                item.items
                    .into_iter()
                    .map(object_id_from_wire)
                    .collect::<Result<_, _>>()
            })
            .transpose()?,
        source_snapshot: value.source_snapshot,
        entry_id: value.entry_id,
        side: value.side.map(side_from_wire).transpose()?,
        context_lines: value.context_lines,
        selected_path: value.selected_path.map(path_from_wire).transpose()?,
        truncated: value.truncated,
        total_files: value.total_files,
        total_units: value.total_units,
        has_omissions: value.has_omissions,
    })
}
pub(super) fn line_fragment_to_wire(
    value: &contract::LineFragment,
) -> Result<pb::LineFragment, Error> {
    Ok(pb::LineFragment {
        line_index: Some(value.line_index),
        byte_offset: Some(value.byte_offset),
        line_complete: Some(value.line_complete),
        origin: Some(value.origin.clone()),
        old_line: value.old_line,
        new_line: value.new_line,
        content_bytes_b64: Some(value.content_bytes_b64.0.clone().into()),
        id: value.id.clone(),
    })
}
pub(super) fn line_fragment_from_wire(
    value: pb::LineFragment,
) -> Result<contract::LineFragment, Error> {
    Ok(contract::LineFragment {
        line_index: required(value.line_index)?,
        byte_offset: required(value.byte_offset)?,
        line_complete: required(value.line_complete)?,
        origin: required(value.origin)?,
        old_line: value.old_line,
        new_line: value.new_line,
        content_bytes_b64: contract::Binary(required(value.content_bytes_b64)?.to_vec()),
        id: value.id,
    })
}
pub(super) fn hunk_fragment_to_wire(
    value: &contract::HunkFragment,
) -> Result<pb::HunkFragment, Error> {
    Ok(pb::HunkFragment {
        id: value.id.clone(),
        index: Some(value.index),
        old_start: Some(value.old_start),
        old_lines: Some(value.old_lines),
        new_start: Some(value.new_start),
        new_lines: Some(value.new_lines),
        total_lines: Some(value.total_lines),
        lines: value
            .lines
            .iter()
            .map(line_fragment_to_wire)
            .collect::<Result<_, _>>()?,
    })
}
pub(super) fn hunk_fragment_from_wire(
    value: pb::HunkFragment,
) -> Result<contract::HunkFragment, Error> {
    Ok(contract::HunkFragment {
        id: value.id,
        index: required(value.index)?,
        old_start: required(value.old_start)?,
        old_lines: required(value.old_lines)?,
        new_start: required(value.new_start)?,
        new_lines: required(value.new_lines)?,
        total_lines: required(value.total_lines)?,
        lines: value
            .lines
            .into_iter()
            .map(line_fragment_from_wire)
            .collect::<Result<_, _>>()?,
    })
}
pub(super) fn file_fragment_to_wire(
    value: &contract::FileFragment,
) -> Result<pb::FileFragment, Error> {
    Ok(pb::FileFragment {
        old_path: Some(path_to_wire(&value.old_path)?),
        new_path: Some(path_to_wire(&value.new_path)?),
        old_oid: value.old_oid.as_ref().map(object_id_to_wire).transpose()?,
        new_oid: value.new_oid.as_ref().map(object_id_to_wire).transpose()?,
        old_mode: Some(value.old_mode),
        new_mode: Some(value.new_mode),
        status: Some(value.status.clone()),
        binary: Some(value.binary),
        additions: Some(value.additions),
        deletions: Some(value.deletions),
        file_index: Some(value.file_index),
        omission_reason: value.omission_reason.clone(),
        hunks: value
            .hunks
            .iter()
            .map(hunk_fragment_to_wire)
            .collect::<Result<_, _>>()?,
    })
}
pub(super) fn file_fragment_from_wire(
    value: pb::FileFragment,
) -> Result<contract::FileFragment, Error> {
    Ok(contract::FileFragment {
        old_path: path_from_wire(required(value.old_path)?)?,
        new_path: path_from_wire(required(value.new_path)?)?,
        old_oid: value.old_oid.map(object_id_from_wire).transpose()?,
        new_oid: value.new_oid.map(object_id_from_wire).transpose()?,
        old_mode: required(value.old_mode)?,
        new_mode: required(value.new_mode)?,
        status: required(value.status)?,
        binary: required(value.binary)?,
        additions: required(value.additions)?,
        deletions: required(value.deletions)?,
        file_index: required(value.file_index)?,
        omission_reason: value.omission_reason,
        hunks: value
            .hunks
            .into_iter()
            .map(hunk_fragment_from_wire)
            .collect::<Result<_, _>>()?,
    })
}
pub(super) fn operation_to_wire(value: &contract::Operation) -> Result<pb::Operation, Error> {
    Ok(pb::Operation {
        operation_id: Some(value.operation_id.clone()),
        repository: Some(value.repository.clone()),
        payload_hash: Some(value.payload_hash.clone()),
        state: Some(operation_state_to_wire(&value.state)),
        seq: Some(value.seq),
        result: value
            .result
            .as_ref()
            .map(operation_result_to_wire)
            .transpose()?,
        error: value.error.as_ref().map(failure_to_wire).transpose()?,
    })
}
pub(super) fn operation_from_wire(value: pb::Operation) -> Result<contract::Operation, Error> {
    Ok(contract::Operation {
        operation_id: required(value.operation_id)?,
        repository: required(value.repository)?,
        payload_hash: required(value.payload_hash)?,
        state: operation_state_from_wire(required(value.state)?)?,
        seq: required(value.seq)?,
        result: value.result.map(operation_result_from_wire).transpose()?,
        error: value.error.map(failure_from_wire).transpose()?,
    })
}
pub(super) fn closed_to_wire(value: &contract::Closed) -> Result<pb::Closed, Error> {
    Ok(pb::Closed {
        closed: Some(value.closed),
    })
}
pub(super) fn closed_from_wire(value: pb::Closed) -> Result<contract::Closed, Error> {
    Ok(contract::Closed {
        closed: required(value.closed)?,
    })
}
pub(super) fn stream_complete_to_wire(
    value: &contract::StreamComplete,
) -> Result<pb::StreamComplete, Error> {
    Ok(pb::StreamComplete {
        last_seq: Some(value.last_seq),
        bytes: Some(value.bytes),
    })
}
pub(super) fn stream_complete_from_wire(
    value: pb::StreamComplete,
) -> Result<contract::StreamComplete, Error> {
    Ok(contract::StreamComplete {
        last_seq: required(value.last_seq)?,
        bytes: required(value.bytes)?,
    })
}
pub(super) fn refresh_to_wire(value: &contract::Refresh) -> Result<pb::Refresh, Error> {
    Ok(pb::Refresh {
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn refresh_from_wire(value: pb::Refresh) -> Result<contract::Refresh, Error> {
    Ok(contract::Refresh {
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn committed_to_wire(value: &contract::Committed) -> Result<pb::Committed, Error> {
    Ok(pb::Committed {
        merge_completed: Some(value.merge_completed),
        commit_oid: Some(value.commit_oid.clone()),
        parent_oid: value.parent_oid.clone(),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn committed_from_wire(value: pb::Committed) -> Result<contract::Committed, Error> {
    Ok(contract::Committed {
        merge_completed: required(value.merge_completed)?,
        commit_oid: required(value.commit_oid)?,
        parent_oid: value.parent_oid,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn amended_to_wire(value: &contract::Amended) -> Result<pb::Amended, Error> {
    Ok(pb::Amended {
        amended: Some(value.amended),
        replaced_oid: Some(value.replaced_oid.clone()),
        commit_oid: Some(value.commit_oid.clone()),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn amended_from_wire(value: pb::Amended) -> Result<contract::Amended, Error> {
    Ok(contract::Amended {
        amended: required(value.amended)?,
        replaced_oid: required(value.replaced_oid)?,
        commit_oid: required(value.commit_oid)?,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn tracking_changed_to_wire(
    value: &contract::TrackingChanged,
) -> Result<pb::TrackingChanged, Error> {
    Ok(pb::TrackingChanged {
        tracking: Some(tracking_to_wire(&value.tracking)?),
        upstream: value.upstream.clone(),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn tracking_changed_from_wire(
    value: pb::TrackingChanged,
) -> Result<contract::TrackingChanged, Error> {
    Ok(contract::TrackingChanged {
        tracking: tracking_from_wire(required(value.tracking)?)?,
        upstream: value.upstream,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn checked_out_to_wire(value: &contract::CheckedOut) -> Result<pb::CheckedOut, Error> {
    Ok(pb::CheckedOut {
        detached: Some(value.detached),
        oid: value.oid.clone(),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn checked_out_from_wire(value: pb::CheckedOut) -> Result<contract::CheckedOut, Error> {
    Ok(contract::CheckedOut {
        detached: required(value.detached)?,
        oid: value.oid,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn remote_renamed_to_wire(
    value: &contract::RemoteRenamed,
) -> Result<pb::RemoteRenamed, Error> {
    Ok(pb::RemoteRenamed {
        renamed_references: Some(value.renamed_references),
        token: Some(value.token.clone()),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn remote_renamed_from_wire(
    value: pb::RemoteRenamed,
) -> Result<contract::RemoteRenamed, Error> {
    Ok(contract::RemoteRenamed {
        renamed_references: required(value.renamed_references)?,
        token: required(value.token)?,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn fast_forwarded_to_wire(
    value: &contract::FastForwarded,
) -> Result<pb::FastForwarded, Error> {
    Ok(pb::FastForwarded {
        fast_forwarded: Some(value.fast_forwarded),
        oid: Some(value.oid.clone()),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn fast_forwarded_from_wire(
    value: pb::FastForwarded,
) -> Result<contract::FastForwarded, Error> {
    Ok(contract::FastForwarded {
        fast_forwarded: required(value.fast_forwarded)?,
        oid: required(value.oid)?,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn lease_matched_to_wire(
    value: &contract::LeaseMatched,
) -> Result<pb::LeaseMatched, Error> {
    Ok(pb::LeaseMatched {
        lease_matched: Some(value.lease_matched),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn lease_matched_from_wire(
    value: pb::LeaseMatched,
) -> Result<contract::LeaseMatched, Error> {
    Ok(contract::LeaseMatched {
        lease_matched: required(value.lease_matched)?,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn remote_deleted_to_wire(
    value: &contract::RemoteDeleted,
) -> Result<pb::RemoteDeleted, Error> {
    Ok(pb::RemoteDeleted {
        deleted: Some(value.deleted),
        lease_matched: Some(value.lease_matched),
        oid: value.oid.clone(),
        destination_tag: value.destination_tag.clone(),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn remote_deleted_from_wire(
    value: pb::RemoteDeleted,
) -> Result<contract::RemoteDeleted, Error> {
    Ok(contract::RemoteDeleted {
        deleted: required(value.deleted)?,
        lease_matched: required(value.lease_matched)?,
        oid: value.oid,
        destination_tag: value.destination_tag,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn tag_created_to_wire(value: &contract::TagCreated) -> Result<pb::TagCreated, Error> {
    Ok(pb::TagCreated {
        oid: Some(value.oid.clone()),
        target_oid: Some(value.target_oid.clone()),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn tag_created_from_wire(value: pb::TagCreated) -> Result<contract::TagCreated, Error> {
    Ok(contract::TagCreated {
        oid: required(value.oid)?,
        target_oid: required(value.target_oid)?,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn reset_to_wire(value: &contract::Reset) -> Result<pb::Reset, Error> {
    Ok(pb::Reset {
        previous_oid: Some(value.previous_oid.clone()),
        oid: Some(value.oid.clone()),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn reset_from_wire(value: pb::Reset) -> Result<contract::Reset, Error> {
    Ok(contract::Reset {
        previous_oid: required(value.previous_oid)?,
        oid: required(value.oid)?,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn discarded_to_wire(value: &contract::Discarded) -> Result<pb::Discarded, Error> {
    Ok(pb::Discarded {
        index_changed: Some(value.index_changed),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn discarded_from_wire(value: pb::Discarded) -> Result<contract::Discarded, Error> {
    Ok(contract::Discarded {
        index_changed: required(value.index_changed)?,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn stash_saved_to_wire(value: &contract::StashSaved) -> Result<pb::StashSaved, Error> {
    Ok(pb::StashSaved {
        oid: Some(value.oid.clone()),
        saved: Some(value.saved),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn stash_saved_from_wire(value: pb::StashSaved) -> Result<contract::StashSaved, Error> {
    Ok(contract::StashSaved {
        oid: required(value.oid)?,
        saved: required(value.saved)?,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn stash_applied_to_wire(
    value: &contract::StashApplied,
) -> Result<pb::StashApplied, Error> {
    Ok(pb::StashApplied {
        oid: Some(value.oid.clone()),
        applied: Some(value.applied),
        dropped: Some(value.dropped),
        needs_resolution: Some(value.needs_resolution),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn stash_applied_from_wire(
    value: pb::StashApplied,
) -> Result<contract::StashApplied, Error> {
    Ok(contract::StashApplied {
        oid: required(value.oid)?,
        applied: required(value.applied)?,
        dropped: required(value.dropped)?,
        needs_resolution: required(value.needs_resolution)?,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn stash_dropped_to_wire(
    value: &contract::StashDropped,
) -> Result<pb::StashDropped, Error> {
    Ok(pb::StashDropped {
        oid: Some(value.oid.clone()),
        dropped: Some(value.dropped),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn stash_dropped_from_wire(
    value: pb::StashDropped,
) -> Result<contract::StashDropped, Error> {
    Ok(contract::StashDropped {
        oid: required(value.oid)?,
        dropped: required(value.dropped)?,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn needs_resolution_to_wire(
    value: &contract::NeedsResolution,
) -> Result<pb::NeedsResolution, Error> {
    Ok(pb::NeedsResolution {
        needs_resolution: Some(value.needs_resolution),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn needs_resolution_from_wire(
    value: pb::NeedsResolution,
) -> Result<contract::NeedsResolution, Error> {
    Ok(contract::NeedsResolution {
        needs_resolution: required(value.needs_resolution)?,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn integration_completed_to_wire(
    value: &contract::IntegrationCompleted,
) -> Result<pb::IntegrationCompleted, Error> {
    Ok(pb::IntegrationCompleted {
        needs_resolution: Some(value.needs_resolution),
        aborted: Some(value.aborted),
        merge_completed: Some(value.merge_completed),
        integration_completed: value.integration_completed.clone(),
        commit_oid: value.commit_oid.clone(),
        oid: value.oid.clone(),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn integration_completed_from_wire(
    value: pb::IntegrationCompleted,
) -> Result<contract::IntegrationCompleted, Error> {
    Ok(contract::IntegrationCompleted {
        needs_resolution: required(value.needs_resolution)?,
        aborted: required(value.aborted)?,
        merge_completed: required(value.merge_completed)?,
        integration_completed: value.integration_completed,
        commit_oid: value.commit_oid,
        oid: value.oid,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn worktree_added_to_wire(
    value: &contract::WorktreeAdded,
) -> Result<pb::WorktreeAdded, Error> {
    Ok(pb::WorktreeAdded {
        name: Some(value.name.clone()),
        path: Some(path_to_wire(&value.path)?),
        open_required: Some(value.open_required),
        branch_created: Some(value.branch_created),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn worktree_added_from_wire(
    value: pb::WorktreeAdded,
) -> Result<contract::WorktreeAdded, Error> {
    Ok(contract::WorktreeAdded {
        name: required(value.name)?,
        path: path_from_wire(required(value.path)?)?,
        open_required: required(value.open_required)?,
        branch_created: required(value.branch_created)?,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn worktree_repaired_to_wire(
    value: &contract::WorktreeRepaired,
) -> Result<pb::WorktreeRepaired, Error> {
    Ok(pb::WorktreeRepaired {
        changed: Some(value.changed),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn worktree_repaired_from_wire(
    value: pb::WorktreeRepaired,
) -> Result<contract::WorktreeRepaired, Error> {
    Ok(contract::WorktreeRepaired {
        changed: required(value.changed)?,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn worktree_registration_changed_to_wire(
    value: &contract::WorktreeRegistrationChanged,
) -> Result<pb::WorktreeRegistrationChanged, Error> {
    Ok(pb::WorktreeRegistrationChanged {
        registration_only: Some(value.registration_only),
        refresh_required: Some(value.refresh_required),
    })
}
pub(super) fn worktree_registration_changed_from_wire(
    value: pb::WorktreeRegistrationChanged,
) -> Result<contract::WorktreeRegistrationChanged, Error> {
    Ok(contract::WorktreeRegistrationChanged {
        registration_only: required(value.registration_only)?,
        refresh_required: required(value.refresh_required)?,
    })
}
pub(super) fn initialized_to_wire(value: &contract::Initialized) -> Result<pb::Initialized, Error> {
    Ok(pb::Initialized {
        initialized: Some(value.initialized),
        path: Some(path_to_wire(&value.path)?),
        initial_branch: Some(value.initial_branch.clone()),
        bare: Some(value.bare),
        open_required: Some(value.open_required),
    })
}
pub(super) fn initialized_from_wire(
    value: pb::Initialized,
) -> Result<contract::Initialized, Error> {
    Ok(contract::Initialized {
        initialized: required(value.initialized)?,
        path: path_from_wire(required(value.path)?)?,
        initial_branch: required(value.initial_branch)?,
        bare: required(value.bare)?,
        open_required: required(value.open_required)?,
    })
}
pub(super) fn cloned_to_wire(value: &contract::Cloned) -> Result<pb::Cloned, Error> {
    Ok(pb::Cloned {
        cloned: Some(value.cloned),
        path: Some(path_to_wire(&value.path)?),
        bare: Some(value.bare),
        oid: value.oid.clone(),
        open_required: Some(value.open_required),
    })
}
pub(super) fn cloned_from_wire(value: pb::Cloned) -> Result<contract::Cloned, Error> {
    Ok(contract::Cloned {
        cloned: required(value.cloned)?,
        path: path_from_wire(required(value.path)?)?,
        bare: required(value.bare)?,
        oid: value.oid,
        open_required: required(value.open_required)?,
    })
}
pub(super) fn configure_to_wire(value: &contract::Configure) -> Result<pb::Configure, Error> {
    Ok(pb::Configure {
        id: Some(value.id.clone()),
        client_id: Some(value.client_id.clone()),
        client_version: Some(value.client_version.clone()),
        command_logs: Some(value.command_logs),
    })
}
pub(super) fn configure_from_wire(value: pb::Configure) -> Result<contract::Configure, Error> {
    Ok(contract::Configure {
        id: required(value.id)?,
        client_id: required(value.client_id)?,
        client_version: required(value.client_version)?,
        command_logs: required(value.command_logs)?,
    })
}
pub(super) fn limits_to_wire(value: &contract::Limits) -> Result<pb::Limits, Error> {
    Ok(pb::Limits {
        max_frame_bytes: Some(value.max_frame_bytes),
        max_chunk_bytes: Some(value.max_chunk_bytes),
        max_inflight: Some(value.max_inflight),
        max_page_items: Some(value.max_page_items),
        stream_window: Some(value.stream_window),
        command_logs: Some(value.command_logs),
    })
}
pub(super) fn limits_from_wire(value: pb::Limits) -> Result<contract::Limits, Error> {
    Ok(contract::Limits {
        max_frame_bytes: required(value.max_frame_bytes)?,
        max_chunk_bytes: required(value.max_chunk_bytes)?,
        max_inflight: required(value.max_inflight)?,
        max_page_items: required(value.max_page_items)?,
        stream_window: required(value.stream_window)?,
        command_logs: required(value.command_logs)?,
    })
}
pub(super) fn capabilities_to_wire(
    value: &contract::Capabilities,
) -> Result<pb::Capabilities, Error> {
    Ok(pb::Capabilities {
        methods: value.methods.clone(),
        object_formats: value.object_formats.clone(),
        features: value.features.clone(),
        actions: value.actions.clone(),
    })
}
pub(super) fn capabilities_from_wire(
    value: pb::Capabilities,
) -> Result<contract::Capabilities, Error> {
    Ok(contract::Capabilities {
        methods: value.methods,
        object_formats: value.object_formats,
        features: value.features,
        actions: value.actions,
    })
}
pub(super) fn ready_to_wire(value: &contract::Ready) -> Result<pb::Ready, Error> {
    Ok(pb::Ready {
        id: Some(value.id.clone()),
        limits: Some(limits_to_wire(&value.limits)?),
        capabilities: Some(capabilities_to_wire(&value.capabilities)?),
    })
}
pub(super) fn ready_from_wire(value: pb::Ready) -> Result<contract::Ready, Error> {
    Ok(contract::Ready {
        id: required(value.id)?,
        limits: limits_from_wire(required(value.limits)?)?,
        capabilities: capabilities_from_wire(required(value.capabilities)?)?,
    })
}
pub(super) fn call_to_wire(value: &contract::Call) -> Result<pb::Call, Error> {
    Ok(pb::Call {
        id: Some(value.id.clone()),
        request: Some(request_to_wire(value.request.as_ref())?),
    })
}
pub(super) fn call_from_wire(value: pb::Call) -> Result<contract::Call, Error> {
    Ok(contract::Call {
        id: required(value.id)?,
        request: Box::new(request_from_wire(required(value.request)?)?),
    })
}
pub(super) fn begin_to_wire(value: &contract::Begin) -> Result<pb::Begin, Error> {
    Ok(pb::Begin {
        id: Some(value.id.clone()),
        stream_id: Some(value.stream_id.clone()),
        snapshot: Some(value.snapshot.clone()),
    })
}
pub(super) fn begin_from_wire(value: pb::Begin) -> Result<contract::Begin, Error> {
    Ok(contract::Begin {
        id: required(value.id)?,
        stream_id: required(value.stream_id)?,
        snapshot: required(value.snapshot)?,
    })
}
pub(super) fn chunk_to_wire(value: &contract::Chunk) -> Result<pb::Chunk, Error> {
    Ok(pb::Chunk {
        id: Some(value.id.clone()),
        stream_id: Some(value.stream_id.clone()),
        seq: Some(value.seq),
        data: Some(value.data.0.clone().into()),
    })
}
pub(super) fn chunk_from_wire(value: pb::Chunk) -> Result<contract::Chunk, Error> {
    Ok(contract::Chunk {
        id: required(value.id)?,
        stream_id: required(value.stream_id)?,
        seq: required(value.seq)?,
        data: contract::Binary(required(value.data)?.to_vec()),
    })
}
pub(super) fn ack_to_wire(value: &contract::Ack) -> Result<pb::Ack, Error> {
    Ok(pb::Ack {
        stream_id: Some(value.stream_id.clone()),
        seq: Some(value.seq),
    })
}
pub(super) fn ack_from_wire(value: pb::Ack) -> Result<contract::Ack, Error> {
    Ok(contract::Ack {
        stream_id: required(value.stream_id)?,
        seq: required(value.seq)?,
    })
}
pub(super) fn log_to_wire(value: &contract::Log) -> Result<pb::Log, Error> {
    Ok(pb::Log {
        id: Some(value.id.clone()),
        entry: Some(command_log_to_wire(&value.entry)?),
    })
}
pub(super) fn log_from_wire(value: pb::Log) -> Result<contract::Log, Error> {
    Ok(contract::Log {
        id: required(value.id)?,
        entry: command_log_from_wire(required(value.entry)?)?,
    })
}
pub(super) fn failure_to_wire(value: &contract::Error) -> Result<pb::Failure, Error> {
    Ok(pb::Failure {
        code: Some(value.code.clone()),
        message: Some(value.message.clone()),
        retry: Some(value.retry.clone()),
    })
}
pub(super) fn failure_from_wire(value: pb::Failure) -> Result<contract::Error, Error> {
    Ok(contract::Error {
        code: required(value.code)?,
        message: required(value.message)?,
        retry: required(value.retry)?,
    })
}
pub(super) fn status_entry_page_to_wire(
    value: &contract::Page<contract::StatusEntry, contract::StatusSummary>,
) -> Result<pb::StatusEntryPage, Error> {
    Ok(pb::StatusEntryPage {
        snapshot: Some(value.snapshot.clone()),
        entries: value
            .entries
            .iter()
            .map(status_entry_to_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor.clone(),
        metadata: Some(status_summary_to_wire(&value.metadata)?),
    })
}
pub(super) fn status_entry_page_from_wire(
    value: pb::StatusEntryPage,
) -> Result<contract::Page<contract::StatusEntry, contract::StatusSummary>, Error> {
    Ok(contract::Page {
        snapshot: required(value.snapshot)?,
        entries: value
            .entries
            .into_iter()
            .map(status_entry_from_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor,
        metadata: status_summary_from_wire(required(value.metadata)?)?,
    })
}
pub(super) fn branch_page_to_wire(
    value: &contract::Page<contract::Branch, contract::TotalEntries>,
) -> Result<pb::BranchPage, Error> {
    Ok(pb::BranchPage {
        snapshot: Some(value.snapshot.clone()),
        entries: value
            .entries
            .iter()
            .map(branch_to_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor.clone(),
        metadata: Some(total_entries_to_wire(&value.metadata)?),
    })
}
pub(super) fn branch_page_from_wire(
    value: pb::BranchPage,
) -> Result<contract::Page<contract::Branch, contract::TotalEntries>, Error> {
    Ok(contract::Page {
        snapshot: required(value.snapshot)?,
        entries: value
            .entries
            .into_iter()
            .map(branch_from_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor,
        metadata: total_entries_from_wire(required(value.metadata)?)?,
    })
}
pub(super) fn worktree_page_to_wire(
    value: &contract::Page<contract::Worktree, contract::WorktreeMetadata>,
) -> Result<pb::WorktreePage, Error> {
    Ok(pb::WorktreePage {
        snapshot: Some(value.snapshot.clone()),
        entries: value
            .entries
            .iter()
            .map(worktree_to_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor.clone(),
        metadata: Some(worktree_metadata_to_wire(&value.metadata)?),
    })
}
pub(super) fn worktree_page_from_wire(
    value: pb::WorktreePage,
) -> Result<contract::Page<contract::Worktree, contract::WorktreeMetadata>, Error> {
    Ok(contract::Page {
        snapshot: required(value.snapshot)?,
        entries: value
            .entries
            .into_iter()
            .map(worktree_from_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor,
        metadata: worktree_metadata_from_wire(required(value.metadata)?)?,
    })
}
pub(super) fn remote_name_page_to_wire(
    value: &contract::Page<contract::RemoteName, contract::TotalEntries>,
) -> Result<pb::RemoteNamePage, Error> {
    Ok(pb::RemoteNamePage {
        snapshot: Some(value.snapshot.clone()),
        entries: value
            .entries
            .iter()
            .map(remote_name_to_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor.clone(),
        metadata: Some(total_entries_to_wire(&value.metadata)?),
    })
}
pub(super) fn remote_name_page_from_wire(
    value: pb::RemoteNamePage,
) -> Result<contract::Page<contract::RemoteName, contract::TotalEntries>, Error> {
    Ok(contract::Page {
        snapshot: required(value.snapshot)?,
        entries: value
            .entries
            .into_iter()
            .map(remote_name_from_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor,
        metadata: total_entries_from_wire(required(value.metadata)?)?,
    })
}
pub(super) fn remote_reference_page_to_wire(
    value: &contract::Page<contract::RemoteReference, contract::RemoteReferenceMetadata>,
) -> Result<pb::RemoteReferencePage, Error> {
    Ok(pb::RemoteReferencePage {
        snapshot: Some(value.snapshot.clone()),
        entries: value
            .entries
            .iter()
            .map(remote_reference_to_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor.clone(),
        metadata: Some(remote_reference_metadata_to_wire(&value.metadata)?),
    })
}
pub(super) fn remote_reference_page_from_wire(
    value: pb::RemoteReferencePage,
) -> Result<contract::Page<contract::RemoteReference, contract::RemoteReferenceMetadata>, Error> {
    Ok(contract::Page {
        snapshot: required(value.snapshot)?,
        entries: value
            .entries
            .into_iter()
            .map(remote_reference_from_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor,
        metadata: remote_reference_metadata_from_wire(required(value.metadata)?)?,
    })
}
pub(super) fn blob_fragment_page_to_wire(
    value: &contract::Page<contract::BlobFragment, contract::BlobMetadata>,
) -> Result<pb::BlobFragmentPage, Error> {
    Ok(pb::BlobFragmentPage {
        snapshot: Some(value.snapshot.clone()),
        entries: value
            .entries
            .iter()
            .map(blob_fragment_to_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor.clone(),
        metadata: Some(blob_metadata_to_wire(&value.metadata)?),
    })
}
pub(super) fn blob_fragment_page_from_wire(
    value: pb::BlobFragmentPage,
) -> Result<contract::Page<contract::BlobFragment, contract::BlobMetadata>, Error> {
    Ok(contract::Page {
        snapshot: required(value.snapshot)?,
        entries: value
            .entries
            .into_iter()
            .map(blob_fragment_from_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor,
        metadata: blob_metadata_from_wire(required(value.metadata)?)?,
    })
}
pub(super) fn stash_page_to_wire(
    value: &contract::Page<contract::Stash, contract::StashMetadata>,
) -> Result<pb::StashPage, Error> {
    Ok(pb::StashPage {
        snapshot: Some(value.snapshot.clone()),
        entries: value
            .entries
            .iter()
            .map(stash_to_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor.clone(),
        metadata: Some(stash_metadata_to_wire(&value.metadata)?),
    })
}
pub(super) fn stash_page_from_wire(
    value: pb::StashPage,
) -> Result<contract::Page<contract::Stash, contract::StashMetadata>, Error> {
    Ok(contract::Page {
        snapshot: required(value.snapshot)?,
        entries: value
            .entries
            .into_iter()
            .map(stash_from_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor,
        metadata: stash_metadata_from_wire(required(value.metadata)?)?,
    })
}
pub(super) fn tag_page_to_wire(
    value: &contract::Page<contract::Tag, contract::TotalEntries>,
) -> Result<pb::TagPage, Error> {
    Ok(pb::TagPage {
        snapshot: Some(value.snapshot.clone()),
        entries: value
            .entries
            .iter()
            .map(tag_to_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor.clone(),
        metadata: Some(total_entries_to_wire(&value.metadata)?),
    })
}
pub(super) fn tag_page_from_wire(
    value: pb::TagPage,
) -> Result<contract::Page<contract::Tag, contract::TotalEntries>, Error> {
    Ok(contract::Page {
        snapshot: required(value.snapshot)?,
        entries: value
            .entries
            .into_iter()
            .map(tag_from_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor,
        metadata: total_entries_from_wire(required(value.metadata)?)?,
    })
}
pub(super) fn commit_page_to_wire(
    value: &contract::Page<contract::Commit, contract::HistoryMetadata>,
) -> Result<pb::CommitPage, Error> {
    Ok(pb::CommitPage {
        snapshot: Some(value.snapshot.clone()),
        entries: value
            .entries
            .iter()
            .map(commit_to_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor.clone(),
        metadata: Some(history_metadata_to_wire(&value.metadata)?),
    })
}
pub(super) fn commit_page_from_wire(
    value: pb::CommitPage,
) -> Result<contract::Page<contract::Commit, contract::HistoryMetadata>, Error> {
    Ok(contract::Page {
        snapshot: required(value.snapshot)?,
        entries: value
            .entries
            .into_iter()
            .map(commit_from_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor,
        metadata: history_metadata_from_wire(required(value.metadata)?)?,
    })
}
pub(super) fn delta_page_to_wire(
    value: &contract::Page<contract::Delta, contract::DiffMetadata>,
) -> Result<pb::DeltaPage, Error> {
    Ok(pb::DeltaPage {
        snapshot: Some(value.snapshot.clone()),
        entries: value
            .entries
            .iter()
            .map(delta_to_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor.clone(),
        metadata: Some(diff_metadata_to_wire(&value.metadata)?),
    })
}
pub(super) fn delta_page_from_wire(
    value: pb::DeltaPage,
) -> Result<contract::Page<contract::Delta, contract::DiffMetadata>, Error> {
    Ok(contract::Page {
        snapshot: required(value.snapshot)?,
        entries: value
            .entries
            .into_iter()
            .map(delta_from_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor,
        metadata: diff_metadata_from_wire(required(value.metadata)?)?,
    })
}
pub(super) fn file_fragment_page_to_wire(
    value: &contract::Page<contract::FileFragment, contract::DiffMetadata>,
) -> Result<pb::FileFragmentPage, Error> {
    Ok(pb::FileFragmentPage {
        snapshot: Some(value.snapshot.clone()),
        entries: value
            .entries
            .iter()
            .map(file_fragment_to_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor.clone(),
        metadata: Some(diff_metadata_to_wire(&value.metadata)?),
    })
}
pub(super) fn file_fragment_page_from_wire(
    value: pb::FileFragmentPage,
) -> Result<contract::Page<contract::FileFragment, contract::DiffMetadata>, Error> {
    Ok(contract::Page {
        snapshot: required(value.snapshot)?,
        entries: value
            .entries
            .into_iter()
            .map(file_fragment_from_wire)
            .collect::<Result<_, _>>()?,
        next_cursor: value.next_cursor,
        metadata: diff_metadata_from_wire(required(value.metadata)?)?,
    })
}
pub(crate) fn request_to_wire(value: &contract::Request) -> Result<pb::Request, Error> {
    use pb::request::Value;
    Ok(pb::Request {
        value: Some(match value {
            contract::Request::Clone {
                operation_id,
                url,
                path,
                branch,
                bare,
            } => Value::Clone(pb::RequestClone {
                operation_id: Some(operation_id.clone()),
                url: Some(url.clone()),
                path: Some(path_to_wire(path)?),
                branch: branch.clone(),
                bare: Some(*bare),
            }),
            contract::Request::Init {
                operation_id,
                path,
                initial_branch,
            } => Value::Init(pb::RequestInit {
                operation_id: Some(operation_id.clone()),
                path: Some(path_to_wire(path)?),
                initial_branch: Some(initial_branch.clone()),
            }),
            contract::Request::Tag { repo_id, oid } => Value::Tag(pb::RequestTag {
                repo_id: Some(repo_id.clone()),
                oid: Some(oid.clone()),
            }),
            contract::Request::Tags {
                repo_id,
                page_size,
                cursor,
                message_bytes,
            } => Value::Tags(pb::RequestTags {
                repo_id: Some(repo_id.clone()),
                page_size: Some(u64::try_from(*page_size).map_err(|_| Invalid("integer"))?),
                cursor: cursor.clone(),
                message_bytes: message_bytes
                    .as_ref()
                    .map(|item| u64::try_from(*item).map_err(|_| Invalid("integer")))
                    .transpose()?,
            }),
            contract::Request::Stashes {
                repo_id,
                page_size,
                cursor,
            } => Value::Stashes(pb::RequestStashes {
                repo_id: Some(repo_id.clone()),
                page_size: Some(u64::try_from(*page_size).map_err(|_| Invalid("integer"))?),
                cursor: cursor.clone(),
            }),
            contract::Request::BlobPage {
                repo_id,
                oid,
                max_bytes,
                cursor,
            } => Value::BlobPage(pb::RequestBlobPage {
                repo_id: Some(repo_id.clone()),
                oid: Some(oid.clone()),
                max_bytes: max_bytes
                    .as_ref()
                    .map(|item| u64::try_from(*item).map_err(|_| Invalid("integer")))
                    .transpose()?,
                cursor: cursor.clone(),
            }),
            contract::Request::Blob { repo_id, oid } => Value::Blob(pb::RequestBlob {
                repo_id: Some(repo_id.clone()),
                oid: Some(oid.clone()),
            }),
            contract::Request::RemoteRefs {
                repo_id,
                remote,
                expected_token,
                for_push,
                filter,
                page_size,
                cursor,
            } => Value::RemoteRefs(pb::RequestRemoteRefs {
                repo_id: Some(repo_id.clone()),
                remote: Some(remote.clone()),
                expected_token: Some(expected_token.clone()),
                for_push: Some(*for_push),
                filter: Some(filter.clone()),
                page_size: Some(u64::try_from(*page_size).map_err(|_| Invalid("integer"))?),
                cursor: cursor.clone(),
            }),
            contract::Request::RemoteNames {
                repo_id,
                filter,
                page_size,
                cursor,
            } => Value::RemoteNames(pb::RequestRemoteNames {
                repo_id: Some(repo_id.clone()),
                filter: Some(filter.clone()),
                page_size: Some(u64::try_from(*page_size).map_err(|_| Invalid("integer"))?),
                cursor: cursor.clone(),
            }),
            contract::Request::Remote { repo_id, name } => Value::Remote(pb::RequestRemote {
                repo_id: Some(repo_id.clone()),
                name: Some(name.clone()),
            }),
            contract::Request::Remotes { repo_id } => Value::Remotes(pb::RequestRemotes {
                repo_id: Some(repo_id.clone()),
            }),
            contract::Request::Start {
                operation_id,
                repo_id,
                expected_snapshot,
                action,
            } => Value::Start(pb::RequestStart {
                operation_id: Some(operation_id.clone()),
                repo_id: Some(repo_id.clone()),
                expected_snapshot: Some(expected_snapshot.clone()),
                action: Some(action_to_wire(action)?),
            }),
            contract::Request::Get { operation_id } => Value::Get(pb::RequestGet {
                operation_id: Some(operation_id.clone()),
            }),
            contract::Request::Review { operation_id } => Value::Review(pb::RequestReview {
                operation_id: Some(operation_id.clone()),
            }),
            contract::Request::Open { path } => Value::Open(pb::RequestOpen {
                path: Some(path_to_wire(path)?),
            }),
            contract::Request::Close { repo_id } => Value::Close(pb::RequestClose {
                repo_id: Some(repo_id.clone()),
            }),
            contract::Request::StatusSummary { repo_id, path } => {
                Value::StatusSummary(pb::RequestStatusSummary {
                    repo_id: repo_id.clone(),
                    path: path.as_ref().map(path_to_wire).transpose()?,
                })
            }
            contract::Request::Status {
                repo_id,
                filter,
                page_size,
                cursor,
            } => Value::Status(pb::RequestStatus {
                repo_id: Some(repo_id.clone()),
                filter: filter.as_ref().map(status_filter_to_wire).transpose()?,
                page_size: Some(u64::try_from(*page_size).map_err(|_| Invalid("integer"))?),
                cursor: cursor.clone(),
            }),
            contract::Request::Worktrees {
                repo_id,
                page_size,
                cursor,
                filter,
                branch,
                name,
                at_snapshot,
            } => Value::Worktrees(pb::RequestWorktrees {
                repo_id: Some(repo_id.clone()),
                page_size: Some(u64::try_from(*page_size).map_err(|_| Invalid("integer"))?),
                cursor: cursor.clone(),
                filter: Some(filter.clone()),
                branch: branch.clone(),
                name: name.clone(),
                at_snapshot: at_snapshot.clone(),
            }),
            contract::Request::Branches {
                repo_id,
                page_size,
                cursor,
                filter,
                branch_kind,
            } => Value::Branches(pb::RequestBranches {
                repo_id: Some(repo_id.clone()),
                page_size: Some(u64::try_from(*page_size).map_err(|_| Invalid("integer"))?),
                cursor: cursor.clone(),
                filter: Some(filter.clone()),
                branch_kind: branch_kind.clone(),
            }),
            contract::Request::History {
                repo_id,
                page_size,
                cursor,
                revision,
                message_bytes,
            } => Value::History(pb::RequestHistory {
                repo_id: Some(repo_id.clone()),
                page_size: Some(u64::try_from(*page_size).map_err(|_| Invalid("integer"))?),
                cursor: cursor.clone(),
                revision: Some(revision.clone()),
                message_bytes: message_bytes
                    .as_ref()
                    .map(|item| u64::try_from(*item).map_err(|_| Invalid("integer")))
                    .transpose()?,
            }),
            contract::Request::Commit {
                repo_id,
                commit_oid,
            } => Value::Commit(pb::RequestCommit {
                repo_id: Some(repo_id.clone()),
                commit_oid: Some(commit_oid.clone()),
            }),
            contract::Request::CommitFiles {
                repo_id,
                commit_oid,
                parent_index,
                page_size,
                cursor,
            } => Value::CommitFiles(pb::RequestCommitFiles {
                repo_id: Some(repo_id.clone()),
                commit_oid: Some(commit_oid.clone()),
                parent_index: Some(u64::try_from(*parent_index).map_err(|_| Invalid("integer"))?),
                page_size: Some(u64::try_from(*page_size).map_err(|_| Invalid("integer"))?),
                cursor: cursor.clone(),
            }),
            contract::Request::CommitDiff {
                path,
                repo_id,
                commit_oid,
                parent_index,
                context_lines,
            } => Value::CommitDiff(pb::RequestCommitDiff {
                path: path.as_ref().map(path_to_wire).transpose()?,
                repo_id: Some(repo_id.clone()),
                commit_oid: Some(commit_oid.clone()),
                parent_index: Some(u64::try_from(*parent_index).map_err(|_| Invalid("integer"))?),
                context_lines: Some(*context_lines),
            }),
            contract::Request::CommitDiffPage {
                repo_id,
                commit_oid,
                path,
                parent_index,
                context_lines,
                page_size,
                max_bytes,
                cursor,
                line_encoding,
            } => Value::CommitDiffPage(pb::RequestCommitDiffPage {
                repo_id: Some(repo_id.clone()),
                commit_oid: Some(commit_oid.clone()),
                path: Some(path_to_wire(path)?),
                parent_index: Some(u64::try_from(*parent_index).map_err(|_| Invalid("integer"))?),
                context_lines: Some(*context_lines),
                page_size: Some(u64::try_from(*page_size).map_err(|_| Invalid("integer"))?),
                max_bytes: max_bytes
                    .as_ref()
                    .map(|item| u64::try_from(*item).map_err(|_| Invalid("integer")))
                    .transpose()?,
                cursor: cursor.clone(),
                line_encoding: line_encoding
                    .as_ref()
                    .map(|item| Ok::<_, Error>(diff_line_encoding_to_wire(item)))
                    .transpose()?,
            }),
            contract::Request::DiffPage {
                repo_id,
                snapshot,
                entry_id,
                side,
                context_lines,
                page_size,
                max_bytes,
                cursor,
                line_encoding,
            } => Value::DiffPage(pb::RequestDiffPage {
                repo_id: Some(repo_id.clone()),
                snapshot: Some(snapshot.clone()),
                entry_id: Some(entry_id.clone()),
                side: Some(side_to_wire(side)),
                context_lines: Some(*context_lines),
                page_size: Some(u64::try_from(*page_size).map_err(|_| Invalid("integer"))?),
                max_bytes: max_bytes
                    .as_ref()
                    .map(|item| u64::try_from(*item).map_err(|_| Invalid("integer")))
                    .transpose()?,
                cursor: cursor.clone(),
                line_encoding: line_encoding
                    .as_ref()
                    .map(|item| Ok::<_, Error>(diff_line_encoding_to_wire(item)))
                    .transpose()?,
            }),
            contract::Request::Diff {
                repo_id,
                snapshot,
                entry_id,
                side,
                context_lines,
            } => Value::Diff(pb::RequestDiff {
                repo_id: Some(repo_id.clone()),
                snapshot: Some(snapshot.clone()),
                entry_id: Some(entry_id.clone()),
                side: Some(side_to_wire(side)),
                context_lines: Some(*context_lines),
            }),
        }),
    })
}
pub(crate) fn request_from_wire(value: pb::Request) -> Result<contract::Request, Error> {
    use pb::request::Value;
    Ok(match required(value.value)? {
        Value::Clone(item) => contract::Request::Clone {
            operation_id: required(item.operation_id)?,
            url: required(item.url)?,
            path: path_from_wire(required(item.path)?)?,
            branch: item.branch,
            bare: required(item.bare)?,
        },
        Value::Init(item) => contract::Request::Init {
            operation_id: required(item.operation_id)?,
            path: path_from_wire(required(item.path)?)?,
            initial_branch: required(item.initial_branch)?,
        },
        Value::Tag(item) => contract::Request::Tag {
            repo_id: required(item.repo_id)?,
            oid: required(item.oid)?,
        },
        Value::Tags(item) => contract::Request::Tags {
            repo_id: required(item.repo_id)?,
            page_size: usize::try_from(required(item.page_size)?)
                .map_err(|_| Invalid("integer"))?,
            cursor: item.cursor,
            message_bytes: item
                .message_bytes
                .map(|item| usize::try_from(item).map_err(|_| Invalid("integer")))
                .transpose()?,
        },
        Value::Stashes(item) => contract::Request::Stashes {
            repo_id: required(item.repo_id)?,
            page_size: usize::try_from(required(item.page_size)?)
                .map_err(|_| Invalid("integer"))?,
            cursor: item.cursor,
        },
        Value::BlobPage(item) => contract::Request::BlobPage {
            repo_id: required(item.repo_id)?,
            oid: required(item.oid)?,
            max_bytes: item
                .max_bytes
                .map(|item| usize::try_from(item).map_err(|_| Invalid("integer")))
                .transpose()?,
            cursor: item.cursor,
        },
        Value::Blob(item) => contract::Request::Blob {
            repo_id: required(item.repo_id)?,
            oid: required(item.oid)?,
        },
        Value::RemoteRefs(item) => contract::Request::RemoteRefs {
            repo_id: required(item.repo_id)?,
            remote: required(item.remote)?,
            expected_token: required(item.expected_token)?,
            for_push: required(item.for_push)?,
            filter: required(item.filter)?,
            page_size: usize::try_from(required(item.page_size)?)
                .map_err(|_| Invalid("integer"))?,
            cursor: item.cursor,
        },
        Value::RemoteNames(item) => contract::Request::RemoteNames {
            repo_id: required(item.repo_id)?,
            filter: required(item.filter)?,
            page_size: usize::try_from(required(item.page_size)?)
                .map_err(|_| Invalid("integer"))?,
            cursor: item.cursor,
        },
        Value::Remote(item) => contract::Request::Remote {
            repo_id: required(item.repo_id)?,
            name: required(item.name)?,
        },
        Value::Remotes(item) => contract::Request::Remotes {
            repo_id: required(item.repo_id)?,
        },
        Value::Start(item) => contract::Request::Start {
            operation_id: required(item.operation_id)?,
            repo_id: required(item.repo_id)?,
            expected_snapshot: required(item.expected_snapshot)?,
            action: action_from_wire(required(item.action)?)?,
        },
        Value::Get(item) => contract::Request::Get {
            operation_id: required(item.operation_id)?,
        },
        Value::Review(item) => contract::Request::Review {
            operation_id: required(item.operation_id)?,
        },
        Value::Open(item) => contract::Request::Open {
            path: path_from_wire(required(item.path)?)?,
        },
        Value::Close(item) => contract::Request::Close {
            repo_id: required(item.repo_id)?,
        },
        Value::StatusSummary(item) => contract::Request::StatusSummary {
            repo_id: item.repo_id,
            path: item.path.map(path_from_wire).transpose()?,
        },
        Value::Status(item) => contract::Request::Status {
            repo_id: required(item.repo_id)?,
            filter: item.filter.map(status_filter_from_wire).transpose()?,
            page_size: usize::try_from(required(item.page_size)?)
                .map_err(|_| Invalid("integer"))?,
            cursor: item.cursor,
        },
        Value::Worktrees(item) => contract::Request::Worktrees {
            repo_id: required(item.repo_id)?,
            page_size: usize::try_from(required(item.page_size)?)
                .map_err(|_| Invalid("integer"))?,
            cursor: item.cursor,
            filter: required(item.filter)?,
            branch: item.branch,
            name: item.name,
            at_snapshot: item.at_snapshot,
        },
        Value::Branches(item) => contract::Request::Branches {
            repo_id: required(item.repo_id)?,
            page_size: usize::try_from(required(item.page_size)?)
                .map_err(|_| Invalid("integer"))?,
            cursor: item.cursor,
            filter: required(item.filter)?,
            branch_kind: item.branch_kind,
        },
        Value::History(item) => contract::Request::History {
            repo_id: required(item.repo_id)?,
            page_size: usize::try_from(required(item.page_size)?)
                .map_err(|_| Invalid("integer"))?,
            cursor: item.cursor,
            revision: required(item.revision)?,
            message_bytes: item
                .message_bytes
                .map(|item| usize::try_from(item).map_err(|_| Invalid("integer")))
                .transpose()?,
        },
        Value::Commit(item) => contract::Request::Commit {
            repo_id: required(item.repo_id)?,
            commit_oid: required(item.commit_oid)?,
        },
        Value::CommitFiles(item) => contract::Request::CommitFiles {
            repo_id: required(item.repo_id)?,
            commit_oid: required(item.commit_oid)?,
            parent_index: usize::try_from(required(item.parent_index)?)
                .map_err(|_| Invalid("integer"))?,
            page_size: usize::try_from(required(item.page_size)?)
                .map_err(|_| Invalid("integer"))?,
            cursor: item.cursor,
        },
        Value::CommitDiff(item) => contract::Request::CommitDiff {
            path: item.path.map(path_from_wire).transpose()?,
            repo_id: required(item.repo_id)?,
            commit_oid: required(item.commit_oid)?,
            parent_index: usize::try_from(required(item.parent_index)?)
                .map_err(|_| Invalid("integer"))?,
            context_lines: required(item.context_lines)?,
        },
        Value::CommitDiffPage(item) => contract::Request::CommitDiffPage {
            repo_id: required(item.repo_id)?,
            commit_oid: required(item.commit_oid)?,
            path: path_from_wire(required(item.path)?)?,
            parent_index: usize::try_from(required(item.parent_index)?)
                .map_err(|_| Invalid("integer"))?,
            context_lines: required(item.context_lines)?,
            page_size: usize::try_from(required(item.page_size)?)
                .map_err(|_| Invalid("integer"))?,
            max_bytes: item
                .max_bytes
                .map(|item| usize::try_from(item).map_err(|_| Invalid("integer")))
                .transpose()?,
            cursor: item.cursor,
            line_encoding: item
                .line_encoding
                .map(diff_line_encoding_from_wire)
                .transpose()?,
        },
        Value::DiffPage(item) => contract::Request::DiffPage {
            repo_id: required(item.repo_id)?,
            snapshot: required(item.snapshot)?,
            entry_id: required(item.entry_id)?,
            side: side_from_wire(required(item.side)?)?,
            context_lines: required(item.context_lines)?,
            page_size: usize::try_from(required(item.page_size)?)
                .map_err(|_| Invalid("integer"))?,
            max_bytes: item
                .max_bytes
                .map(|item| usize::try_from(item).map_err(|_| Invalid("integer")))
                .transpose()?,
            cursor: item.cursor,
            line_encoding: item
                .line_encoding
                .map(diff_line_encoding_from_wire)
                .transpose()?,
        },
        Value::Diff(item) => contract::Request::Diff {
            repo_id: required(item.repo_id)?,
            snapshot: required(item.snapshot)?,
            entry_id: required(item.entry_id)?,
            side: side_from_wire(required(item.side)?)?,
            context_lines: required(item.context_lines)?,
        },
    })
}
pub(super) fn action_to_wire(value: &contract::Action) -> Result<pb::Action, Error> {
    use pb::action::Value;
    Ok(pb::Action {
        value: Some(match value {
            contract::Action::ConflictResolve {
                entry_ids,
                side,
                expected_oid,
            } => Value::ConflictResolve(pb::ActionConflictResolve {
                entry_ids: entry_ids.clone(),
                side: Some(conflict_side_to_wire(side)),
                expected_oid: expected_oid.clone(),
            }),
            contract::Action::Discard {
                entry_ids,
                source,
                hunks,
            } => Value::Discard(pb::ActionDiscard {
                entry_ids: entry_ids.clone(),
                source: Some(discard_source_to_wire(source)),
                hunks: hunks.as_ref().map(hunk_selection_to_wire).transpose()?,
            }),
            contract::Action::Reset {
                target_oid,
                expected_oid,
                mode,
            } => Value::Reset(pb::ActionReset {
                target_oid: Some(target_oid.clone()),
                expected_oid: Some(expected_oid.clone()),
                mode: Some(reset_mode_to_wire(mode)),
            }),
            contract::Action::Rebase {
                upstream_oid,
                onto_oid,
                committer,
            } => Value::Rebase(pb::ActionRebase {
                upstream_oid: Some(upstream_oid.clone()),
                onto_oid: onto_oid.clone(),
                committer: committer.as_ref().map(author_to_wire).transpose()?,
            }),
            contract::Action::WorktreeRepair { name, path } => {
                Value::WorktreeRepair(pb::ActionWorktreeRepair {
                    name: Some(name.clone()),
                    path: Some(path_to_wire(path)?),
                })
            }
            contract::Action::WorktreeRemove { name } => {
                Value::WorktreeRemove(pb::ActionWorktreeRemove {
                    name: Some(name.clone()),
                })
            }
            contract::Action::WorktreePrune { name } => {
                Value::WorktreePrune(pb::ActionWorktreePrune {
                    name: Some(name.clone()),
                })
            }
            contract::Action::WorktreeAdd {
                name,
                path,
                branch,
                expected_oid,
                locked,
                new_branch,
            } => Value::WorktreeAdd(pb::ActionWorktreeAdd {
                name: Some(name.clone()),
                path: Some(path_to_wire(path)?),
                branch: Some(branch.clone()),
                expected_oid: Some(expected_oid.clone()),
                locked: Some(*locked),
                new_branch: Some(*new_branch),
            }),
            contract::Action::WorktreeLock { name, reason } => {
                Value::WorktreeLock(pb::ActionWorktreeLock {
                    name: Some(name.clone()),
                    reason: reason.clone(),
                })
            }
            contract::Action::WorktreeUnlock { name } => {
                Value::WorktreeUnlock(pb::ActionWorktreeUnlock {
                    name: Some(name.clone()),
                })
            }
            contract::Action::IntegrationSkip {} => {
                Value::IntegrationSkip(pb::ActionIntegrationSkip {})
            }
            contract::Action::BranchSetUpstream {
                name,
                expected_oid,
                expected_token,
                upstream,
            } => Value::BranchSetUpstream(pb::ActionBranchSetUpstream {
                name: Some(name.clone()),
                expected_oid: Some(expected_oid.clone()),
                expected_token: Some(expected_token.clone()),
                upstream: upstream.clone(),
            }),
            contract::Action::CherryPick {
                target_oid,
                mainline,
                author,
            } => Value::CherryPick(pb::ActionCherryPick {
                target_oid: Some(target_oid.clone()),
                mainline: Some(*mainline),
                author: author.as_ref().map(author_to_wire).transpose()?,
            }),
            contract::Action::Revert {
                target_oid,
                mainline,
                author,
            } => Value::Revert(pb::ActionRevert {
                target_oid: Some(target_oid.clone()),
                mainline: Some(*mainline),
                author: author.as_ref().map(author_to_wire).transpose()?,
            }),
            contract::Action::IntegrationContinue { message, author } => {
                Value::IntegrationContinue(pb::ActionIntegrationContinue {
                    message: message.clone(),
                    author: author.as_ref().map(author_to_wire).transpose()?,
                })
            }
            contract::Action::IntegrationAbort {} => {
                Value::IntegrationAbort(pb::ActionIntegrationAbort {})
            }
            contract::Action::TagDeleteRemote {
                remote,
                expected_token,
                name,
                expected_oid,
            } => Value::TagDeleteRemote(pb::ActionTagDeleteRemote {
                remote: Some(remote.clone()),
                expected_token: Some(expected_token.clone()),
                name: Some(name.clone()),
                expected_oid: Some(expected_oid.clone()),
            }),
            contract::Action::TagPush {
                remote,
                expected_token,
                name,
                expected_oid,
            } => Value::TagPush(pb::ActionTagPush {
                remote: Some(remote.clone()),
                expected_token: Some(expected_token.clone()),
                name: Some(name.clone()),
                expected_oid: Some(expected_oid.clone()),
            }),
            contract::Action::TagCreate {
                name,
                target_oid,
                annotation,
            } => Value::TagCreate(pb::ActionTagCreate {
                name: Some(name.clone()),
                target_oid: Some(target_oid.clone()),
                annotation: annotation
                    .as_ref()
                    .map(tag_annotation_to_wire)
                    .transpose()?,
            }),
            contract::Action::TagDelete { name, expected_oid } => {
                Value::TagDelete(pb::ActionTagDelete {
                    name: Some(name.clone()),
                    expected_oid: Some(expected_oid.clone()),
                })
            }
            contract::Action::StashSave {
                message,
                include_untracked,
                keep_index,
                author,
            } => Value::StashSave(pb::ActionStashSave {
                message: Some(message.clone()),
                include_untracked: Some(*include_untracked),
                keep_index: Some(*keep_index),
                author: author.as_ref().map(author_to_wire).transpose()?,
            }),
            contract::Action::StashApply {
                oid,
                index,
                expected_token,
                reinstate_index,
            } => Value::StashApply(pb::ActionStashApply {
                oid: Some(oid.clone()),
                index: index
                    .as_ref()
                    .map(|item| u64::try_from(*item).map_err(|_| Invalid("integer")))
                    .transpose()?,
                expected_token: Some(expected_token.clone()),
                reinstate_index: Some(*reinstate_index),
            }),
            contract::Action::StashPop {
                oid,
                index,
                expected_token,
                reinstate_index,
            } => Value::StashPop(pb::ActionStashPop {
                oid: Some(oid.clone()),
                index: index
                    .as_ref()
                    .map(|item| u64::try_from(*item).map_err(|_| Invalid("integer")))
                    .transpose()?,
                expected_token: Some(expected_token.clone()),
                reinstate_index: Some(*reinstate_index),
            }),
            contract::Action::StashDrop {
                oid,
                index,
                expected_token,
            } => Value::StashDrop(pb::ActionStashDrop {
                oid: Some(oid.clone()),
                index: index
                    .as_ref()
                    .map(|item| u64::try_from(*item).map_err(|_| Invalid("integer")))
                    .transpose()?,
                expected_token: Some(expected_token.clone()),
            }),
            contract::Action::MergeAbort {} => Value::MergeAbort(pb::ActionMergeAbort {}),
            contract::Action::Merge { target_oid } => Value::Merge(pb::ActionMerge {
                target_oid: Some(target_oid.clone()),
            }),
            contract::Action::PullFastForward {
                remote,
                expected_token,
                remote_branch,
            } => Value::PullFastForward(pb::ActionPullFastForward {
                remote: Some(remote.clone()),
                expected_token: Some(expected_token.clone()),
                remote_branch: Some(remote_branch.clone()),
            }),
            contract::Action::FastForward { target_oid } => {
                Value::FastForward(pb::ActionFastForward {
                    target_oid: Some(target_oid.clone()),
                })
            }
            contract::Action::RemoteAdd { name, url } => Value::RemoteAdd(pb::ActionRemoteAdd {
                name: Some(name.clone()),
                url: Some(url.clone()),
            }),
            contract::Action::RemoteRename {
                name,
                new_name,
                expected_token,
            } => Value::RemoteRename(pb::ActionRemoteRename {
                name: Some(name.clone()),
                new_name: Some(new_name.clone()),
                expected_token: Some(expected_token.clone()),
            }),
            contract::Action::RemoteSetUrl {
                name,
                url,
                expected_token,
            } => Value::RemoteSetUrl(pb::ActionRemoteSetUrl {
                name: Some(name.clone()),
                url: Some(url.clone()),
                expected_token: Some(expected_token.clone()),
            }),
            contract::Action::RemoteRemove {
                name,
                expected_token,
            } => Value::RemoteRemove(pb::ActionRemoteRemove {
                name: Some(name.clone()),
                expected_token: Some(expected_token.clone()),
            }),
            contract::Action::ConfigSet {
                expected_token,
                fetch_prune,
                user_name,
                user_email,
            } => Value::ConfigSet(pb::ActionConfigSet {
                expected_token: Some(expected_token.clone()),
                fetch_prune: *fetch_prune,
                user_name: user_name.clone(),
                user_email: user_email.clone(),
            }),
            contract::Action::Fetch {
                remote,
                expected_token,
                prune,
            } => Value::Fetch(pb::ActionFetch {
                remote: Some(remote.clone()),
                expected_token: Some(expected_token.clone()),
                prune: Some(*prune),
            }),
            contract::Action::BranchDeleteRemote {
                remote,
                expected_token,
                branch,
                expected_oid,
            } => Value::BranchDeleteRemote(pb::ActionBranchDeleteRemote {
                remote: Some(remote.clone()),
                expected_token: Some(expected_token.clone()),
                branch: Some(branch.clone()),
                expected_oid: Some(expected_oid.clone()),
            }),
            contract::Action::PushWithLease {
                remote,
                expected_token,
                branch,
                expected_oid,
                destination_branch,
                expected_remote_oid,
            } => Value::PushWithLease(pb::ActionPushWithLease {
                remote: Some(remote.clone()),
                expected_token: Some(expected_token.clone()),
                branch: Some(branch.clone()),
                expected_oid: Some(expected_oid.clone()),
                destination_branch: Some(destination_branch.clone()),
                expected_remote_oid: Some(expected_remote_oid.clone()),
            }),
            contract::Action::Push {
                remote,
                expected_token,
                branch,
                expected_oid,
                destination_branch,
            } => Value::Push(pb::ActionPush {
                remote: Some(remote.clone()),
                expected_token: Some(expected_token.clone()),
                branch: Some(branch.clone()),
                expected_oid: Some(expected_oid.clone()),
                destination_branch: Some(destination_branch.clone()),
            }),
            contract::Action::Checkout { target } => Value::Checkout(pb::ActionCheckout {
                target: Some(checkout_target_to_wire(target)?),
            }),
            contract::Action::BranchCreate { name, start_oid } => {
                Value::BranchCreate(pb::ActionBranchCreate {
                    name: Some(name.clone()),
                    start_oid: Some(start_oid.clone()),
                })
            }
            contract::Action::BranchRename {
                name,
                new_name,
                expected_oid,
            } => Value::BranchRename(pb::ActionBranchRename {
                name: Some(name.clone()),
                new_name: Some(new_name.clone()),
                expected_oid: Some(expected_oid.clone()),
            }),
            contract::Action::BranchDelete {
                name,
                expected_oid,
                force,
            } => Value::BranchDelete(pb::ActionBranchDelete {
                name: Some(name.clone()),
                expected_oid: Some(expected_oid.clone()),
                force: Some(*force),
            }),
            contract::Action::Amend {
                expected_oid,
                message,
                committer,
                author,
            } => Value::Amend(pb::ActionAmend {
                expected_oid: Some(expected_oid.clone()),
                message: Some(message.clone()),
                committer: committer.as_ref().map(author_to_wire).transpose()?,
                author: author.as_ref().map(author_to_wire).transpose()?,
            }),
            contract::Action::Commit { message, author } => Value::Commit(pb::ActionCommit {
                message: Some(message.clone()),
                author: author.as_ref().map(author_to_wire).transpose()?,
            }),
            contract::Action::Stage { entry_ids, hunks } => Value::Stage(pb::ActionStage {
                entry_ids: entry_ids.clone(),
                hunks: hunks.as_ref().map(hunk_selection_to_wire).transpose()?,
            }),
            contract::Action::Unstage { entry_ids, hunks } => Value::Unstage(pb::ActionUnstage {
                entry_ids: entry_ids.clone(),
                hunks: hunks.as_ref().map(hunk_selection_to_wire).transpose()?,
            }),
        }),
    })
}
pub(super) fn action_from_wire(value: pb::Action) -> Result<contract::Action, Error> {
    use pb::action::Value;
    Ok(match required(value.value)? {
        Value::ConflictResolve(item) => contract::Action::ConflictResolve {
            entry_ids: item.entry_ids,
            side: conflict_side_from_wire(required(item.side)?)?,
            expected_oid: item.expected_oid,
        },
        Value::Discard(item) => contract::Action::Discard {
            entry_ids: item.entry_ids,
            source: discard_source_from_wire(required(item.source)?)?,
            hunks: item.hunks.map(hunk_selection_from_wire).transpose()?,
        },
        Value::Reset(item) => contract::Action::Reset {
            target_oid: required(item.target_oid)?,
            expected_oid: required(item.expected_oid)?,
            mode: reset_mode_from_wire(required(item.mode)?)?,
        },
        Value::Rebase(item) => contract::Action::Rebase {
            upstream_oid: required(item.upstream_oid)?,
            onto_oid: item.onto_oid,
            committer: item.committer.map(author_from_wire).transpose()?,
        },
        Value::WorktreeRepair(item) => contract::Action::WorktreeRepair {
            name: required(item.name)?,
            path: path_from_wire(required(item.path)?)?,
        },
        Value::WorktreeRemove(item) => contract::Action::WorktreeRemove {
            name: required(item.name)?,
        },
        Value::WorktreePrune(item) => contract::Action::WorktreePrune {
            name: required(item.name)?,
        },
        Value::WorktreeAdd(item) => contract::Action::WorktreeAdd {
            name: required(item.name)?,
            path: path_from_wire(required(item.path)?)?,
            branch: required(item.branch)?,
            expected_oid: required(item.expected_oid)?,
            locked: required(item.locked)?,
            new_branch: required(item.new_branch)?,
        },
        Value::WorktreeLock(item) => contract::Action::WorktreeLock {
            name: required(item.name)?,
            reason: item.reason,
        },
        Value::WorktreeUnlock(item) => contract::Action::WorktreeUnlock {
            name: required(item.name)?,
        },
        Value::IntegrationSkip(_) => contract::Action::IntegrationSkip {},
        Value::BranchSetUpstream(item) => contract::Action::BranchSetUpstream {
            name: required(item.name)?,
            expected_oid: required(item.expected_oid)?,
            expected_token: required(item.expected_token)?,
            upstream: item.upstream,
        },
        Value::CherryPick(item) => contract::Action::CherryPick {
            target_oid: required(item.target_oid)?,
            mainline: required(item.mainline)?,
            author: item.author.map(author_from_wire).transpose()?,
        },
        Value::Revert(item) => contract::Action::Revert {
            target_oid: required(item.target_oid)?,
            mainline: required(item.mainline)?,
            author: item.author.map(author_from_wire).transpose()?,
        },
        Value::IntegrationContinue(item) => contract::Action::IntegrationContinue {
            message: item.message,
            author: item.author.map(author_from_wire).transpose()?,
        },
        Value::IntegrationAbort(_) => contract::Action::IntegrationAbort {},
        Value::TagDeleteRemote(item) => contract::Action::TagDeleteRemote {
            remote: required(item.remote)?,
            expected_token: required(item.expected_token)?,
            name: required(item.name)?,
            expected_oid: required(item.expected_oid)?,
        },
        Value::TagPush(item) => contract::Action::TagPush {
            remote: required(item.remote)?,
            expected_token: required(item.expected_token)?,
            name: required(item.name)?,
            expected_oid: required(item.expected_oid)?,
        },
        Value::TagCreate(item) => contract::Action::TagCreate {
            name: required(item.name)?,
            target_oid: required(item.target_oid)?,
            annotation: item.annotation.map(tag_annotation_from_wire).transpose()?,
        },
        Value::TagDelete(item) => contract::Action::TagDelete {
            name: required(item.name)?,
            expected_oid: required(item.expected_oid)?,
        },
        Value::StashSave(item) => contract::Action::StashSave {
            message: required(item.message)?,
            include_untracked: required(item.include_untracked)?,
            keep_index: required(item.keep_index)?,
            author: item.author.map(author_from_wire).transpose()?,
        },
        Value::StashApply(item) => contract::Action::StashApply {
            oid: required(item.oid)?,
            index: item
                .index
                .map(|item| usize::try_from(item).map_err(|_| Invalid("integer")))
                .transpose()?,
            expected_token: required(item.expected_token)?,
            reinstate_index: required(item.reinstate_index)?,
        },
        Value::StashPop(item) => contract::Action::StashPop {
            oid: required(item.oid)?,
            index: item
                .index
                .map(|item| usize::try_from(item).map_err(|_| Invalid("integer")))
                .transpose()?,
            expected_token: required(item.expected_token)?,
            reinstate_index: required(item.reinstate_index)?,
        },
        Value::StashDrop(item) => contract::Action::StashDrop {
            oid: required(item.oid)?,
            index: item
                .index
                .map(|item| usize::try_from(item).map_err(|_| Invalid("integer")))
                .transpose()?,
            expected_token: required(item.expected_token)?,
        },
        Value::MergeAbort(_) => contract::Action::MergeAbort {},
        Value::Merge(item) => contract::Action::Merge {
            target_oid: required(item.target_oid)?,
        },
        Value::PullFastForward(item) => contract::Action::PullFastForward {
            remote: required(item.remote)?,
            expected_token: required(item.expected_token)?,
            remote_branch: required(item.remote_branch)?,
        },
        Value::FastForward(item) => contract::Action::FastForward {
            target_oid: required(item.target_oid)?,
        },
        Value::RemoteAdd(item) => contract::Action::RemoteAdd {
            name: required(item.name)?,
            url: required(item.url)?,
        },
        Value::RemoteRename(item) => contract::Action::RemoteRename {
            name: required(item.name)?,
            new_name: required(item.new_name)?,
            expected_token: required(item.expected_token)?,
        },
        Value::RemoteSetUrl(item) => contract::Action::RemoteSetUrl {
            name: required(item.name)?,
            url: required(item.url)?,
            expected_token: required(item.expected_token)?,
        },
        Value::RemoteRemove(item) => contract::Action::RemoteRemove {
            name: required(item.name)?,
            expected_token: required(item.expected_token)?,
        },
        Value::ConfigSet(item) => contract::Action::ConfigSet {
            expected_token: required(item.expected_token)?,
            fetch_prune: item.fetch_prune,
            user_name: item.user_name,
            user_email: item.user_email,
        },
        Value::Fetch(item) => contract::Action::Fetch {
            remote: required(item.remote)?,
            expected_token: required(item.expected_token)?,
            prune: required(item.prune)?,
        },
        Value::BranchDeleteRemote(item) => contract::Action::BranchDeleteRemote {
            remote: required(item.remote)?,
            expected_token: required(item.expected_token)?,
            branch: required(item.branch)?,
            expected_oid: required(item.expected_oid)?,
        },
        Value::PushWithLease(item) => contract::Action::PushWithLease {
            remote: required(item.remote)?,
            expected_token: required(item.expected_token)?,
            branch: required(item.branch)?,
            expected_oid: required(item.expected_oid)?,
            destination_branch: required(item.destination_branch)?,
            expected_remote_oid: required(item.expected_remote_oid)?,
        },
        Value::Push(item) => contract::Action::Push {
            remote: required(item.remote)?,
            expected_token: required(item.expected_token)?,
            branch: required(item.branch)?,
            expected_oid: required(item.expected_oid)?,
            destination_branch: required(item.destination_branch)?,
        },
        Value::Checkout(item) => contract::Action::Checkout {
            target: checkout_target_from_wire(required(item.target)?)?,
        },
        Value::BranchCreate(item) => contract::Action::BranchCreate {
            name: required(item.name)?,
            start_oid: required(item.start_oid)?,
        },
        Value::BranchRename(item) => contract::Action::BranchRename {
            name: required(item.name)?,
            new_name: required(item.new_name)?,
            expected_oid: required(item.expected_oid)?,
        },
        Value::BranchDelete(item) => contract::Action::BranchDelete {
            name: required(item.name)?,
            expected_oid: required(item.expected_oid)?,
            force: required(item.force)?,
        },
        Value::Amend(item) => contract::Action::Amend {
            expected_oid: required(item.expected_oid)?,
            message: required(item.message)?,
            committer: item.committer.map(author_from_wire).transpose()?,
            author: item.author.map(author_from_wire).transpose()?,
        },
        Value::Commit(item) => contract::Action::Commit {
            message: required(item.message)?,
            author: item.author.map(author_from_wire).transpose()?,
        },
        Value::Stage(item) => contract::Action::Stage {
            entry_ids: item.entry_ids,
            hunks: item.hunks.map(hunk_selection_from_wire).transpose()?,
        },
        Value::Unstage(item) => contract::Action::Unstage {
            entry_ids: item.entry_ids,
            hunks: item.hunks.map(hunk_selection_from_wire).transpose()?,
        },
    })
}
pub(super) fn checkout_target_to_wire(
    value: &contract::CheckoutTarget,
) -> Result<pb::CheckoutTarget, Error> {
    use pb::checkout_target::Value;
    Ok(pb::CheckoutTarget {
        value: Some(match value {
            contract::CheckoutTarget::Branch { name, expected_oid } => {
                Value::Branch(pb::CheckoutTargetBranch {
                    name: Some(name.clone()),
                    expected_oid: Some(expected_oid.clone()),
                })
            }
            contract::CheckoutTarget::Detached { oid } => {
                Value::Detached(pb::CheckoutTargetDetached {
                    oid: Some(oid.clone()),
                })
            }
        }),
    })
}
pub(super) fn checkout_target_from_wire(
    value: pb::CheckoutTarget,
) -> Result<contract::CheckoutTarget, Error> {
    use pb::checkout_target::Value;
    Ok(match required(value.value)? {
        Value::Branch(item) => contract::CheckoutTarget::Branch {
            name: required(item.name)?,
            expected_oid: required(item.expected_oid)?,
        },
        Value::Detached(item) => contract::CheckoutTarget::Detached {
            oid: required(item.oid)?,
        },
    })
}
pub(crate) fn reply_to_wire(value: &contract::Reply) -> Result<pb::Reply, Error> {
    use pb::reply::Value;
    Ok(pb::Reply {
        value: Some(match value {
            contract::Reply::Open(item) => {
                Value::Open(Box::new(repository_to_wire(item.as_ref())?))
            }
            contract::Reply::Close(item) => Value::Close(Box::new(closed_to_wire(item.as_ref())?)),
            contract::Reply::StatusSummary(item) => {
                Value::StatusSummary(Box::new(status_summary_to_wire(item.as_ref())?))
            }
            contract::Reply::Status(item) => {
                Value::Status(Box::new(status_entry_page_to_wire(item.as_ref())?))
            }
            contract::Reply::Branches(item) => {
                Value::Branches(Box::new(branch_page_to_wire(item.as_ref())?))
            }
            contract::Reply::Worktrees(item) => {
                Value::Worktrees(Box::new(worktree_page_to_wire(item.as_ref())?))
            }
            contract::Reply::Remotes(item) => {
                Value::Remotes(Box::new(remotes_to_wire(item.as_ref())?))
            }
            contract::Reply::Remote(item) => {
                Value::Remote(Box::new(remote_to_wire(item.as_ref())?))
            }
            contract::Reply::RemoteNames(item) => {
                Value::RemoteNames(Box::new(remote_name_page_to_wire(item.as_ref())?))
            }
            contract::Reply::RemoteRefs(item) => {
                Value::RemoteRefs(Box::new(remote_reference_page_to_wire(item.as_ref())?))
            }
            contract::Reply::Blob(item) => Value::Blob(Box::new(blob_to_wire(item.as_ref())?)),
            contract::Reply::BlobPage(item) => {
                Value::BlobPage(Box::new(blob_fragment_page_to_wire(item.as_ref())?))
            }
            contract::Reply::Stashes(item) => {
                Value::Stashes(Box::new(stash_page_to_wire(item.as_ref())?))
            }
            contract::Reply::Tags(item) => Value::Tags(Box::new(tag_page_to_wire(item.as_ref())?)),
            contract::Reply::Tag(item) => Value::Tag(Box::new(tag_to_wire(item.as_ref())?)),
            contract::Reply::History(item) => {
                Value::History(Box::new(commit_page_to_wire(item.as_ref())?))
            }
            contract::Reply::Commit(item) => {
                Value::Commit(Box::new(commit_to_wire(item.as_ref())?))
            }
            contract::Reply::CommitFiles(item) => {
                Value::CommitFiles(Box::new(delta_page_to_wire(item.as_ref())?))
            }
            contract::Reply::DiffPage(item) => {
                Value::DiffPage(Box::new(file_fragment_page_to_wire(item.as_ref())?))
            }
            contract::Reply::CommitDiffPage(item) => {
                Value::CommitDiffPage(Box::new(file_fragment_page_to_wire(item.as_ref())?))
            }
            contract::Reply::Operation(item) => {
                Value::Operation(Box::new(operation_to_wire(item.as_ref())?))
            }
            contract::Reply::StreamComplete(item) => {
                Value::StreamComplete(Box::new(stream_complete_to_wire(item.as_ref())?))
            }
        }),
    })
}
pub(crate) fn reply_from_wire(value: pb::Reply) -> Result<contract::Reply, Error> {
    use pb::reply::Value;
    Ok(match required(value.value)? {
        Value::Open(item) => contract::Reply::Open(Box::new(repository_from_wire(*item)?)),
        Value::Close(item) => contract::Reply::Close(Box::new(closed_from_wire(*item)?)),
        Value::StatusSummary(item) => {
            contract::Reply::StatusSummary(Box::new(status_summary_from_wire(*item)?))
        }
        Value::Status(item) => {
            contract::Reply::Status(Box::new(status_entry_page_from_wire(*item)?))
        }
        Value::Branches(item) => contract::Reply::Branches(Box::new(branch_page_from_wire(*item)?)),
        Value::Worktrees(item) => {
            contract::Reply::Worktrees(Box::new(worktree_page_from_wire(*item)?))
        }
        Value::Remotes(item) => contract::Reply::Remotes(Box::new(remotes_from_wire(*item)?)),
        Value::Remote(item) => contract::Reply::Remote(Box::new(remote_from_wire(*item)?)),
        Value::RemoteNames(item) => {
            contract::Reply::RemoteNames(Box::new(remote_name_page_from_wire(*item)?))
        }
        Value::RemoteRefs(item) => {
            contract::Reply::RemoteRefs(Box::new(remote_reference_page_from_wire(*item)?))
        }
        Value::Blob(item) => contract::Reply::Blob(Box::new(blob_from_wire(*item)?)),
        Value::BlobPage(item) => {
            contract::Reply::BlobPage(Box::new(blob_fragment_page_from_wire(*item)?))
        }
        Value::Stashes(item) => contract::Reply::Stashes(Box::new(stash_page_from_wire(*item)?)),
        Value::Tags(item) => contract::Reply::Tags(Box::new(tag_page_from_wire(*item)?)),
        Value::Tag(item) => contract::Reply::Tag(Box::new(tag_from_wire(*item)?)),
        Value::History(item) => contract::Reply::History(Box::new(commit_page_from_wire(*item)?)),
        Value::Commit(item) => contract::Reply::Commit(Box::new(commit_from_wire(*item)?)),
        Value::CommitFiles(item) => {
            contract::Reply::CommitFiles(Box::new(delta_page_from_wire(*item)?))
        }
        Value::DiffPage(item) => {
            contract::Reply::DiffPage(Box::new(file_fragment_page_from_wire(*item)?))
        }
        Value::CommitDiffPage(item) => {
            contract::Reply::CommitDiffPage(Box::new(file_fragment_page_from_wire(*item)?))
        }
        Value::Operation(item) => contract::Reply::Operation(Box::new(operation_from_wire(*item)?)),
        Value::StreamComplete(item) => {
            contract::Reply::StreamComplete(Box::new(stream_complete_from_wire(*item)?))
        }
    })
}
pub(super) fn operation_result_to_wire(
    value: &contract::OperationResult,
) -> Result<pb::OperationResult, Error> {
    use pb::operation_result::Value;
    Ok(pb::OperationResult {
        value: Some(match value {
            contract::OperationResult::Refresh(item) => Value::Refresh(refresh_to_wire(item)?),
            contract::OperationResult::Committed(item) => {
                Value::Committed(committed_to_wire(item)?)
            }
            contract::OperationResult::Amended(item) => Value::Amended(amended_to_wire(item)?),
            contract::OperationResult::TrackingChanged(item) => {
                Value::TrackingChanged(tracking_changed_to_wire(item)?)
            }
            contract::OperationResult::CheckedOut(item) => {
                Value::CheckedOut(checked_out_to_wire(item)?)
            }
            contract::OperationResult::RemoteRenamed(item) => {
                Value::RemoteRenamed(remote_renamed_to_wire(item)?)
            }
            contract::OperationResult::FastForwarded(item) => {
                Value::FastForwarded(fast_forwarded_to_wire(item)?)
            }
            contract::OperationResult::LeaseMatched(item) => {
                Value::LeaseMatched(lease_matched_to_wire(item)?)
            }
            contract::OperationResult::RemoteDeleted(item) => {
                Value::RemoteDeleted(remote_deleted_to_wire(item)?)
            }
            contract::OperationResult::TagCreated(item) => {
                Value::TagCreated(tag_created_to_wire(item)?)
            }
            contract::OperationResult::Reset(item) => Value::Reset(reset_to_wire(item)?),
            contract::OperationResult::Discarded(item) => {
                Value::Discarded(discarded_to_wire(item)?)
            }
            contract::OperationResult::StashSaved(item) => {
                Value::StashSaved(stash_saved_to_wire(item)?)
            }
            contract::OperationResult::StashApplied(item) => {
                Value::StashApplied(stash_applied_to_wire(item)?)
            }
            contract::OperationResult::StashDropped(item) => {
                Value::StashDropped(stash_dropped_to_wire(item)?)
            }
            contract::OperationResult::NeedsResolution(item) => {
                Value::NeedsResolution(needs_resolution_to_wire(item)?)
            }
            contract::OperationResult::IntegrationCompleted(item) => {
                Value::IntegrationCompleted(integration_completed_to_wire(item)?)
            }
            contract::OperationResult::WorktreeAdded(item) => {
                Value::WorktreeAdded(worktree_added_to_wire(item)?)
            }
            contract::OperationResult::WorktreeRepaired(item) => {
                Value::WorktreeRepaired(worktree_repaired_to_wire(item)?)
            }
            contract::OperationResult::WorktreeRegistrationChanged(item) => {
                Value::WorktreeRegistrationChanged(worktree_registration_changed_to_wire(item)?)
            }
            contract::OperationResult::Initialized(item) => {
                Value::Initialized(initialized_to_wire(item)?)
            }
            contract::OperationResult::Cloned(item) => Value::Cloned(cloned_to_wire(item)?),
        }),
    })
}
pub(super) fn operation_result_from_wire(
    value: pb::OperationResult,
) -> Result<contract::OperationResult, Error> {
    use pb::operation_result::Value;
    Ok(match required(value.value)? {
        Value::Refresh(item) => contract::OperationResult::Refresh(refresh_from_wire(item)?),
        Value::Committed(item) => contract::OperationResult::Committed(committed_from_wire(item)?),
        Value::Amended(item) => contract::OperationResult::Amended(amended_from_wire(item)?),
        Value::TrackingChanged(item) => {
            contract::OperationResult::TrackingChanged(tracking_changed_from_wire(item)?)
        }
        Value::CheckedOut(item) => {
            contract::OperationResult::CheckedOut(checked_out_from_wire(item)?)
        }
        Value::RemoteRenamed(item) => {
            contract::OperationResult::RemoteRenamed(remote_renamed_from_wire(item)?)
        }
        Value::FastForwarded(item) => {
            contract::OperationResult::FastForwarded(fast_forwarded_from_wire(item)?)
        }
        Value::LeaseMatched(item) => {
            contract::OperationResult::LeaseMatched(lease_matched_from_wire(item)?)
        }
        Value::RemoteDeleted(item) => {
            contract::OperationResult::RemoteDeleted(remote_deleted_from_wire(item)?)
        }
        Value::TagCreated(item) => {
            contract::OperationResult::TagCreated(tag_created_from_wire(item)?)
        }
        Value::Reset(item) => contract::OperationResult::Reset(reset_from_wire(item)?),
        Value::Discarded(item) => contract::OperationResult::Discarded(discarded_from_wire(item)?),
        Value::StashSaved(item) => {
            contract::OperationResult::StashSaved(stash_saved_from_wire(item)?)
        }
        Value::StashApplied(item) => {
            contract::OperationResult::StashApplied(stash_applied_from_wire(item)?)
        }
        Value::StashDropped(item) => {
            contract::OperationResult::StashDropped(stash_dropped_from_wire(item)?)
        }
        Value::NeedsResolution(item) => {
            contract::OperationResult::NeedsResolution(needs_resolution_from_wire(item)?)
        }
        Value::IntegrationCompleted(item) => {
            contract::OperationResult::IntegrationCompleted(integration_completed_from_wire(item)?)
        }
        Value::WorktreeAdded(item) => {
            contract::OperationResult::WorktreeAdded(worktree_added_from_wire(item)?)
        }
        Value::WorktreeRepaired(item) => {
            contract::OperationResult::WorktreeRepaired(worktree_repaired_from_wire(item)?)
        }
        Value::WorktreeRegistrationChanged(item) => {
            contract::OperationResult::WorktreeRegistrationChanged(
                worktree_registration_changed_from_wire(item)?,
            )
        }
        Value::Initialized(item) => {
            contract::OperationResult::Initialized(initialized_from_wire(item)?)
        }
        Value::Cloned(item) => contract::OperationResult::Cloned(cloned_from_wire(item)?),
    })
}
pub(super) fn response_to_wire(value: &contract::Response) -> Result<pb::Response, Error> {
    Ok(pb::Response {
        id: Some(value.id.clone()),
        result: Some(match &value.result {
            Ok(reply) => pb::response::Result::Reply(reply_to_wire(reply)?),
            Err(error) => pb::response::Result::Error(failure_to_wire(error)?),
        }),
    })
}
pub(super) fn response_from_wire(value: pb::Response) -> Result<contract::Response, Error> {
    Ok(contract::Response {
        id: required(value.id)?,
        result: match required(value.result)? {
            pb::response::Result::Reply(reply) => Ok(reply_from_wire(reply)?),
            pb::response::Result::Error(error) => Err(failure_from_wire(error)?),
        },
    })
}
