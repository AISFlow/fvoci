//! Version-one, single-author private-project archive. This is content, never
//! authentication or operational backup data. ZIP I/O lives in the office child.
use std::collections::{BTreeMap, BTreeSet};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub use crate::native_history::{
    check_canonical_history, check_report_bounds, decode, inspect_native_target, log_reason,
    retained_closure_blocker, target_binding, target_content_digest, ArchiveError,
    NativeTargetInput, RetainedHistoryBlocker, RetainedRevisionMeta, MAX_BYTES, MAX_ENTRIES,
    MAX_GRAPH_BYTES,
};

pub const KIND: &str = "fvoci-native-user-archive";
pub const MAX_OBJECTS: usize = 256;

pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}
// Concrete SQL record shapes. deny_unknown_fields makes a newer required
// model fail rather than discard columns, and none contains credentials/grants.
macro_rules! record {
    ($name:ident { $($(#[$attr:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $($(#[$attr])* pub $field: $ty),* }
    };
}
record!(Project {
    id: Uuid, key: String, name: String, description: Option<String>, icon: Option<String>,
    visibility: String, root_document_id: Option<Uuid>, status: String, next_number: i32,
    created_by: Uuid, created_at: String, updated_at: String, deleted_at: Option<String>
});
record!(Workflow {
    id: Uuid,
    project_id: Uuid,
    created_at: String,
    updated_at: String
});
record!(Status {
    id: Uuid, project_id: Uuid, workflow_id: Uuid, name: String, category: String,
    sort_key: String, wip_limit: Option<i32>, created_at: String, updated_at: String
});
record!(Document {
    id: Uuid, title: String, icon: Option<String>, path: String, parent_id: Option<Uuid>,
    sort_key: String, project_id: Option<Uuid>, number: i32, status: String, schema_version: i32,
    text: String, chosung: String, version: i32, created_by: Uuid, created_at: String,
    updated_at: String, deleted_at: Option<String>, content_json: Value, kind: String
});
record!(Task {
    id: Uuid, project_id: Uuid, number: i32, title: String, r#type: String, priority: String,
    status_id: Uuid, start_date: Option<String>, due_date: Option<String>, due_at: Option<String>,
    estimate: Option<Value>, parent_id: Option<Uuid>, milestone_id: Option<Uuid>,
    recurrence: Option<Value>, sort_key: String, schema_version: i32, content_json: Value,
    version: i32, archived_at: Option<String>, deleted_at: Option<String>, created_by: Uuid,
    created_at: String, updated_at: String, text: String, chosung: String,
    // Migration 049 (current schema): NULL or the explicit 'minutes' unit of
    // an integral estimate. Absent on a schema without the column; a NULL
    // value travels as absent and lands as the column default (NULL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    estimate_unit: Option<String>
});
record!(Assignee {
    task_id: Uuid,
    user_id: Uuid
});
// Project labels and their task assignments (migration 015).
record!(Label {
    id: Uuid,
    project_id: Uuid,
    name: String,
    color: String,
    created_at: String,
    updated_at: String
});
record!(TaskLabel {
    task_id: Uuid,
    label_id: Uuid
});
// Project milestones and task dependencies (migration 017).
record!(Milestone {
    id: Uuid, project_id: Uuid, name: String, due_date: Option<String>, sort_key: String,
    created_at: String, updated_at: String
});
record!(TaskDependency {
    blocker_id: Uuid,
    blocked_id: Uuid,
    r#type: String,
    lag_days: i32
});
// Workspace document tags named by archived documents (migration 028).
record!(DocumentTag {
    id: Uuid,
    name: String,
    color: String,
    created_at: String,
    updated_at: String
});
record!(DocumentTagAssignment {
    document_id: Uuid,
    tag_id: Uuid
});
// Owner-private saved project views (migration 028 `views`).
record!(View {
    id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
    name: String,
    r#type: String,
    config: Value,
    created_at: String,
    updated_at: String
});
// Document/task comments (migration 011): replies, resolution and reactions.
record!(Comment {
    id: Uuid, document_id: Option<Uuid>, task_id: Option<Uuid>, parent_id: Option<Uuid>,
    created_by: Uuid, body: String, chosung: String, resolved_at: Option<String>,
    reactions: Value, created_at: String, updated_at: String
});
record!(Origin {
    task_id: Uuid, document_id: Uuid, request_id: Uuid, request_hash: String,
    anchor: Option<String>, created_at: String, updated_at: String
});
record!(Activity {
    id: Uuid, task_id: Uuid, actor_user_id: Option<Uuid>, channel: String, kind: String,
    changes: Value, created_at: String
});
record!(Revision {
    id: Uuid, target_kind: String, target_id: Uuid, snapshot_entry: String, encoding: i16,
    content_json: Value, text: String, reason: String, created_by: Option<Uuid>, created_at: String,
    // Migration 050 (current schema) restore provenance: all four set for a
    // 'restore' row, all NULL otherwise. Same absent/NULL rule as above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restored_from_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restore_correlation_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restore_base_tail_seq: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restore_committed_tail_seq: Option<i64>
});
record!(Attachment {
    id: Uuid, document_id: Option<Uuid>, task_id: Option<Uuid>, uploader_id: Uuid,
    name: String, mime: String, declared_mime: Option<String>, size_bytes: i64,
    image: bool, created_at: String, completed_at: String, payload_entry: String
});
// The migration028 baseline: one task collection per project and one item
// per task. Their UUIDs and timestamps are product identities (collection
// APIs, query cursors, value FKs), preserved exactly on restore.
record!(Collection {
    id: Uuid, project_id: Option<Uuid>, kind: String, name: String, version: i32,
    deleted_at: Option<String>, created_at: String, updated_at: String
});
record!(CollectionItem {
    id: Uuid, collection_id: Uuid, document_id: Option<Uuid>, task_id: Option<Uuid>,
    version: i32, created_at: String, updated_at: String
});
// Person-made collection state (migration 028): fields, options, the one
// value row of an item's field in its type's table, and collection views.
record!(CollectionField {
    id: Uuid, collection_id: Uuid, key: String, name: String, description: Option<String>,
    r#type: String, sort_key: String, version: i32, deleted_at: Option<String>,
    created_at: String, updated_at: String
});
record!(CollectionOption {
    id: Uuid, collection_id: Uuid, field_id: Uuid, key: String, label: String,
    sort_key: String, deleted_at: Option<String>
});
record!(CollectionValue {
    collection_id: Uuid, item_id: Uuid, field_id: Uuid, field_type: String,
    value_text: Option<String>, value_number: Option<String>, value_date: Option<String>,
    value_ts: Option<String>, value_bool: Option<bool>
});
record!(CollectionChoice {
    collection_id: Uuid,
    item_id: Uuid,
    field_id: Uuid,
    field_type: String,
    option_id: Uuid
});
record!(CollectionPerson {
    collection_id: Uuid,
    item_id: Uuid,
    field_id: Uuid,
    field_type: String,
    user_id: Uuid
});
record!(CollectionView {
    id: Uuid,
    collection_id: Uuid,
    owner_id: Uuid,
    visibility: String,
    name: String,
    r#type: String,
    config: Value,
    version: i32,
    created_at: String,
    updated_at: String
});
// Owner-private Zotero mirror (migration 051), the five portable tables.
// Only canonical columns travel: the connector's operational sync state is
// reset on restore and its sealed credential (zotero_credentials) is never
// read. Personal wiki documents (project_id NULL) back the references.
record!(ZoteroConnector {
    id: Uuid,
    library_type: String,
    remote_library_id: i64,
    library_url: String,
    completed_version: i64,
    created_at: String,
    updated_at: String
});
record!(ZoteroReference {
    id: Uuid, connector_id: Uuid, document_id: Option<Uuid>, item_key: String,
    remote_version: i64, local_version: i64, bibliography: Value, return_url: String,
    availability: String
});
record!(ZoteroCollection {
    connector_id: Uuid, collection_key: String, remote_version: i64, name: String,
    parent_key: Option<String>, availability: String
});
record!(ZoteroMembership {
    connector_id: Uuid,
    reference_id: Uuid,
    collection_key: String
});
record!(ZoteroLink {
    id: Uuid, connector_id: Uuid, reference_id: Uuid, document_id: Option<Uuid>,
    task_id: Option<Uuid>, anchor: String
});
// Personal-input capture receipts (migration 045) whose targets lie inside
// the selection. They travel as source bookkeeping and land RETIRED (all
// target columns NULL, key/hash/intent/created_at kept): never rehashed and
// never a live replay target in the destination.
record!(PersonalInputCommand {
    request_id: Uuid, request_hash: String, intent: String, document_id: Option<Uuid>,
    task_id: Option<Uuid>, project_id: Option<Uuid>, created_at: String
});
// Task time (migration 034 entries; 048 stopwatch runs/segments/legacy open
// reservations, command receipts and correction audit). Only the single
// source actor's rows travel (048 tables are actor-self; another author's
// entry is refused). Identities and typed JSON locators are preserved.
record!(TimeEntry {
    id: Uuid, task_id: Uuid, user_id: Uuid, started_at: String, ended_at: Option<String>,
    duration_seconds: Option<i32>, note: Option<String>
});
record!(TimerRun {
    id: Uuid, user_id: Uuid, task_id: Uuid, status: String, version: i32, started_at: String,
    stopped_at: Option<String>, note: Option<String>
});
record!(TimerSegment {
    id: Uuid, run_id: Uuid, user_id: Uuid, task_id: Uuid, started_at: String,
    ended_at: Option<String>, time_entry_id: Option<Uuid>
});
record!(TimerLegacyOpen {
    time_entry_id: Uuid,
    user_id: Uuid,
    task_id: Uuid
});
// Immutable replay receipts: publish lands them retired (052 marker), never
// as a live replay success; hash/result/created_at stay exact.
record!(TimerCommand {
    request_id: Uuid, user_id: Uuid, request_hash: String, run_id: Option<Uuid>,
    result: Value, created_at: String
});
record!(TimerAudit {
    id: Uuid, user_id: Uuid, request_id: Uuid, workspace_id: Option<Uuid>, task_id: Option<Uuid>,
    time_entry_id: Option<Uuid>, verb: String, before_value: Value, after_value: Value,
    reason: String, created_at: String
});
record!(Update {
    seq: i64,
    op_id: Uuid,
    payload_entry: String,
    created_at: String
});
record!(Receipt {
    op_id: Uuid,
    seq: i64,
    payload_len: i64,
    payload_sha256: String,
    actor_user_id: Uuid,
    created_at: String
});
record!(NativeState {
    target_kind: String, target_id: Uuid, state_entry: String, encoding: i16,
    snapshot_cutoff_seq: i64, tail_seq: i64, compacted_at: Option<String>,
    created_at: String, updated_at: String, updates: Vec<Update>, receipts: Vec<Receipt>
});
record!(Graph {
    source_workspace_id: Uuid, source_actor_id: Uuid, captured_at: String,
    project: Project, workflows: Vec<Workflow>, statuses: Vec<Status>, documents: Vec<Document>,
    tasks: Vec<Task>, assignees: Vec<Assignee>, labels: Vec<Label>, task_labels: Vec<TaskLabel>,
    milestones: Vec<Milestone>, dependencies: Vec<TaskDependency>, views: Vec<View>,
    document_tags: Vec<DocumentTag>, document_tag_assignments: Vec<DocumentTagAssignment>,
    origins: Vec<Origin>, activity: Vec<Activity>,
    // Ids of purged labels/milestones that the history still names (sorted).
    purged_label_refs: Vec<Uuid>, purged_milestone_refs: Vec<Uuid>, comments: Vec<Comment>,
    states: Vec<NativeState>, revisions: Vec<Revision>, attachments: Vec<Attachment>,
    collections: Vec<Collection>, collection_items: Vec<CollectionItem>,
    collection_fields: Vec<CollectionField>, collection_options: Vec<CollectionOption>,
    collection_values: Vec<CollectionValue>, collection_choices: Vec<CollectionChoice>,
    collection_people: Vec<CollectionPerson>, collection_views: Vec<CollectionView>,
    zotero_connectors: Vec<ZoteroConnector>, zotero_references: Vec<ZoteroReference>,
    zotero_collections: Vec<ZoteroCollection>, zotero_memberships: Vec<ZoteroMembership>,
    zotero_links: Vec<ZoteroLink>, personal_input_commands: Vec<PersonalInputCommand>,
    time_entries: Vec<TimeEntry>, timer_runs: Vec<TimerRun>, timer_segments: Vec<TimerSegment>,
    timer_legacy_open: Vec<TimerLegacyOpen>, timer_commands: Vec<TimerCommand>,
    timer_audit: Vec<TimerAudit>,
    native_inventory: Option<Vec<collab_engine::archive_history::NativeArchiveInventory>>
});
record!(EntryDigest {
    size: u64,
    sha256: String
});
record!(Manifest {
    kind: String, format_version: u32, native_encoding: u16, complete: bool,
    required_models: Vec<String>, mapping_policy: String, captured_at: String,
    entries: BTreeMap<String, EntryDigest>
});

// Base64 is solely bounded child IPC, never an alternative native encoding.
record!(Archive { graph: Graph, entries: BTreeMap<String, String> });

impl Archive {
    pub fn bytes(&self, name: &str) -> Result<Vec<u8>, ArchiveError> {
        decode(
            self.entries
                .get(name)
                .ok_or_else(|| ArchiveError::Invalid("missing entry".into()))?,
        )
    }

    pub fn validate(&self) -> Result<(), ArchiveError> {
        let g = &self.graph;
        let invalid = || ArchiveError::Invalid("graph affiliation or identity".into());
        if self.entries.len() > MAX_ENTRIES
            || g.documents.len() + g.tasks.len() > MAX_OBJECTS
            || g.documents.is_empty()
            || g.workflows.is_empty()
            || g.statuses.is_empty()
        {
            return Err(ArchiveError::Limit);
        }
        if serde_json::to_vec(g).map_err(|_| invalid())?.len() > MAX_GRAPH_BYTES {
            return Err(ArchiveError::Limit);
        }
        if let Some(reports) = g.native_inventory.as_deref() {
            let _ = inventory_index(Some(reports), g.states.len())?;
            if reports.iter().any(|r| {
                r.schema_version != 1
                    || r.binding.len() != 64
                    || !r
                        .binding
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                    || !collab_engine::archive_history::report_wire_fits(
                        r,
                        collab_engine::limits::MAX_PROJECT_JSON_BYTES,
                    )
            }) {
                return Err(ArchiveError::Invalid("native report schema or size".into()));
            }
        }

        if g.project.visibility != "private"
            || g.project.deleted_at.is_some()
            || g.project.created_by != g.source_actor_id
        {
            return Err(ArchiveError::Unsupported(
                "private single-author project required".into(),
            ));
        }
        if crate::projects::normalize_project_key(&g.project.key)
            .ok()
            .as_ref()
            != Some(&g.project.key)
            || !crate::projects::name_is_valid(&g.project.name)
            || !crate::projects::description_is_valid(g.project.description.as_deref())
            || !crate::projects::icon_is_valid(g.project.icon.as_deref())
            || !matches!(g.project.status.as_str(), "active" | "archived")
            || g.project.next_number < 1
        {
            return Err(invalid());
        }
        validate_dates(g)?;
        validate_dates(&g.project)?;
        let mut ids = BTreeSet::new();
        let mut insert = |id| {
            if ids.insert(id) {
                Ok(())
            } else {
                Err(invalid())
            }
        };
        insert(g.project.id)?;
        for id in g
            .workflows
            .iter()
            .map(|v| v.id)
            .chain(g.statuses.iter().map(|v| v.id))
            .chain(g.documents.iter().map(|v| v.id))
            .chain(g.tasks.iter().map(|v| v.id))
            .chain(g.labels.iter().map(|v| v.id))
            .chain(g.comments.iter().map(|v| v.id))
            .chain(g.revisions.iter().map(|v| v.id))
            .chain(g.attachments.iter().map(|v| v.id))
            .chain(g.activity.iter().map(|v| v.id))
            .chain(g.collections.iter().map(|v| v.id))
            .chain(g.collection_items.iter().map(|v| v.id))
            .chain(g.zotero_connectors.iter().map(|v| v.id))
            .chain(g.zotero_links.iter().map(|v| v.id))
        {
            insert(id)?;
        }
        let docs: BTreeSet<_> = g.documents.iter().map(|v| v.id).collect();
        let tasks: BTreeSet<_> = g.tasks.iter().map(|v| v.id).collect();
        let workflows: BTreeSet<_> = g.workflows.iter().map(|v| v.id).collect();
        let statuses: BTreeSet<_> = g.statuses.iter().map(|v| v.id).collect();
        if !g
            .project
            .root_document_id
            .is_some_and(|id| docs.contains(&id))
        {
            return Err(invalid());
        }
        // Project documents form one tree under the project root. Personal
        // wiki documents (project_id NULL) are numbered in the workspace and
        // travel only as the Zotero closure checked in validate_zotero.
        let wiki: BTreeSet<_> = g
            .documents
            .iter()
            .filter(|d| d.project_id.is_none())
            .map(|d| d.id)
            .collect();
        if g.documents
            .iter()
            .filter(|d| d.project_id.is_some() && d.parent_id.is_none())
            .count()
            != 1
            || !g.documents.iter().any(|d| {
                Some(d.id) == g.project.root_document_id
                    && d.project_id.is_some()
                    && d.parent_id.is_none()
            })
        {
            return Err(invalid());
        }
        let mut numbers = BTreeSet::new();
        if g.documents
            .iter()
            .filter(|d| d.project_id.is_some())
            .map(|d| d.number)
            .chain(g.tasks.iter().map(|t| t.number))
            .any(|n| n < 1 || n == i32::MAX || !numbers.insert(n))
        {
            return Err(invalid());
        }
        let mut wiki_numbers = BTreeSet::new();
        if g.documents
            .iter()
            .filter(|d| d.project_id.is_none())
            .any(|d| d.number < 1 || d.number == i32::MAX || !wiki_numbers.insert(d.number))
        {
            return Err(invalid());
        }
        for w in &g.workflows {
            validate_dates(w)?;
            if w.project_id != g.project.id {
                return Err(invalid());
            }
        }
        for s in &g.statuses {
            validate_dates(s)?;
            if s.project_id != g.project.id
                || !workflows.contains(&s.workflow_id)
                || !crate::db::workflow_statuses::status_name_is_valid(s.name.trim())
                || !crate::db::workflow_statuses::status_category_is_valid(&s.category)
            {
                return Err(invalid());
            }
        }
        for d in &g.documents {
            if d.project_id.is_some_and(|id| id != g.project.id)
                || d.created_by != g.source_actor_id
                || d.deleted_at.is_some()
                || d.kind != "doc"
                || d.parent_id.is_some_and(|id| {
                    !docs.contains(&id) || wiki.contains(&id) != d.project_id.is_none()
                })
            {
                return Err(invalid());
            }
            if d.schema_version != crate::db::documents::DOCUMENT_SCHEMA_VERSION {
                return Err(ArchiveError::Unsupported("document schema version".into()));
            }
            let path = match d.parent_id {
                Some(id) => format!(
                    "{}.{}",
                    g.documents
                        .iter()
                        .find(|p| p.id == id)
                        .ok_or_else(invalid)?
                        .path,
                    crate::db::documents::to_path_label(d.id)
                ),
                None => crate::db::documents::to_path_label(d.id),
            };
            if d.path != path
                || d.path.split('.').count() > 20
                || !crate::db::documents::title_is_valid(&d.title)
                || !crate::db::documents::status_is_valid(&d.status)
                || d.number < 1
                || d.version < 1
            {
                return Err(invalid());
            }
            validate_dates(d)?;
            validate_derived_body(&d.content_json, &d.text, Some(&d.chosung))?;
            validate_references(&d.content_json, &docs, &tasks, &g.attachments)?;
        }
        // Milestones (017) belong to this project with the writer's name, date
        // and fractional sort key rules; a task's milestone is one of them.
        let mut milestones = BTreeSet::new();
        for m in &g.milestones {
            validate_dates(m)?;
            if m.project_id != g.project.id
                || !crate::db::milestones::milestone_name_is_valid(&m.name)
                || m.due_date
                    .as_deref()
                    .is_some_and(|d| crate::tasks::parse_iso_date(d).is_none())
                || crate::db::documents::between(Some(&m.sort_key), None).is_err()
                || !milestones.insert(m.id)
            {
                return Err(invalid());
            }
        }
        for t in &g.tasks {
            let mut ancestors = BTreeSet::new();
            let mut current = Some(t.id);
            while let Some(id) = current {
                if !ancestors.insert(id) {
                    return Err(invalid());
                }
                current = g
                    .tasks
                    .iter()
                    .find(|parent| parent.id == id)
                    .ok_or_else(invalid)?
                    .parent_id;
            }
            if t.project_id != g.project.id
                || t.created_by != g.source_actor_id
                || t.deleted_at.is_some()
                || !statuses.contains(&t.status_id)
                || t.parent_id.is_some_and(|id| !tasks.contains(&id))
            {
                return Err(invalid());
            }
            if t.schema_version != crate::db::documents::DOCUMENT_SCHEMA_VERSION {
                return Err(ArchiveError::Unsupported("task schema version".into()));
            }
            if !crate::tasks::title_is_valid(&t.title)
                || !crate::tasks::task_type_is_valid(&t.r#type)
                || !crate::tasks::priority_is_valid(&t.priority)
                || t.number < 1
                || t.version < 1
                || t.start_date
                    .as_deref()
                    .is_some_and(|d| crate::tasks::parse_iso_date(d).is_none())
                || t.due_date
                    .as_deref()
                    .is_some_and(|d| crate::tasks::parse_iso_date(d).is_none())
                || t.estimate.as_ref().is_some_and(|v| {
                    v.as_str()
                        .is_none_or(|s| !crate::tasks::patch::estimate_is_valid(s))
                })
                || !estimate_unit_is_valid(t.estimate.as_ref(), t.estimate_unit.as_deref())
            {
                return Err(invalid());
            }
            validate_dates(t)?;
            if t.milestone_id.is_some_and(|id| !milestones.contains(&id)) {
                return Err(invalid());
            }
            // The writers' recurrence preset (create DTO deserializer, patch
            // recurrence_preset_is_valid): null or exactly {"kind": daily |
            // weekly | monthly}.
            if t.recurrence
                .as_ref()
                .is_some_and(|r| !recurrence_preset_is_valid(r))
            {
                return Err(invalid());
            }
            validate_derived_body(&t.content_json, &t.text, Some(&t.chosung))?;
            validate_references(&t.content_json, &docs, &tasks, &g.attachments)?;
        }
        let mut assignees = BTreeSet::new();
        for a in &g.assignees {
            if a.user_id != g.source_actor_id
                || !tasks.contains(&a.task_id)
                || !assignees.insert((a.task_id, a.user_id))
            {
                return Err(invalid());
            }
        }
        validate_dependencies(g, &tasks)?;
        let labels: BTreeSet<_> = g.labels.iter().map(|v| v.id).collect();
        for l in &g.labels {
            validate_dates(l)?;
            if l.project_id != g.project.id
                || !crate::db::labels::label_name_is_valid(&l.name)
                || !crate::db::labels::label_color_is_valid(&l.color)
            {
                return Err(invalid());
            }
        }
        // The writers hard-delete labels and milestones (purge); history keeps
        // their stored {id, historical name} and saved view/collection-view
        // filters keep the stored labelId/milestoneId. Each purged list is at
        // most MAX_ENTRIES long, strictly ascending, disjoint from the other
        // list and from live rows of either kind, and named as its own kind
        // by the history or an archived view filter (no padding). Purged ids
        // widen only those references (history, view label/milestone
        // filters), never task labels, milestones or any other field; a
        // reference in neither set stays its typed refusal.
        if g.purged_label_refs.len() > MAX_ENTRIES || g.purged_milestone_refs.len() > MAX_ENTRIES {
            return Err(ArchiveError::Limit);
        }
        let purged_ok = |list: &[Uuid], other: &[Uuid]| {
            list.windows(2).all(|w| w[0] < w[1])
                && list.iter().all(|id| {
                    !labels.contains(id)
                        && !milestones.contains(id)
                        && other.binary_search(id).is_err()
                })
        };
        if !purged_ok(&g.purged_label_refs, &g.purged_milestone_refs)
            || !purged_ok(&g.purged_milestone_refs, &g.purged_label_refs)
        {
            return Err(invalid());
        }
        // Bounded before the filters are collected (two per view row).
        activity_reference_count(&g.activity)?
            .checked_add(view_filter_ref_bound(&g.views, &g.collection_views)?)
            .filter(|count| *count <= MAX_GRAPH_BYTES / HISTORY_REFERENCE_BYTES)
            .ok_or(ArchiveError::Limit)?;
        let view_refs = view_filter_refs(&g.views, &g.collection_views);
        let (named_labels, named_milestones) =
            purged_refs(&g.activity, &view_refs, &labels, &milestones)?;
        if !g
            .purged_label_refs
            .iter()
            .all(|id| named_labels.contains(id))
            || !g
                .purged_milestone_refs
                .iter()
                .all(|id| named_milestones.contains(id))
        {
            return Err(invalid());
        }
        let label_refs: BTreeSet<Uuid> =
            labels.iter().chain(&g.purged_label_refs).copied().collect();
        let milestone_refs: BTreeSet<Uuid> = milestones
            .iter()
            .chain(&g.purged_milestone_refs)
            .copied()
            .collect();
        validate_views(g, &statuses, &label_refs, &milestone_refs)?;
        let mut task_labels = BTreeSet::new();
        for t in &g.task_labels {
            if !tasks.contains(&t.task_id)
                || !labels.contains(&t.label_id)
                || !task_labels.insert((t.task_id, t.label_id))
            {
                return Err(invalid());
            }
        }
        for id in &tasks {
            if g.task_labels.iter().filter(|t| t.task_id == *id).count() > MAX_TASK_REFS
                || g.assignees.iter().filter(|a| a.task_id == *id).count() > MAX_TASK_REFS
            {
                return Err(ArchiveError::Limit);
            }
        }
        validate_zotero(g, &wiki, &docs, &tasks)?;
        let comments: BTreeMap<Uuid, &Comment> = g.comments.iter().map(|c| (c.id, c)).collect();
        for c in &g.comments {
            validate_dates(c)?;
            let target_ok = match (c.document_id, c.task_id) {
                (Some(d), None) => docs.contains(&d),
                (None, Some(t)) => tasks.contains(&t),
                _ => false,
            };
            let parent_ok = c.parent_id.is_none_or(|p| {
                comments.get(&p).is_some_and(|parent| {
                    p != c.id && parent.document_id == c.document_id && parent.task_id == c.task_id
                })
            });
            if !target_ok
                || !parent_ok
                || c.created_by != g.source_actor_id
                || c.body != c.body.trim()
                || c.body.is_empty()
                || c.body.encode_utf16().count() > COMMENT_BODY_MAX
                || c.chosung != crate::collab::derived_body::to_chosung(&c.body)
                || (c.parent_id.is_some() && c.resolved_at.is_some())
                || !comment_reactions_supported(&c.reactions, g.source_actor_id)
            {
                return Err(invalid());
            }
        }
        // Reply chains end at a top-level comment (no cycles): every comment
        // is reached from one.
        comments_parent_first(&g.comments).map_err(|_| invalid())?;
        let mut origin_tasks = BTreeSet::new();
        let mut origin_commands = BTreeSet::new();
        for o in &g.origins {
            validate_dates(o)?;
            if !docs.contains(&o.document_id)
                || !tasks.contains(&o.task_id)
                || !origin_tasks.insert(o.task_id)
                || !origin_commands.insert((o.document_id, o.request_id))
                || o.anchor.as_ref().is_some_and(|a| a.chars().count() > 200)
            {
                return Err(invalid());
            }
        }
        let mut receipts = BTreeSet::new();
        for r in &g.personal_input_commands {
            validate_dates(r)?;
            if r.request_hash.len() != 64
                || !r
                    .request_hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || !matches!(r.intent.as_str(), "quick" | "note" | "task")
                || (r.intent != "task" && (r.task_id.is_some() || r.project_id.is_some()))
                || !receipts.insert(r.request_id)
            {
                return Err(invalid());
            }
            if r.document_id.is_some_and(|id| !docs.contains(&id))
                || r.task_id.is_some_and(|id| !tasks.contains(&id))
                || r.project_id.is_some_and(|id| id != g.project.id)
                || (r.document_id.is_none() && r.task_id.is_none() && r.project_id.is_none())
            {
                return Err(ArchiveError::Unsupported("personal input commands".into()));
            }
        }
        validate_task_time(g, &tasks)?;
        for a in &g.activity {
            validate_dates(a)?;
            let changes_ok = match a.kind.as_str() {
                "created" => a.changes == serde_json::json!([]),
                "changed" => activity_changes_supported(
                    &a.changes,
                    g.source_actor_id,
                    &label_refs,
                    &statuses,
                    &tasks,
                    &milestone_refs,
                ),
                _ => false,
            };
            if a.actor_user_id.is_some_and(|id| id != g.source_actor_id)
                || !tasks.contains(&a.task_id)
                || !changes_ok
                || !matches!(
                    a.channel.as_str(),
                    "web" | "api" | "mcp" | "webhook" | "system"
                )
            {
                return Err(ArchiveError::Unsupported(
                    "non-baseline task activity".into(),
                ));
            }
        }
        for id in &tasks {
            if g.activity
                .iter()
                .filter(|a| a.task_id == *id && a.kind == "created")
                .count()
                != 1
            {
                return Err(ArchiveError::Invalid("missing creation activity".into()));
            }
        }
        validate_collections(g, &docs, &tasks, &statuses, &label_refs, &milestone_refs)?;
        validate_document_tags(g, &docs)?;
        let mut expected = BTreeSet::new();
        let mut state_ids = BTreeSet::new();
        for s in &g.states {
            validate_dates(s)?;
            let target = match s.target_kind.as_str() {
                "document" => &docs,
                "task" => &tasks,
                _ => return Err(invalid()),
            };
            if !target.contains(&s.target_id)
                || !state_ids.insert((s.target_kind.clone(), s.target_id))
                || s.encoding != 1
                || s.snapshot_cutoff_seq < 0
                || s.tail_seq < s.snapshot_cutoff_seq
            {
                return Err(invalid());
            }
            let name = format!("native/{}/{}/state.v1", s.target_kind, s.target_id);
            if s.state_entry != name {
                return Err(invalid());
            }
            expected.insert(name);
            let mut seq = s.snapshot_cutoff_seq;
            for u in &s.updates {
                validate_dates(u)?;
                seq = seq.checked_add(1).ok_or_else(invalid)?;
                if u.seq != seq
                    || u.payload_entry
                        != format!("native/{}/{}/{}.v1", s.target_kind, s.target_id, seq)
                {
                    return Err(invalid());
                }
                let bytes = self.bytes(&u.payload_entry)?;
                if bytes.is_empty() || bytes.len() > 8 * 1024 * 1024 {
                    return Err(ArchiveError::Limit);
                }
                if !s.receipts.iter().any(|r| {
                    r.op_id == u.op_id
                        && r.seq == u.seq
                        && r.payload_len == bytes.len() as i64
                        && r.payload_sha256 == digest(&bytes)
                }) {
                    return Err(invalid());
                }
                expected.insert(u.payload_entry.clone());
            }
            let mut receipt_ops = BTreeSet::new();
            for receipt in &s.receipts {
                validate_dates(receipt)?;
                if !receipt_ops.insert(receipt.op_id)
                    || receipt.seq < 1
                    || receipt.seq > s.tail_seq
                    || !(1..=8 * 1024 * 1024).contains(&receipt.payload_len)
                    || hex::decode(&receipt.payload_sha256).is_err()
                    || receipt.payload_sha256.len() != 64
                {
                    return Err(invalid());
                }
            }
            if seq != s.tail_seq
                || s.receipts
                    .iter()
                    .any(|r| r.actor_user_id != g.source_actor_id)
            {
                return Err(invalid());
            }
            let bytes = self.bytes(&s.state_entry)?;
            if bytes.is_empty() || bytes.len() > 8 * 1024 * 1024 {
                return Err(ArchiveError::Limit);
            }
        }
        // A body never opened natively has no state row. The product creates
        // the row on first open only for the empty document body
        // (db::collab::ensure_collab_state_row refuses any other stateless
        // body), so exactly that body may travel stateless; any other
        // stateless body is refused before publication. A revision always
        // needs its target's state (below), and capture refuses
        // updates/receipts/revisions without a state.
        let empty = crate::db::documents::empty_document_json();
        let stateless_body_is_empty = |kind: &str, id: Uuid, body: &Value| {
            state_ids.contains(&(kind.to_owned(), id)) || *body == empty
        };
        if !g
            .documents
            .iter()
            .all(|d| stateless_body_is_empty("document", d.id, &d.content_json))
            || !g
                .tasks
                .iter()
                .all(|t| stateless_body_is_empty("task", t.id, &t.content_json))
        {
            return Err(ArchiveError::Unsupported(
                "missing native state for captured body".into(),
            ));
        }
        let mut restore_correlations = BTreeSet::new();
        for r in &g.revisions {
            validate_dates(r)?;
            let state_tail = g
                .states
                .iter()
                .find(|s| s.target_kind == r.target_kind && s.target_id == r.target_id)
                .map(|s| s.tail_seq);
            if !state_ids.contains(&(r.target_kind.clone(), r.target_id))
                || r.encoding != 1
                || !matches!(
                    r.reason.as_str(),
                    "manual" | "session" | "scheduled" | "restore"
                )
                || !restore_metadata_is_valid(r, state_tail)
                || r.restore_correlation_id
                    .is_some_and(|c| !restore_correlations.insert(c))
                || r.created_by.is_some_and(|id| id != g.source_actor_id)
                || r.snapshot_entry != format!("revisions/{}.snapshot.v1", r.id)
            {
                return Err(invalid());
            }
            validate_references(&r.content_json, &docs, &tasks, &g.attachments)?;
            validate_derived_body(&r.content_json, &r.text, None)?;
            expected.insert(r.snapshot_entry.clone());
        }
        for a in &g.attachments {
            validate_dates(a)?;
            if a.uploader_id != g.source_actor_id
                || a.size_bytes <= 0
                || (a.document_id.is_some() == a.task_id.is_some())
                || a.document_id.is_some_and(|id| !docs.contains(&id))
                || a.task_id.is_some_and(|id| !tasks.contains(&id))
                || a.payload_entry != format!("attachments/{}/payload", a.id)
            {
                return Err(invalid());
            }
            if self.bytes(&a.payload_entry)?.len() as i64 != a.size_bytes {
                return Err(invalid());
            }
            expected.insert(a.payload_entry.clone());
        }
        if expected != self.entries.keys().cloned().collect() {
            return Err(ArchiveError::Invalid("entry allowlist".into()));
        }
        let mut total = 0usize;
        for name in expected {
            total = total
                .checked_add(self.bytes(&name)?.len())
                .ok_or(ArchiveError::Limit)?;
            if total > MAX_BYTES {
                return Err(ArchiveError::Limit);
            }
        }
        Ok(())
    }
}

/// Migration 049: an explicit unit is only 'minutes' over a stored integral
/// estimate in 0..=i32::MAX (exact decimal text, never a guessed unit).
fn estimate_unit_is_valid(estimate: Option<&Value>, unit: Option<&str>) -> bool {
    match unit {
        None => true,
        Some("minutes") => estimate.and_then(Value::as_str).is_some_and(|text| {
            let integral = text.split_once('.').map_or(text, |(int, frac)| {
                if frac.bytes().all(|b| b == b'0') {
                    int
                } else {
                    ""
                }
            });
            !integral.is_empty()
                && integral.bytes().all(|b| b.is_ascii_digit())
                && integral.parse::<u64>().is_ok_and(|n| n <= i32::MAX as u64)
        }),
        Some(_) => false,
    }
}

/// Migration 050: a 'restore' revision names its actor, source revision,
/// correlation and adjacent tails (committed = base + 1, within the target's
/// archived tail); every other reason carries none of them.
fn restore_metadata_is_valid(r: &Revision, state_tail: Option<i64>) -> bool {
    let fields = (
        r.restored_from_id,
        r.restore_correlation_id,
        r.restore_base_tail_seq,
        r.restore_committed_tail_seq,
    );
    if r.reason != "restore" {
        return fields == (None, None, None, None);
    }
    match fields {
        (Some(_), Some(_), Some(base), Some(committed)) => {
            r.created_by.is_some()
                && base >= 0
                && committed.checked_sub(base) == Some(1)
                && state_tail.is_some_and(|tail| committed <= tail)
        }
        _ => false,
    }
}

/// 034/048 task time: rows of the single source actor on archived tasks, the
/// tables' own CHECK/uniqueness rules, and every relation inside the archive.
/// The task writers' recurrence preset: exactly one key "kind" whose value
/// is daily, weekly or monthly.
fn recurrence_preset_is_valid(value: &Value) -> bool {
    value.as_object().is_some_and(|preset| {
        preset.len() == 1
            && matches!(
                preset.get("kind").and_then(Value::as_str),
                Some("daily" | "weekly" | "monthly")
            )
    })
}

/// Workspace document tags (028) travel with the archived documents that
/// carry them: exactly the tags those assignments name, each with the tag
/// writers' trimmed name (parse_name) and palette color, unique ids, and an
/// assignment of an archived document to an archived tag at most once. The
/// case-insensitive name uniqueness stays the database's (lower(name)), so a
/// destination with the same name is an ordinary Conflict on restore.
fn validate_document_tags(g: &Graph, docs: &BTreeSet<Uuid>) -> Result<(), ArchiveError> {
    let invalid = || ArchiveError::Invalid("document tags".into());
    let mut tags = BTreeSet::new();
    for t in &g.document_tags {
        validate_dates(t)?;
        if crate::collections::parse_name(&Value::String(t.name.clone()))
            .ok()
            .as_deref()
            != Some(t.name.as_str())
            || !crate::db::labels::label_color_is_valid(&t.color)
            || !tags.insert(t.id)
        {
            return Err(invalid());
        }
    }
    let mut pairs = BTreeSet::new();
    let mut named = BTreeSet::new();
    for a in &g.document_tag_assignments {
        if !docs.contains(&a.document_id)
            || !tags.contains(&a.tag_id)
            || !pairs.insert((a.document_id, a.tag_id))
        {
            return Err(invalid());
        }
        named.insert(a.tag_id);
    }
    if named != tags {
        return Err(invalid());
    }
    Ok(())
}

/// The project's collections (028): its one trigger-made task collection,
/// with its real name/version (the project's name at creation; a later
/// project rename does not change it), plus live document collections of the
/// project. Every task is an item of the task collection; document items are
/// archived project documents. Fields, options, values, choices, people and
/// collection views follow the collection writers' rules (crate::collections
/// limits and value shapes, one value row of an item's field in its type's
/// table, at most one choice of a select and one person of a user field,
/// canonical numbers and view configs). Person-scoped rows of another person
/// are the typed "collection people" / "collection views" refusals; deleted
/// or extra task collections the typed "collections" refusal. These are the
/// current single-author slice, not product exclusions.
fn validate_collections(
    g: &Graph,
    docs: &BTreeSet<Uuid>,
    tasks: &BTreeSet<Uuid>,
    statuses: &BTreeSet<Uuid>,
    labels: &BTreeSet<Uuid>,
    milestones: &BTreeSet<Uuid>,
) -> Result<(), ArchiveError> {
    use crate::collections::{FieldType, FIELDS_PER_COLLECTION_MAX, FIELD_DESCRIPTION_MAX};
    let invalid = || ArchiveError::Invalid("collections".into());
    let unsupported = || ArchiveError::Unsupported("collections".into());
    let name_ok = |name: &str| {
        crate::collections::parse_name(&Value::String(name.to_owned()))
            .ok()
            .as_deref()
            == Some(name)
    };
    let mut kinds = BTreeMap::new();
    // Live workspace wiki document collections (project_id NULL) travel when
    // every item is an archived wiki document (the item writer keeps them to
    // wiki documents); each must hold at least one, so no unrelated
    // workspace collection rides along.
    let mut wiki = BTreeSet::new();
    for c in &g.collections {
        validate_dates(c)?;
        if c.deleted_at.is_some() {
            return Err(unsupported());
        }
        match c.project_id {
            Some(project) if project == g.project.id => {}
            None if c.kind == "document" => {
                wiki.insert(c.id);
            }
            None if c.kind == "task" => return Err(invalid()),
            _ => return Err(unsupported()),
        }
        // The task collection's name is the project's name at creation (the
        // 028 trigger copies it): the project name contract; a document
        // collection's name is the collection writer's.
        let named = match c.kind.as_str() {
            "task" => crate::projects::name_is_valid(&c.name),
            _ => name_ok(&c.name),
        };
        if !matches!(c.kind.as_str(), "task" | "document")
            || c.version < 1
            || !named
            || kinds.insert(c.id, c.kind.as_str()).is_some()
        {
            return Err(invalid());
        }
    }
    if kinds.values().filter(|k| **k == "task").count() != 1 {
        return Err(unsupported());
    }
    let mut items = BTreeMap::new();
    let mut item_tasks = BTreeSet::new();
    let mut item_docs = BTreeSet::new();
    for item in &g.collection_items {
        validate_dates(item)?;
        let target_ok = match (
            kinds.get(&item.collection_id),
            item.document_id,
            item.task_id,
        ) {
            (Some(&"task"), None, Some(t)) => tasks.contains(&t) && item_tasks.insert(t),
            (Some(&"document"), Some(d), None) => {
                // A project collection holds project documents, a wiki
                // collection wiki documents (both archived).
                let home = (!wiki.contains(&item.collection_id)).then_some(g.project.id);
                docs.contains(&d)
                    && g.documents
                        .iter()
                        .any(|x| x.id == d && x.project_id == home)
                    && item_docs.insert(d)
            }
            _ => false,
        };
        if !target_ok || item.version < 1 || items.insert(item.id, item.collection_id).is_some() {
            return Err(invalid());
        }
    }
    if item_tasks.len() != tasks.len() {
        return Err(unsupported());
    }
    if wiki.iter().any(|c| !items.values().any(|owner| owner == c)) {
        return Err(invalid());
    }
    let mut fields = BTreeMap::new();
    let mut keys = BTreeSet::new();
    for f in &g.collection_fields {
        validate_dates(f)?;
        let field_type = FieldType::parse(&f.r#type).ok_or_else(invalid)?;
        if !kinds.contains_key(&f.collection_id)
            || !crate::collections::field_key_is_valid(&f.key)
            || !keys.insert((f.collection_id, f.key.as_str()))
            || !name_ok(&f.name)
            || f.description
                .as_ref()
                .is_some_and(|d| d.encode_utf16().count() > FIELD_DESCRIPTION_MAX)
            || f.sort_key.is_empty()
            || f.version < 1
            || fields.insert(f.id, (f.collection_id, field_type)).is_some()
        {
            return Err(invalid());
        }
    }
    for id in kinds.keys() {
        if fields.values().filter(|(c, _)| c == id).count() > FIELDS_PER_COLLECTION_MAX {
            return Err(invalid());
        }
    }
    // The field writers' catalog cap counts archived options too (a patch
    // must list every existing option): a larger catalog is not current data
    // and would leave the restored field uneditable.
    for id in fields.keys() {
        if g.collection_options
            .iter()
            .filter(|o| o.field_id == *id)
            .count()
            > crate::collections::PATCH_OPTIONS_MAX
        {
            return Err(invalid());
        }
    }
    let mut options = BTreeMap::new();
    let mut option_keys = BTreeSet::new();
    for o in &g.collection_options {
        validate_dates(o)?;
        if !fields
            .get(&o.field_id)
            .is_some_and(|(c, t)| *c == o.collection_id && t.has_options())
            || !option_keys.insert((o.field_id, o.key.as_str()))
            || o.key.is_empty()
            || !name_ok(&o.label)
            || o.sort_key.is_empty()
            || options.insert(o.id, o.field_id).is_some()
        {
            return Err(invalid());
        }
    }
    // One (item, field) uses one table; the row's collection/type match.
    let mut cells = BTreeMap::new();
    let mut cell = |collection: Uuid, item: Uuid, field: Uuid, field_type: &str, table: u8| {
        let Some((field_collection, declared)) = fields.get(&field) else {
            return Err(invalid());
        };
        if *field_collection != collection
            || items.get(&item) != Some(&collection)
            || declared.as_str() != field_type
        {
            return Err(invalid());
        }
        match cells.insert((item, field), table) {
            Some(previous) if previous != table => Err(invalid()),
            _ => Ok(*declared),
        }
    };
    let mut scalar_cells = BTreeSet::new();
    for v in &g.collection_values {
        let field_type = cell(v.collection_id, v.item_id, v.field_id, &v.field_type, 0)?;
        let present = [
            v.value_text.is_some(),
            v.value_number.is_some(),
            v.value_date.is_some(),
            v.value_ts.is_some(),
            v.value_bool.is_some(),
        ];
        let shape_ok = present.iter().filter(|p| **p).count() == 1
            && match field_type {
                FieldType::Text | FieldType::Paragraph => v.value_text.as_ref().is_some_and(|t| {
                    t.encode_utf16().count() <= crate::collections::VALUE_TEXT_MAX
                }),
                // numeric::text as captured: a plain finite decimal.
                FieldType::Number => v.value_number.as_ref().is_some_and(|n| {
                    let digits = n.strip_prefix('-').unwrap_or(n);
                    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, "0"));
                    !whole.is_empty()
                        && !fraction.is_empty()
                        && whole
                            .bytes()
                            .chain(fraction.bytes())
                            .all(|b| b.is_ascii_digit())
                        && n.parse::<f64>().is_ok_and(f64::is_finite)
                }),
                FieldType::Date => v
                    .value_date
                    .as_deref()
                    .is_some_and(|d| crate::tasks::parse_iso_date(d).is_some()),
                FieldType::Datetime => v
                    .value_ts
                    .as_deref()
                    .is_some_and(|d| chrono::DateTime::parse_from_rfc3339(d).is_ok()),
                FieldType::Checkbox => v.value_bool.is_some(),
                _ => false,
            };
        if !shape_ok || !scalar_cells.insert((v.item_id, v.field_id)) {
            return Err(invalid());
        }
    }
    let mut choices = BTreeMap::<(Uuid, Uuid), BTreeSet<Uuid>>::new();
    for c in &g.collection_choices {
        let field_type = cell(c.collection_id, c.item_id, c.field_id, &c.field_type, 1)?;
        let chosen = choices.entry((c.item_id, c.field_id)).or_default();
        if !field_type.has_options()
            || options.get(&c.option_id) != Some(&c.field_id)
            || !chosen.insert(c.option_id)
            || (field_type == FieldType::Select && chosen.len() > 1)
            || chosen.len() > crate::collections::VALUE_OPTIONS_MAX
        {
            return Err(invalid());
        }
    }
    let mut people = BTreeMap::<(Uuid, Uuid), usize>::new();
    for p in &g.collection_people {
        let field_type = cell(p.collection_id, p.item_id, p.field_id, &p.field_type, 2)?;
        if !field_type.is_people() {
            return Err(invalid());
        }
        if p.user_id != g.source_actor_id {
            return Err(ArchiveError::Unsupported("collection people".into()));
        }
        let count = people.entry((p.item_id, p.field_id)).or_default();
        *count += 1;
        if *count > 1 {
            // Only the single source actor may appear, once per cell.
            return Err(invalid());
        }
    }
    let mut views = BTreeSet::new();
    for v in &g.collection_views {
        validate_dates(v)?;
        if v.owner_id != g.source_actor_id {
            return Err(ArchiveError::Unsupported("collection views".into()));
        }
        let config = crate::collections::parse_query_config(&v.config).map_err(|_| invalid())?;
        // The writer checks group/date fields at write time; a later field
        // deletion leaves the stored view as it is, so any deletion state.
        let live = |id: Uuid, accept: &dyn Fn(FieldType) -> bool| {
            g.collection_fields.iter().any(|f| {
                f.id == id
                    && f.collection_id == v.collection_id
                    && FieldType::parse(&f.r#type).is_some_and(accept)
            })
        };
        let task_collection = kinds.get(&v.collection_id) == Some(&"task");
        let group_ok = match config.group_by {
            None => true,
            Some(crate::collections::GroupBy::Status) => task_collection,
            Some(crate::collections::GroupBy::Field(id)) => live(id, &|t| t == FieldType::Select),
        };
        let date_ok = match config.date_by {
            None => true,
            Some(crate::collections::DateBy::Due | crate::collections::DateBy::Start) => {
                task_collection
            }
            Some(crate::collections::DateBy::Field(id)) => {
                live(id, &|t| matches!(t, FieldType::Date | FieldType::Datetime))
            }
        };
        if !kinds.contains_key(&v.collection_id)
            || !matches!(v.visibility.as_str(), "private" | "shared")
            || crate::collections::CollectionViewType::parse(&v.r#type).is_none()
            || !name_ok(&v.name)
            || v.config.to_string().len() > 262_144
            || config.to_json() != v.config
            || v.version < 1
            || !group_ok
            || !date_ok
            || !views.insert(v.id)
        {
            return Err(invalid());
        }
        check_view_query(
            &config.query,
            g,
            statuses,
            labels,
            milestones,
            Some(v.collection_id),
            !task_collection,
        )?;
    }
    Ok(())
}

/// Saved project views are owner-private: only the source actor's own views
/// travel (another person's view is the typed "views" refusal). Each keeps
/// the writer's shape: project, type, trimmed name, size, and the canonical
/// config the writer stores (view_query_to_json of its parse); its query is
/// checked by check_view_query against the project's task collection.
fn validate_views(
    g: &Graph,
    statuses: &BTreeSet<Uuid>,
    labels: &BTreeSet<Uuid>,
    milestones: &BTreeSet<Uuid>,
) -> Result<(), ArchiveError> {
    let invalid = || ArchiveError::Invalid("views".into());
    let mut ids = BTreeSet::new();
    for v in &g.views {
        validate_dates(v)?;
        if v.user_id != g.source_actor_id {
            return Err(ArchiveError::Unsupported("views".into()));
        }
        let query =
            crate::tasks::list_query::parse_view_query_value(&v.config).map_err(|_| invalid())?;
        if v.project_id != g.project.id
            || !crate::collections::PROJECT_VIEW_TYPES.contains(&v.r#type.as_str())
            || crate::collections::parse_name(&Value::String(v.name.clone()))
                .ok()
                .as_ref()
                != Some(&v.name)
            || v.config.to_string().len() > 262_144
            || crate::tasks::list_query::view_query_to_json(&query) != v.config
            || !ids.insert(v.id)
        {
            return Err(invalid());
        }
        // A project view's custom fields are the project's task collection's.
        let task_collection = g
            .collections
            .iter()
            .find(|c| c.kind == "task")
            .map(|c| c.id);
        check_view_query(
            &query,
            g,
            statuses,
            labels,
            milestones,
            task_collection,
            false,
        )?;
    }
    Ok(())
}

/// A stored view query as db::view_query::compile_view_query accepts it at
/// write time. Writer shape (Invalid "view query"): no task-only filter or
/// sort on a document collection; a custom equals value of the field type's
/// form (option/people UUID text, number, checkbox bool, ISO date, RFC 3339
/// datetime, text); field sorts only on scalar (value column) fields.
/// References (typed "view references" when outside this archive): status
/// of this project; milestone and label of this project, live or a listed
/// purged reference (the caller passes those sets); assignee "me" or the
/// source actor;
/// custom fields and sort fields of the query's own collection - in any
/// deletion state, since a later field/option deletion leaves stored views
/// as they are; an option equals value an option of that field; a people
/// equals value the source actor (remapped on restore).
fn check_view_query(
    query: &crate::tasks::list_query::ViewQuery,
    g: &Graph,
    statuses: &BTreeSet<Uuid>,
    labels: &BTreeSet<Uuid>,
    milestones: &BTreeSet<Uuid>,
    collection: Option<Uuid>,
    document_kind: bool,
) -> Result<(), ArchiveError> {
    use crate::collections::FieldType;
    use crate::tasks::list_query::{AssigneeFilter, CustomOperator, CustomValue, SortField};
    let invalid = || ArchiveError::Invalid("view query".into());
    let outside = || ArchiveError::Unsupported("view references".into());
    let f = &query.filters;
    if document_kind
        && (f.task_type.is_some()
            || f.status_id.is_some()
            || f.assignee_id.is_some()
            || f.priority.is_some()
            || f.label_id.is_some()
            || f.milestone_id.is_some()
            || f.open_only
            || f.due_before.is_some()
            || query.sort.iter().any(|s| {
                matches!(
                    s.field,
                    SortField::Priority | SortField::Due | SortField::Status
                )
            }))
    {
        return Err(invalid());
    }
    let field_type = |id: Uuid| {
        g.collection_fields
            .iter()
            .find(|f| f.id == id && Some(f.collection_id) == collection)
            .and_then(|f| FieldType::parse(&f.r#type))
    };
    // db::view_query's parse_uuid_text: exactly the 36-character hyphenated
    // form (either case); compact or braced spellings are not written.
    let uuid_text = |raw: &str| {
        (raw.len() == 36)
            .then(|| Uuid::parse_str(raw).ok())
            .flatten()
            .ok_or_else(invalid)
    };
    for c in &f.custom {
        let kind = field_type(c.field_id).ok_or_else(outside)?;
        match (&c.operator, kind) {
            (CustomOperator::Empty, _) => {}
            (CustomOperator::Equals(CustomValue::Text(raw)), k) if k.has_options() => {
                let option = uuid_text(raw)?;
                if !g
                    .collection_options
                    .iter()
                    .any(|o| o.id == option && o.field_id == c.field_id)
                {
                    return Err(outside());
                }
            }
            (CustomOperator::Equals(CustomValue::Text(raw)), k) if k.is_people() => {
                if uuid_text(raw)? != g.source_actor_id {
                    return Err(outside());
                }
            }
            (CustomOperator::Equals(CustomValue::Number(_)), FieldType::Number)
            | (CustomOperator::Equals(CustomValue::Bool(_)), FieldType::Checkbox) => {}
            (CustomOperator::Equals(CustomValue::Text(raw)), FieldType::Date)
                if crate::tasks::parse_iso_date(raw).is_some() => {}
            (CustomOperator::Equals(CustomValue::Text(raw)), FieldType::Datetime)
                if crate::tasks::parse_iso_datetime(raw).is_some() => {}
            (
                CustomOperator::Equals(CustomValue::Text(_)),
                FieldType::Text | FieldType::Paragraph,
            ) => {}
            _ => return Err(invalid()),
        }
    }
    for s in &query.sort {
        if let SortField::Field(id) = s.field {
            let kind = field_type(id).ok_or_else(outside)?;
            if crate::db::view_query::value_column(kind.as_str()).is_none() {
                return Err(invalid());
            }
        }
    }
    if f.status_id.is_some_and(|id| !statuses.contains(&id))
        || f.label_id.is_some_and(|id| !labels.contains(&id))
        || f.milestone_id.is_some_and(|id| !milestones.contains(&id))
        || matches!(f.assignee_id, Some(AssigneeFilter::User(id)) if id != g.source_actor_id)
    {
        return Err(outside());
    }
    Ok(())
}

/// The restored copy of a validated view query: the assignee filter naming
/// the source actor and a people-field custom value whose UUID (any spelling
/// the writer accepted) is the source actor name the destination actor.
/// Text, option and every other value stay byte-for-byte as stored.
fn mapped_view_query(
    query: &crate::tasks::list_query::ViewQuery,
    people: &dyn Fn(Uuid) -> bool,
    source: Uuid,
    destination: Uuid,
) -> crate::tasks::list_query::ViewQuery {
    use crate::tasks::list_query::{AssigneeFilter, CustomOperator, CustomValue};
    let mut query = query.clone();
    if query.filters.assignee_id == Some(AssigneeFilter::User(source)) {
        query.filters.assignee_id = Some(AssigneeFilter::User(destination));
    }
    for c in &mut query.filters.custom {
        if !people(c.field_id) {
            continue;
        }
        if let CustomOperator::Equals(CustomValue::Text(raw)) = &mut c.operator {
            if Uuid::parse_str(raw).is_ok_and(|id| id == source) {
                *raw = destination.to_string();
            }
        }
    }
    query
}

/// A project view config (a view query) as restored; see mapped_view_query.
pub fn mapped_project_view_config(
    config: &Value,
    people: &dyn Fn(Uuid) -> bool,
    source: Uuid,
    destination: Uuid,
) -> Value {
    use crate::tasks::list_query::{parse_view_query_value, view_query_to_json};
    match parse_view_query_value(config) {
        Ok(query) => view_query_to_json(&mapped_view_query(&query, people, source, destination)),
        Err(_) => config.clone(),
    }
}

/// A collection view config ({query, groupBy, dateBy}) as restored.
pub fn mapped_collection_view_config(
    config: &Value,
    people: &dyn Fn(Uuid) -> bool,
    source: Uuid,
    destination: Uuid,
) -> Value {
    match crate::collections::parse_query_config(config) {
        Ok(mut parsed) => {
            parsed.query = mapped_view_query(&parsed.query, people, source, destination);
            parsed.to_json()
        }
        Err(_) => config.clone(),
    }
}

/// Task dependencies stay inside the archived tasks with the writer's rules:
/// no self edge, FS/SS/FF, non-negative lag, one row per ordered pair and no
/// cycle (add_task_dependency refuses one). Calendar inequalities are the
/// writer's edit-time check against the workspace holidays, not re-derived.
fn validate_dependencies(g: &Graph, tasks: &BTreeSet<Uuid>) -> Result<(), ArchiveError> {
    let invalid = || ArchiveError::Invalid("dependencies".into());
    let mut edges: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
    let mut pairs = BTreeSet::new();
    for d in &g.dependencies {
        if d.blocker_id == d.blocked_id
            || !tasks.contains(&d.blocker_id)
            || !tasks.contains(&d.blocked_id)
            || !matches!(d.r#type.as_str(), "FS" | "SS" | "FF")
            || d.lag_days < 0
            || !pairs.insert((d.blocker_id, d.blocked_id))
        {
            return Err(invalid());
        }
        edges.entry(d.blocker_id).or_default().push(d.blocked_id);
    }
    // Iterative depth-first search: 1 = on the current path, 2 = finished.
    let mut state: BTreeMap<Uuid, u8> = BTreeMap::new();
    for &start in edges.keys() {
        if state.contains_key(&start) {
            continue;
        }
        let mut stack = vec![(start, 0usize)];
        state.insert(start, 1);
        while let Some((node, next)) = stack.pop() {
            let successors = edges.get(&node).map(Vec::as_slice).unwrap_or(&[]);
            if let Some(&child) = successors.get(next) {
                stack.push((node, next + 1));
                match state.get(&child) {
                    Some(1) => return Err(invalid()),
                    Some(_) => {}
                    None => {
                        state.insert(child, 1);
                        stack.push((child, 0));
                    }
                }
            } else {
                state.insert(node, 2);
            }
        }
    }
    Ok(())
}

fn validate_task_time(g: &Graph, tasks: &BTreeSet<Uuid>) -> Result<(), ArchiveError> {
    let invalid = || ArchiveError::Invalid("task time".into());
    let actor = g.source_actor_id;
    let ts = |v: &str| chrono::DateTime::parse_from_rfc3339(v).map_err(|_| invalid());
    let note_ok = |n: &Option<String>| n.as_ref().is_none_or(|n| n.chars().count() <= 2000);
    let mut entries = BTreeMap::new();
    let mut open_entry = false;
    for e in &g.time_entries {
        validate_dates(e)?;
        if e.user_id != actor {
            return Err(ArchiveError::Unsupported("time entries".into()));
        }
        let started = ts(&e.started_at)?;
        let consistent = match (&e.ended_at, e.duration_seconds) {
            (None, None) => {
                if open_entry {
                    return Err(invalid());
                }
                open_entry = true;
                true
            }
            (Some(ended), Some(seconds)) => {
                let span = ts(ended)? - started;
                span.num_milliseconds() > 0
                    && seconds > 0
                    && i64::from(seconds) == span.num_seconds()
            }
            _ => false,
        };
        if !tasks.contains(&e.task_id)
            || !consistent
            || !note_ok(&e.note)
            || entries.insert(e.id, e).is_some()
        {
            return Err(invalid());
        }
    }
    let mut runs = BTreeMap::new();
    let mut unfinished = false;
    for r in &g.timer_runs {
        validate_dates(r)?;
        let stopped = r.status == "stopped";
        if r.user_id != actor
            || !tasks.contains(&r.task_id)
            || !matches!(r.status.as_str(), "running" | "paused" | "stopped")
            || r.version < 1
            || stopped != r.stopped_at.is_some()
            || r.stopped_at.as_deref().is_some_and(|s| {
                ts(s)
                    .ok()
                    .zip(ts(&r.started_at).ok())
                    .is_none_or(|(s, b)| s < b)
            })
            || !note_ok(&r.note)
            || (!stopped && std::mem::replace(&mut unfinished, true))
            || runs.insert(r.id, r).is_some()
        {
            return Err(invalid());
        }
    }
    let mut open_segments = BTreeSet::new();
    let mut segment_entries = BTreeSet::new();
    let mut segments = BTreeSet::new();
    for s in &g.timer_segments {
        validate_dates(s)?;
        let run = runs.get(&s.run_id).ok_or_else(invalid)?;
        let ended_ok = s.ended_at.as_deref().is_none_or(|e| {
            ts(e)
                .ok()
                .zip(ts(&s.started_at).ok())
                .is_some_and(|(e, b)| e >= b)
        });
        if s.user_id != run.user_id
            || s.task_id != run.task_id
            || !ended_ok
            || (s.ended_at.is_none() && !open_segments.insert(s.run_id))
            || s.time_entry_id.is_some_and(|id| {
                s.ended_at.is_none()
                    || !segment_entries.insert(id)
                    || entries.get(&id).is_none_or(|e| e.task_id != s.task_id)
            })
            || !segments.insert(s.id)
        {
            return Err(invalid());
        }
    }
    let mut reservations = BTreeSet::new();
    for l in &g.timer_legacy_open {
        let entry = entries.get(&l.time_entry_id).ok_or_else(invalid)?;
        if l.user_id != actor
            || entry.ended_at.is_some()
            || entry.task_id != l.task_id
            || !reservations.insert(l.time_entry_id)
        {
            return Err(invalid());
        }
    }
    // The person-global active rule counts reservations, not open 034 rows:
    // a released open entry stays and a later run may be unfinished, but a
    // reserved entry and an unfinished run never coexist (048 start/trigger).
    if !reservations.is_empty() && unfinished {
        return Err(invalid());
    }
    let mut commands = BTreeSet::new();
    for c in &g.timer_commands {
        validate_dates(c)?;
        if c.user_id != actor
            || c.request_hash.chars().count() != 64
            || c.run_id.is_some_and(|id| !runs.contains_key(&id))
            || !commands.insert(c.request_id)
        {
            return Err(invalid());
        }
    }
    let segment_tasks: BTreeMap<Uuid, Uuid> =
        g.timer_segments.iter().map(|s| (s.id, s.task_id)).collect();
    let mut audits = BTreeSet::new();
    // A historical workspace locator (the operation ran before an ordinary
    // MOVE re-homed the task) is actor-private provenance, accepted only when
    // the audit also locates a selected task or entry; it grants nothing.
    for a in &g.timer_audit {
        validate_dates(a)?;
        if a.user_id != actor
            || a.workspace_id.is_some_and(|w| {
                w != g.source_workspace_id && a.task_id.is_none() && a.time_entry_id.is_none()
            })
            || a.task_id.is_some_and(|t| !tasks.contains(&t))
            || a.time_entry_id.is_some_and(|e| !entries.contains_key(&e))
            || a.verb.is_empty()
            || !(1..=2000).contains(&a.reason.chars().count())
            || !audits.insert(a.id)
        {
            return Err(invalid());
        }
        // Typed association, by the writers' own value shapes (record_audit,
        // TimerCommandOutput, LegacyReleaseOutput, task estimate): a recordId
        // names a selected record of its recorded kind, a runId a selected
        // run, a timeEntryId the entry column; each, like the entry column,
        // belongs to the audit's task when the audit names one. NULL values
        // (a start's absent run, an un-closed segment) and NULL-locator cleanup
        // stay valid; an estimate audit carries no record reference.
        let in_task = |owner: Uuid| a.task_id.is_none_or(|task| task == owner);
        if a.time_entry_id
            .is_some_and(|e| !in_task(entries[&e].task_id))
        {
            return Err(invalid());
        }
        for side in [&a.before_value, &a.after_value] {
            let Some(value) = side.as_object() else {
                continue;
            };
            let id = |key: &str| -> Result<Option<Uuid>, ArchiveError> {
                match value.get(key) {
                    None | Some(Value::Null) => Ok(None),
                    Some(v) => v
                        .as_str()
                        .and_then(|s| Uuid::parse_str(s).ok())
                        .map(Some)
                        .ok_or_else(invalid),
                }
            };
            if let Some(record) = id("recordId")? {
                let owner = match value.get("kind").and_then(Value::as_str) {
                    Some("manual") => entries.get(&record).map(|e| e.task_id),
                    Some("segment") => segment_tasks.get(&record).copied(),
                    _ => None,
                };
                if !owner.is_some_and(in_task) {
                    return Err(invalid());
                }
            }
            if let Some(run) = id("runId")? {
                if !runs.get(&run).is_some_and(|r| in_task(r.task_id)) {
                    return Err(invalid());
                }
            }
            if let Some(entry) = id("timeEntryId")? {
                if a.time_entry_id != Some(entry) {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(())
}

/// Selected connectors per archive; bounded before any source reads.
pub const MAX_ZOTERO_CONNECTORS: usize = 8;

/// The W6 policy (library descriptor, saved return URL, stored bibliography
/// schema, keys) applied to the archived mirror, plus the canonical closure:
/// every wiki document is a reference's backing document or its ancestor,
/// and every row's target is inside the archive. Credentials have no field.
fn validate_zotero(
    g: &Graph,
    wiki: &BTreeSet<Uuid>,
    docs: &BTreeSet<Uuid>,
    tasks: &BTreeSet<Uuid>,
) -> Result<(), ArchiveError> {
    use crate::api::zotero_dto::{Bibliography, LibraryType};
    use crate::integrations::zotero::{self, Library};
    let invalid = || ArchiveError::Invalid("zotero record".into());
    let outside = || ArchiveError::Unsupported("zotero closure".into());
    if g.zotero_connectors.len() > MAX_ZOTERO_CONNECTORS {
        return Err(ArchiveError::Limit);
    }
    let mut libraries = BTreeMap::new();
    let mut descriptors = BTreeSet::new();
    for c in &g.zotero_connectors {
        validate_dates(c)?;
        let kind = match c.library_type.as_str() {
            "user" => LibraryType::User,
            "group" => LibraryType::Group,
            _ => return Err(invalid()),
        };
        let library = Library::new(kind, &c.remote_library_id.to_string(), &c.library_url)
            .map_err(|_| invalid())?;
        if library.website != c.library_url
            || c.completed_version < 0
            || !descriptors.insert((c.library_type.as_str(), c.remote_library_id))
        {
            return Err(invalid());
        }
        libraries.insert(c.id, library);
    }
    let mut references = BTreeMap::new();
    let mut item_keys = BTreeSet::new();
    let mut backing = BTreeSet::new();
    for r in &g.zotero_references {
        let library = libraries.get(&r.connector_id).ok_or_else(outside)?;
        let bibliography: Bibliography =
            serde_json::from_value(r.bibliography.clone()).map_err(|_| invalid())?;
        zotero::validate_bibliography(&bibliography).map_err(|_| invalid())?;
        if library
            .validate_saved_return_url(&r.item_key, &r.return_url)
            .map_err(|_| invalid())?
            != r.return_url
            || r.return_url.len() > 2048
            || r.remote_version < 0
            || r.local_version < 1
            || !matches!(
                r.availability.as_str(),
                "available" | "trashed" | "deleted" | "excluded"
            )
            || !item_keys.insert((r.connector_id, r.item_key.as_str()))
            || references.insert(r.id, r.connector_id).is_some()
        {
            return Err(invalid());
        }
        if let Some(document) = r.document_id {
            // W6 creates each reference with its backing document's identity.
            if document != r.id || !backing.insert(document) {
                return Err(invalid());
            }
            if !wiki.contains(&document) {
                return Err(outside());
            }
        }
    }
    for id in libraries.keys() {
        if g.zotero_references
            .iter()
            .filter(|r| r.connector_id == *id)
            .count()
            > zotero::KEY_MAX
        {
            return Err(ArchiveError::Limit);
        }
    }
    // Wiki documents travel only as required dependencies: Zotero backing
    // documents and personal-input task origin documents, with ancestors.
    let roots = backing.iter().copied().chain(
        g.origins
            .iter()
            .map(|o| o.document_id)
            .filter(|id| wiki.contains(id)),
    );
    let wiki_closure = || ArchiveError::Unsupported("wiki closure".into());
    let mut closure = BTreeSet::new();
    for id in roots {
        let mut current = Some(id);
        while let Some(id) = current {
            if !closure.insert(id) {
                break;
            }
            current = g
                .documents
                .iter()
                .find(|d| d.id == id)
                .ok_or_else(wiki_closure)?
                .parent_id;
        }
    }
    if closure != *wiki {
        return Err(wiki_closure());
    }
    let mut collections = BTreeMap::new();
    for c in &g.zotero_collections {
        if !libraries.contains_key(&c.connector_id) {
            return Err(outside());
        }
        if !zotero::key_valid(&c.collection_key)
            || c.parent_key
                .as_deref()
                .is_some_and(|k| !zotero::key_valid(k))
            || c.parent_key.as_deref() == Some(c.collection_key.as_str())
            || c.name.len() > 4096
            || c.name.contains('\0')
            || c.remote_version < 0
            || !matches!(c.availability.as_str(), "available" | "deleted")
        {
            return Err(invalid());
        }
        collections
            .entry(c.connector_id)
            .or_insert_with(Vec::new)
            .push(zotero::Collection {
                key: c.collection_key.clone(),
                version: c.remote_version,
                name: c.name.clone(),
                parent: c.parent_key.clone(),
            });
    }
    // Unique keys, parents inside the same connector, no cycles: W6's graph check.
    for graph in collections.values() {
        zotero::collection_graph(graph).map_err(|_| invalid())?;
    }
    let mut memberships = BTreeSet::new();
    for m in &g.zotero_memberships {
        if references.get(&m.reference_id) != Some(&m.connector_id)
            || !collections
                .get(&m.connector_id)
                .is_some_and(|c| c.iter().any(|c| c.key == m.collection_key))
        {
            return Err(outside());
        }
        if !memberships.insert((m.connector_id, m.reference_id, m.collection_key.as_str())) {
            return Err(invalid());
        }
    }
    let mut links = BTreeSet::new();
    for l in &g.zotero_links {
        if references.get(&l.reference_id) != Some(&l.connector_id) {
            return Err(outside());
        }
        match (l.document_id, l.task_id) {
            (Some(d), None) if docs.contains(&d) => {}
            (None, Some(t)) if tasks.contains(&t) => {}
            (Some(_), None) | (None, Some(_)) => return Err(outside()),
            _ => return Err(invalid()),
        }
        // PostgreSQL text cannot hold NUL: refuse it here, before admission.
        if l.anchor.chars().count() > 256
            || l.anchor.contains('\0')
            || !links.insert((l.reference_id, l.document_id, l.task_id, l.anchor.as_str()))
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn validate_dates<T: Serialize>(record: &T) -> Result<(), ArchiveError> {
    let value = serde_json::to_value(record).map_err(|_| ArchiveError::Invalid("record".into()))?;
    for (key, value) in value
        .as_object()
        .ok_or_else(|| ArchiveError::Invalid("record".into()))?
    {
        if key.ends_with("_at")
            && !value.is_null()
            && value
                .as_str()
                .is_none_or(|s| chrono::DateTime::parse_from_rfc3339(s).is_err())
        {
            return Err(ArchiveError::Invalid("record timestamp".into()));
        }
    }
    Ok(())
}

fn validate_derived_body(
    body: &Value,
    text: &str,
    chosung: Option<&str>,
) -> Result<(), ArchiveError> {
    let prepared = crate::collab::derived_body::prepare_derived_body(body.clone())
        .map_err(|_| ArchiveError::Invalid("body schema or size".into()))?;
    if prepared.text() != text || chosung.is_some_and(|s| prepared.chosung() != s) {
        return Err(ArchiveError::Invalid("derived body disagreement".into()));
    }
    Ok(())
}

fn validate_references(
    body: &Value,
    docs: &BTreeSet<Uuid>,
    tasks: &BTreeSet<Uuid>,
    attachments: &[Attachment],
) -> Result<(), ArchiveError> {
    crate::collab::derived_body::prepare_derived_body(body.clone())
        .map_err(|_| ArchiveError::Invalid("body schema or budget".into()))?;
    fn walk(
        v: &Value,
        docs: &BTreeSet<Uuid>,
        tasks: &BTreeSet<Uuid>,
        files: &[Attachment],
    ) -> Result<(), ArchiveError> {
        if let Some(kind) = v.get("type").and_then(Value::as_str) {
            if matches!(kind, "mention" | "embed" | "attachment") {
                let attrs = &v["attrs"];
                let field = if kind == "embed" { "ref" } else { "id" };
                let id = attrs[field]
                    .as_str()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .ok_or_else(|| ArchiveError::Invalid("reference id".into()))?;
                let supported = if kind == "attachment" {
                    files.iter().any(|f| f.id == id)
                } else {
                    match attrs["entity"].as_str() {
                        Some("document") => docs.contains(&id),
                        Some("task") => tasks.contains(&id),
                        _ => false,
                    }
                };
                if !supported {
                    return Err(ArchiveError::Unsupported(
                        "reference outside selected closure".into(),
                    ));
                }
            }
        }
        match v {
            Value::Array(items) => {
                for item in items {
                    walk(item, docs, tasks, files)?;
                }
            }
            Value::Object(fields) => {
                for value in fields.values() {
                    walk(value, docs, tasks, files)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    walk(body, docs, tasks, attachments)
}

/// The existing process boundary owns byte parsing, limits and cancellation.
/// No ZIP reader is linked into this request/worker orchestration path.
pub async fn container(
    helper: &std::path::Path,
    bytes: Vec<u8>,
    pack: bool,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<String, ArchiveError> {
    use crate::documents::office::{
        run_office_helper, OfficeKind, OfficeLimits, OfficeMode, OfficeOutcome,
    };
    let limits = OfficeLimits {
        max_input_bytes: (MAX_BYTES / 3 * 4 + MAX_GRAPH_BYTES) as u64,
        max_output: MAX_BYTES / 3 * 4 + MAX_GRAPH_BYTES,
        timeout: std::time::Duration::from_secs(300),
        address_space: 1536 * 1024 * 1024,
    };
    match run_office_helper(
        helper,
        bytes,
        OfficeKind::NativeArchive,
        if pack {
            OfficeMode::Text
        } else {
            OfficeMode::Markdown
        },
        &limits,
        cancel,
    )
    .await
    .map_err(|_| ArchiveError::Cancelled)?
    {
        OfficeOutcome::Ok {
            text,
            truncated: false,
        } => Ok(text),
        OfficeOutcome::Unsupported { detail } => Err(ArchiveError::Unsupported(detail)),
        OfficeOutcome::Corrupt { detail } => Err(ArchiveError::Invalid(detail)),
        OfficeOutcome::ResourceLimit { .. } => Err(ArchiveError::Limit),
        _ => Err(ArchiveError::Worker),
    }
}

pub async fn parse(
    helper: &std::path::Path,
    bytes: Vec<u8>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Archive, ArchiveError> {
    let result = container(helper, bytes, false, cancel).await?;
    serde_json::from_str(&result).map_err(|_| ArchiveError::Invalid("child graph result".into()))
}

pub async fn read_file(
    storage: &crate::attachments::ObjectStorage,
    key: &str,
    size: i64,
) -> Result<Vec<u8>, ArchiveError> {
    use futures_util::StreamExt;
    if size <= 0 || size as usize > MAX_BYTES {
        return Err(ArchiveError::Limit);
    }
    if storage
        .head(key)
        .await
        .map_err(|_| ArchiveError::Invalid("missing file".into()))?
        != Some(size as u64)
    {
        return Err(ArchiveError::Invalid("file size".into()));
    }
    let mut stream = storage
        .open_payload_stream(key, 0, size as u64 - 1)
        .await
        .map_err(|_| ArchiveError::Invalid("file open".into()))?;
    let mut output = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ArchiveError::Invalid("file read".into()))?;
        if output.len().saturating_add(chunk.len()) > size as usize {
            return Err(ArchiveError::Limit);
        }
        output.extend_from_slice(&chunk);
    }
    if output.len() != size as usize {
        return Err(ArchiveError::Invalid("truncated file".into()));
    }
    Ok(output)
}

/// Product task limit on assignee and label references (db::tasks).
const MAX_TASK_REFS: usize = 50;
/// db::comments::COMMENT_BODY_MAX (UTF-16 code units) and its reaction set.
const COMMENT_BODY_MAX: usize = crate::db::comments::COMMENT_BODY_MAX;
const COMMENT_REACTIONS: &[&str] = crate::db::comments::VALID_REACTIONS;

/// Stored comment reactions (db::comments::reactions_to_json): an object of
/// product reaction emoji to unique user-ID strings, here only the single
/// source actor (remapped on restore).
fn comment_reactions_supported(reactions: &Value, actor: Uuid) -> bool {
    reactions.as_object().is_some_and(|map| {
        map.iter().all(|(emoji, ids)| {
            COMMENT_REACTIONS.contains(&emoji.as_str())
                && ids.as_array().is_some_and(|ids| {
                    ids.len() <= 1
                        && ids.iter().all(|id| {
                            id.as_str().and_then(|s| Uuid::parse_str(s).ok()) == Some(actor)
                        })
                })
        })
    })
}

/// The restored copy of validated comment reactions: the source actor's
/// reactions become the destination actor's.
pub fn mapped_comment_reactions(reactions: &Value, source: Uuid, destination: Uuid) -> Value {
    let mut reactions = reactions.clone();
    if let Some(map) = reactions.as_object_mut() {
        for ids in map.values_mut() {
            if let Some(ids) = ids.as_array_mut() {
                for id in ids.iter_mut() {
                    if id.as_str().and_then(|s| Uuid::parse_str(s).ok()) == Some(source) {
                        *id = Value::String(destination.to_string());
                    }
                }
            }
        }
    }
    reactions
}

/// The archived comments with every parent before its replies: top-level
/// comments in archive order, each followed depth-first by its replies in
/// archive order (an explicit stack; no recursion). Each comment is visited
/// once; a repeated comment id, or a comment never reached from a top-level
/// comment (a reply cycle, or a parent that is not an archived comment), is
/// Invalid. The id and reply indexes are BTree collections, so O(n log n) for
/// n comments (n is capped by the capture row limit).
pub fn comments_parent_first(comments: &[Comment]) -> Result<Vec<&Comment>, ArchiveError> {
    let invalid = || ArchiveError::Invalid("comment parent order".into());
    let mut ids = BTreeSet::new();
    if !comments.iter().all(|comment| ids.insert(comment.id)) {
        return Err(invalid());
    }
    let mut replies: BTreeMap<Uuid, Vec<usize>> = BTreeMap::new();
    let mut stack = Vec::new();
    for (index, comment) in comments.iter().enumerate().rev() {
        match comment.parent_id {
            Some(parent) => replies.entry(parent).or_default().push(index),
            None => stack.push(index),
        }
    }
    // With unique ids each comment is pushed at most once (only by its own
    // parent), so the stack never revisits.
    let mut order = Vec::with_capacity(comments.len());
    while let Some(index) = stack.pop() {
        let comment = &comments[index];
        order.push(comment);
        // Replies were collected in reverse archive order, so pushing them as
        // they are pops them in archive order.
        if let Some(children) = replies.get(&comment.id) {
            stack.extend(children.iter().copied());
        }
    }
    if order.len() != comments.len() {
        return Err(invalid());
    }
    Ok(order)
}

/// JSON bytes charged to the graph budget per extracted history reference
/// (a 36-character id, its quotes and a separator).
pub const HISTORY_REFERENCE_BYTES: usize = 39;

/// The (field, side value) pairs of labelIds/milestoneId changes in recorded
/// task activity.
fn label_milestone_sides(activity: &[Activity]) -> impl Iterator<Item = (bool, &Value)> {
    activity
        .iter()
        .filter_map(|a| a.changes.as_array())
        .flatten()
        .filter_map(|change| {
            let field = change.get("field")?.as_str()?;
            matches!(field, "labelIds" | "milestoneId").then_some((field == "labelIds", change))
        })
        .flat_map(|(is_label, change)| {
            ["from", "to"]
                .into_iter()
                .filter_map(move |side| change.get(side).map(|v| (is_label, v)))
        })
}

/// How many label/milestone references recorded task activity holds (list
/// items and objects, any shape), counted with checked arithmetic before
/// anything is collected; more than the graph budget can hold is a Limit.
pub fn activity_reference_count(activity: &[Activity]) -> Result<usize, ArchiveError> {
    label_milestone_sides(activity).try_fold(0usize, |count, (is_label, v)| {
        let items = if is_label {
            v.as_array().map_or(0, Vec::len)
        } else {
            usize::from(v.is_object())
        };
        count
            .checked_add(items)
            .filter(|count| *count <= MAX_GRAPH_BYTES / HISTORY_REFERENCE_BYTES)
            .ok_or(ArchiveError::Limit)
    })
}

/// The most label/milestone filters the archived views can name (two per
/// view row), known before any config is parsed or collected.
pub fn view_filter_ref_bound(
    views: &[View],
    collection_views: &[CollectionView],
) -> Result<usize, ArchiveError> {
    views
        .len()
        .checked_add(collection_views.len())
        .and_then(|rows| rows.checked_mul(2))
        .ok_or(ArchiveError::Limit)
}

/// The (is label, id) filters that archived project views and collection
/// views name (labelId, milestoneId), read through the parsers validation
/// uses; an unparsable config is left to validation's typed refusal. At most
/// view_filter_ref_bound entries.
pub fn view_filter_refs(views: &[View], collection_views: &[CollectionView]) -> Vec<(bool, Uuid)> {
    let project = views
        .iter()
        .filter_map(|v| crate::tasks::list_query::parse_view_query_value(&v.config).ok());
    let collection = collection_views
        .iter()
        .filter_map(|v| crate::collections::parse_query_config(&v.config).ok())
        .map(|config| config.query);
    project
        .chain(collection)
        .flat_map(|query| {
            let f = query.filters;
            f.label_id
                .map(|id| (true, id))
                .into_iter()
                .chain(f.milestone_id.map(|id| (false, id)))
        })
        .collect()
}

/// The purged label and milestone identities that recorded task activity
/// (well-formed stored references; any other shape is left to the activity
/// validation's typed refusal) and archived view filters name: ids that are
/// not a live row of the same kind. Occurrences are bounded first (the
/// history's counted references plus the view filters); repeated
/// references to one id are free, and a new distinct id beyond MAX_ENTRIES
/// per kind is a Limit before it is inserted. One id named as both a label
/// and a milestone, or a named id that is a live row of the other kind, is
/// not data the writers produce.
pub fn purged_refs(
    activity: &[Activity],
    view_refs: &[(bool, Uuid)],
    live_labels: &BTreeSet<Uuid>,
    live_milestones: &BTreeSet<Uuid>,
) -> Result<(BTreeSet<Uuid>, BTreeSet<Uuid>), ArchiveError> {
    activity_reference_count(activity)?
        .checked_add(view_refs.len())
        .filter(|count| *count <= MAX_GRAPH_BYTES / HISTORY_REFERENCE_BYTES)
        .ok_or(ArchiveError::Limit)?;
    let id = |v: &Value| {
        v.get("id")
            .and_then(Value::as_str)
            .and_then(|raw| Uuid::parse_str(raw).ok())
    };
    let admit = |set: &mut BTreeSet<Uuid>, live: &BTreeSet<Uuid>, id: Uuid| {
        if live.contains(&id) || set.contains(&id) {
            return Ok(());
        }
        if set.len() >= MAX_ENTRIES {
            return Err(ArchiveError::Limit);
        }
        set.insert(id);
        Ok(())
    };
    let (mut labels, mut milestones) = (BTreeSet::new(), BTreeSet::new());
    for (is_label, v) in label_milestone_sides(activity) {
        if is_label {
            for found in v.as_array().into_iter().flatten().filter_map(id) {
                admit(&mut labels, live_labels, found)?;
            }
        } else if let Some(found) = id(v) {
            admit(&mut milestones, live_milestones, found)?;
        }
    }
    for &(is_label, found) in view_refs {
        if is_label {
            admit(&mut labels, live_labels, found)?;
        } else {
            admit(&mut milestones, live_milestones, found)?;
        }
    }
    if labels
        .iter()
        .any(|id| milestones.contains(id) || live_milestones.contains(id))
        || milestones.iter().any(|id| live_labels.contains(id))
    {
        return Err(ArchiveError::Unsupported(
            "non-baseline task activity".into(),
        ));
    }
    Ok((labels, milestones))
}

/// A recorded "changed" task activity (tasks::activity::diff_activity over
/// db::task_activity snapshots) is portable when every change names a
/// distinct product activity field and each stored value has exactly that
/// field's snapshot shape with identities inside this archive: scalars pass
/// the product task rules; statusId is {id: archived status, label}; parentId
/// null or {id: archived task, label: null}; assigneeIds [{id: the single
/// source actor, label: null}] (remapped on restore); labelIds [{id: archived
/// or purged label, label: historical name or null}]; milestoneId null or {id:
/// archived or purged milestone, label: historical name or null}; recurrence
/// null or the preset's kind (daily, weekly, monthly). Anything else fails
/// closed.
fn activity_changes_supported(
    changes: &Value,
    actor: Uuid,
    labels: &BTreeSet<Uuid>,
    statuses: &BTreeSet<Uuid>,
    tasks: &BTreeSet<Uuid>,
    milestones: &BTreeSet<Uuid>,
) -> bool {
    let Some(list) = changes.as_array() else {
        return false;
    };
    if list.is_empty() || list.len() > crate::tasks::activity::ACTIVITY_FIELDS.len() {
        return false;
    }
    // A stored reference object: exactly {id, label}, the id a UUID string.
    let reference = |v: &Value| -> Option<(Uuid, Value)> {
        let object = v.as_object()?;
        if object.len() != 2 {
            return None;
        }
        let id = Uuid::parse_str(object.get("id")?.as_str()?).ok()?;
        Some((id, object.get("label")?.clone()))
    };
    // A list side is a set of distinct identities (the product records it from
    // PK-backed rows): a repeat, under any textual spelling, is refused.
    let references = |v: &Value, ok: &dyn Fn(Uuid, &Value) -> bool| {
        v.as_array().is_some_and(|items| {
            let mut ids = BTreeSet::new();
            items.len() <= MAX_TASK_REFS
                && items.iter().all(|item| {
                    reference(item).is_some_and(|(id, label)| ids.insert(id) && ok(id, &label))
                })
        })
    };
    let string = |v: &Value, ok: &dyn Fn(&str) -> bool| v.as_str().is_some_and(ok);
    let optional = |v: &Value, ok: &dyn Fn(&str) -> bool| v.is_null() || string(v, ok);
    let mut seen = BTreeSet::new();
    list.iter().all(|change| {
        let Some(object) = change.as_object() else {
            return false;
        };
        let (Some(field), Some(from), Some(to)) = (
            object.get("field").and_then(Value::as_str),
            object.get("from"),
            object.get("to"),
        ) else {
            return false;
        };
        if object.len() != 3
            || !crate::tasks::activity::ACTIVITY_FIELDS.contains(&field)
            || !seen.insert(field)
        {
            return false;
        }
        [from, to].into_iter().all(|v| match field {
            "title" => string(v, &crate::tasks::title_is_valid),
            "type" => string(v, &crate::tasks::task_type_is_valid),
            "priority" => string(v, &crate::tasks::priority_is_valid),
            "startDate" | "dueDate" => optional(v, &|s| crate::tasks::parse_iso_date(s).is_some()),
            "dueAt" => optional(v, &|s| chrono::DateTime::parse_from_rfc3339(s).is_ok()),
            "estimate" => optional(v, &crate::tasks::patch::estimate_is_valid),
            "archived" => v.is_boolean(),
            "statusId" => reference(v).is_some_and(|(id, label)| {
                statuses.contains(&id)
                    && (label.is_null()
                        || string(&label, &|s| {
                            crate::db::workflow_statuses::status_name_is_valid(s.trim())
                        }))
            }),
            "parentId" => {
                v.is_null()
                    || reference(v)
                        .is_some_and(|(id, label)| tasks.contains(&id) && label.is_null())
            }
            "assigneeIds" => references(v, &|id, label| id == actor && label.is_null()),
            "labelIds" => references(v, &|id, label| {
                labels.contains(&id)
                    && (label.is_null() || string(label, &crate::db::labels::label_name_is_valid))
            }),
            "milestoneId" => {
                v.is_null()
                    || reference(v).is_some_and(|(id, label)| {
                        milestones.contains(&id)
                            && (label.is_null()
                                || string(&label, &crate::db::milestones::milestone_name_is_valid))
                    })
            }
            // The activity snapshot records the preset's kind or null.
            "recurrence" => {
                v.is_null() || matches!(v.as_str(), Some("daily" | "weekly" | "monthly"))
            }
            _ => false,
        })
    })
}

/// The restored copy of a validated activity's changes: assignee reference
/// IDs (only ever the source actor) become the destination actor.
pub fn mapped_activity_changes(changes: &Value, source: Uuid, destination: Uuid) -> Value {
    let mut changes = changes.clone();
    if let Some(list) = changes.as_array_mut() {
        for change in list {
            if change.get("field").and_then(Value::as_str) == Some("assigneeIds") {
                for side in ["from", "to"] {
                    if let Some(items) = change.get_mut(side).and_then(Value::as_array_mut) {
                        for item in items.iter_mut() {
                            let is_source = item
                                .get("id")
                                .and_then(Value::as_str)
                                .and_then(|s| Uuid::parse_str(s).ok())
                                == Some(source);
                            if is_source {
                                item["id"] = Value::String(destination.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    changes
}

fn archive_target(
    archive: &Archive,
    state: &NativeState,
) -> Result<NativeTargetInput, ArchiveError> {
    let body = if state.target_kind == "document" {
        &archive
            .graph
            .documents
            .iter()
            .find(|d| d.id == state.target_id)
            .ok_or_else(|| ArchiveError::Invalid("target".into()))?
            .content_json
    } else {
        &archive
            .graph
            .tasks
            .iter()
            .find(|t| t.id == state.target_id)
            .ok_or_else(|| ArchiveError::Invalid("target".into()))?
            .content_json
    };
    let mut tail = Vec::with_capacity(state.updates.len());
    for row in &state.updates {
        tail.push((row.seq, row.op_id, archive.bytes(&row.payload_entry)?));
    }
    let mut revisions = Vec::new();
    for revision in archive
        .graph
        .revisions
        .iter()
        .filter(|r| r.target_kind == state.target_kind && r.target_id == state.target_id)
    {
        revisions.push((
            revision.id,
            revision.encoding,
            archive.bytes(&revision.snapshot_entry)?,
            revision.content_json.clone(),
        ));
    }
    Ok(NativeTargetInput {
        kind: state.target_kind.clone(),
        id: state.target_id,
        encoding: state.encoding,
        cutoff: state.snapshot_cutoff_seq,
        tail_seq: state.tail_seq,
        snapshot: archive.bytes(&state.state_entry)?,
        tail,
        body: body.clone(),
        revisions,
    })
}

fn native_capture_binding(archive: &Archive, target: &NativeTargetInput) -> String {
    target_binding(
        archive.graph.source_workspace_id,
        archive.graph.source_actor_id,
        &archive.graph.captured_at,
        target,
    )
}

fn validate_native_inventory(
    archive: &Archive,
    report: &collab_engine::archive_history::NativeArchiveInventory,
    binding: &str,
    limits: &collab_engine::Limits,
) -> Result<(), ArchiveError> {
    use collab_engine::archive_history::ReferenceKind;
    check_report_bounds(report, binding, limits)?;
    let blocker = retained_closure_blocker(report, |kind, id| match kind {
        ReferenceKind::Document => archive.graph.documents.iter().any(|d| d.id == id),
        ReferenceKind::Task => archive.graph.tasks.iter().any(|t| t.id == id),
        ReferenceKind::Attachment => archive.graph.attachments.iter().any(|f| f.id == id),
        ReferenceKind::LinkHref => false,
    });
    match blocker {
        None => Ok(()),
        Some(RetainedHistoryBlocker::Incomplete) => Err(ArchiveError::Unsupported(format!(
            "retained native identity/coverage incomplete: {:?}",
            report.diagnostics.first().map(|d| d.reason)
        ))),
        Some(blocker) => Err(ArchiveError::Unsupported(blocker.reason().into())),
    }
}

fn inventory_index(
    reports: Option<&[collab_engine::archive_history::NativeArchiveInventory]>,
    states: usize,
) -> Result<
    Option<BTreeMap<&str, &collab_engine::archive_history::NativeArchiveInventory>>,
    ArchiveError,
> {
    let Some(reports) = reports else {
        return Ok(None);
    };
    if reports.len() != states || reports.len() > MAX_OBJECTS {
        return Err(ArchiveError::Invalid(
            "missing or extra native reports".into(),
        ));
    }
    let mut index = BTreeMap::new();
    for report in reports {
        if index.insert(report.binding.as_str(), report).is_some() {
            return Err(ArchiveError::Invalid("duplicate native reports".into()));
        }
    }
    Ok(Some(index))
}
fn same_inventory_semantics(
    source: &collab_engine::archive_history::NativeArchiveInventory,
    actual: &collab_engine::archive_history::NativeArchiveInventory,
) -> bool {
    fn references(
        report: &collab_engine::archive_history::NativeArchiveInventory,
    ) -> BTreeSet<(
        collab_engine::archive_history::NativeId,
        collab_engine::archive_history::NativeId,
        collab_engine::archive_history::ReferenceKind,
        collab_engine::archive_history::ReferenceCertainty,
        &str,
    )> {
        report
            .references
            .iter()
            .map(|r| {
                (
                    r.owner,
                    r.declaration,
                    r.kind,
                    r.certainty,
                    r.value.as_str(),
                )
            })
            .collect()
    }
    let unavailable = |report: &collab_engine::archive_history::NativeArchiveInventory| {
        report
            .unavailable
            .iter()
            .map(|r| (r.id, r.len, r.kind))
            .collect::<BTreeSet<_>>()
    };
    // Work is observational but bounded independently. Semantic duplicates are
    // rejected; source and destination parser iteration order is not an oracle.
    source.binding == actual.binding
        && source.schema_version == actual.schema_version
        && source.complete == actual.complete
        && source.diagnostics == actual.diagnostics
        && source.references.len() == actual.references.len()
        && source.unavailable.len() == actual.unavailable.len()
        && references(source).len() == source.references.len()
        && unavailable(source).len() == source.unavailable.len()
        && references(source) == references(actual)
        && unavailable(source) == unavailable(actual)
}

/// Uses the native store for every captured revision in fresh isolated
/// sessions. This never persists a forward-restore update or reseeds content.
pub async fn validate_native(
    mut archive: Archive,
    config: crate::collab::CollabConfig,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Archive, ArchiveError> {
    let cancelled = cancel.child_token();
    let _stop_on_drop = cancelled.clone().drop_guard();
    let stop = cancelled.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        archive.validate()?;
        let supplied = archive.graph.native_inventory.take();
        let supplied_index = inventory_index(supplied.as_deref(), archive.graph.states.len())?;
        let mut inventories = Vec::with_capacity(archive.graph.states.len());
        let mut inventory_bytes = serde_json::to_vec(&archive.graph)
            .map_err(|_| ArchiveError::Invalid("graph report budget".into()))?
            .len();
        for state in &archive.graph.states {
            if stop.is_cancelled() {
                return Err(ArchiveError::Cancelled);
            }
            let target = archive_target(&archive, state)?;
            let binding = native_capture_binding(&archive, &target);
            let report = inspect_native_target(
                &target,
                &binding,
                &config.engine_bin,
                config.limits,
                &stop,
                &mut |report| {
                    validate_native_inventory(&archive, report, &binding, &config.limits)?;
                    if let Some(index) = &supplied_index {
                        let source = index.get(binding.as_str()).ok_or_else(|| {
                            ArchiveError::Invalid("missing or rebound native report".into())
                        })?;
                        validate_native_inventory(&archive, source, &binding, &config.limits)?;
                        if !same_inventory_semantics(source, report) {
                            return Err(ArchiveError::Invalid(
                                "tampered native report semantics".into(),
                            ));
                        }
                    }
                    let report_bytes = collab_engine::archive_history::report_wire_bytes(
                        report,
                        (MAX_GRAPH_BYTES.saturating_sub(inventory_bytes)) as u64,
                    )
                    .ok_or(ArchiveError::Limit)?;
                    inventory_bytes = inventory_bytes
                        .checked_add(report_bytes as usize)
                        .and_then(|n| n.checked_add(1))
                        .ok_or(ArchiveError::Limit)?;
                    if inventory_bytes > MAX_GRAPH_BYTES {
                        return Err(ArchiveError::Limit);
                    }
                    Ok(())
                },
            )?;
            inventories.push(report);
        }
        // Preserve the recomputed, bounded source availability evidence. Existing
        // supplied evidence was compared before any graph remap/write.
        archive.graph.native_inventory = Some(inventories);
        archive.validate()?;
        Ok(archive)
    });
    tokio::select! {
        result = &mut worker => result.map_err(|_| ArchiveError::Worker)?,
        () = cancel.cancelled() => {
            cancelled.cancel();
            // The currently bounded call finishes on its owning thread; the
            // EngineSession then kills/reaps before this cancellation returns.
            let _ = worker.await;
            Err(ArchiveError::Cancelled)
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// Policy-only fixture: empty native bytes are not a history acceptance
    /// oracle. Native Load/Project/history tests must use independent writers.
    pub(crate) fn policy_fixture() -> Archive {
        let actor = "20000000-0000-4000-8000-000000000001";
        let project = "10000000-0000-4000-8000-000000000001";
        let doc = "10000000-0000-4000-8000-000000000002";
        let task = "10000000-0000-4000-8000-000000000003";
        let workflow = "10000000-0000-4000-8000-000000000004";
        let status = "10000000-0000-4000-8000-000000000005";
        let label = "10000000-0000-4000-8000-000000000009";
        let at = "2026-10-02T00:00:00Z";
        let body = json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"literal-anchor"},"content":[{"type":"text","text":"한글 🧪","marks":[{"type":"bold"}]}]}]});
        let state = |kind: &str, id: &str| json!({"target_kind":kind,"target_id":id,"state_entry":format!("native/{kind}/{id}/state.v1"),"encoding":1,"snapshot_cutoff_seq":0,"tail_seq":0,"compacted_at":null,"created_at":at,"updated_at":at,"updates":[],"receipts":[]});
        let mut graph = json!({
            "source_workspace_id":"30000000-0000-4000-8000-000000000001","source_actor_id":actor,"captured_at":at,
            "project":{"id":project,"key":"ARCH","name":"원본 프로젝트 🧪","description":null,"icon":null,"visibility":"private","root_document_id":doc,"status":"active","next_number":3,"created_by":actor,"created_at":at,"updated_at":at,"deleted_at":null},
            "workflows":[{"id":workflow,"project_id":project,"created_at":at,"updated_at":at}],
            "statuses":[{"id":status,"project_id":project,"workflow_id":workflow,"name":"진행 전","category":"backlog","sort_key":"a0","wip_limit":null,"created_at":at,"updated_at":at}],
            "documents":[{"id":doc,"title":"문서 🧪","icon":null,"path":"10000000000040008000000000000002","parent_id":null,"sort_key":"a0","project_id":project,"number":1,"status":"draft","schema_version":2,"text":"한글 🧪","chosung":"ㅎㄱ 🧪","version":1,"created_by":actor,"created_at":at,"updated_at":at,"deleted_at":null,"content_json":body,"kind":"doc"}],
            "tasks":[{"id":task,"project_id":project,"number":2,"title":"일반 태스크 🧪","type":"task","priority":"medium","status_id":status,"start_date":"2026-10-02","due_date":"2026-10-03","due_at":null,"estimate":null,"parent_id":null,"milestone_id":null,"recurrence":null,"sort_key":"a0","schema_version":2,"content_json":body,"version":1,"archived_at":null,"deleted_at":null,"created_by":actor,"created_at":at,"updated_at":at,"text":"한글 🧪","chosung":"ㅎㄱ 🧪"}],
            "assignees":[{"task_id":task,"user_id":actor}],"origins":[],
            "labels":[{"id":label,"project_id":project,"name":"검토 🧪","color":"blue","created_at":at,"updated_at":at}],
            "comments":[
                {"id":"10000000-0000-4000-8000-00000000000b","document_id":doc,"task_id":null,"parent_id":null,"created_by":actor,"body":"검토 댓글 🧪","chosung":"ㄱㅌ ㄷㄱ 🧪","resolved_at":at,"reactions":{"👍":[actor],"🎉":[]},"created_at":at,"updated_at":at},
                {"id":"10000000-0000-4000-8000-00000000000c","document_id":doc,"task_id":null,"parent_id":"10000000-0000-4000-8000-00000000000b","created_by":actor,"body":"답글","chosung":"ㄷㄱ","resolved_at":null,"reactions":{},"created_at":at,"updated_at":at},
                {"id":"10000000-0000-4000-8000-00000000000d","document_id":null,"task_id":task,"parent_id":null,"created_by":actor,"body":"확인","chosung":"ㅎㅇ","resolved_at":null,"reactions":{"❤️":[actor]},"created_at":at,"updated_at":at}],
            "task_labels":[{"task_id":task,"label_id":label}],
            "activity":[{"id":"10000000-0000-4000-8000-000000000006","task_id":task,"actor_user_id":actor,"channel":"web","kind":"created","changes":[],"created_at":at},
                {"id":"10000000-0000-4000-8000-00000000000a","task_id":task,"actor_user_id":actor,"channel":"web","kind":"changed","changes":[
                    {"field":"labelIds","from":[],"to":[{"id":label,"label":"검토 🧪"}]},
                    {"field":"assigneeIds","from":[],"to":[{"id":actor,"label":null}]},
                    {"field":"title","from":"이전 🧪","to":"일반 태스크 🧪"},
                    {"field":"statusId","from":{"id":status,"label":"진행 전"},"to":{"id":status,"label":null}}],"created_at":at}],
            "states":[state("document",doc),state("task",task)],"revisions":[],"attachments":[],
            "collections":[{"id":"10000000-0000-4000-8000-000000000007","project_id":project,"kind":"task","name":"원본 프로젝트 🧪","version":1,"deleted_at":null,"created_at":at,"updated_at":at}],
            "collection_items":[{"id":"10000000-0000-4000-8000-000000000008","collection_id":"10000000-0000-4000-8000-000000000007","document_id":null,"task_id":task,"version":1,"created_at":at,"updated_at":at}]
        });
        // Empty model arrays outside the literal (one json! of the whole graph
        // exceeds the macro recursion limit).
        for key in [
            "zotero_connectors",
            "zotero_references",
            "zotero_collections",
            "zotero_memberships",
            "zotero_links",
            "personal_input_commands",
            "time_entries",
            "timer_runs",
            "timer_segments",
            "timer_legacy_open",
            "timer_commands",
            "timer_audit",
            "milestones",
            "dependencies",
            "views",
            "document_tags",
            "document_tag_assignments",
            "purged_label_refs",
            "purged_milestone_refs",
            "collection_fields",
            "collection_options",
            "collection_values",
            "collection_choices",
            "collection_people",
            "collection_views",
        ] {
            graph[key] = json!([]);
        }
        let graph = serde_json::from_value(graph).unwrap();
        let entries = [
            (format!("native/document/{doc}/state.v1"), encode(&[0, 0])),
            (format!("native/task/{task}/state.v1"), encode(&[0, 0])),
        ]
        .into();
        Archive { graph, entries }
    }

    #[test]
    fn native_archive_policy_keeps_mandatory_ordinary_creation_activity() {
        let archive = policy_fixture();
        archive.validate().unwrap();
        assert_eq!(
            archive.graph.activity[0].id.to_string(),
            "10000000-0000-4000-8000-000000000006"
        );
        assert_eq!(
            archive.graph.documents[0].content_json["content"][0]["content"][0]["text"],
            "한글 🧪"
        );
    }

    #[test]
    fn native_archive_policy_refuses_missing_state_activity_and_outside_references() {
        let mut archive = policy_fixture();
        archive.graph.states.pop();
        assert!(archive.validate().is_err());
        let mut archive = policy_fixture();
        archive.graph.activity.clear();
        assert!(archive.validate().is_err());
        let mut archive = policy_fixture();
        archive.graph.documents[0].content_json = json!({"type":"doc","content":[{"type":"mention","attrs":{"entity":"task","id":"ffffffff-ffff-4fff-8fff-ffffffffffff","label":"외부"}}]});
        archive.graph.documents[0].text = "외부".into();
        archive.graph.documents[0].chosung = "ㅇㅂ".into();
        assert!(matches!(
            archive.validate(),
            Err(ArchiveError::Unsupported(_))
        ));
        let mut archive = policy_fixture();
        archive
            .entries
            .insert("unlisted/secret".into(), encode(b"private"));
        assert!(archive.validate().is_err());
    }

    #[test]
    fn native_archive_policy_body_without_native_state_travels_body_only() {
        // A task never opened natively has no state row. Only the empty
        // document body may travel stateless (the product creates the row on
        // first open only for it); any other stateless body is refused.
        let mut archive = policy_fixture();
        let state = archive.graph.states.pop().unwrap();
        assert_eq!(state.target_kind, "task");
        // The non-empty stateless body is refused (first, with its typed
        // reason), whether or not the removed state's entry is still there.
        let stateless = |archive: &Archive| {
            matches!(
                archive.validate(),
                Err(ArchiveError::Unsupported(m)) if m == "missing native state for captured body"
            )
        };
        assert!(stateless(&archive));
        let mut without_entry = archive.clone();
        without_entry.entries.remove(&state.state_entry);
        assert!(stateless(&without_entry));
        // With the empty body, a dangling entry of the removed state is still
        // refused by the entry allowlist; without it the body travels alone.
        archive.graph.tasks[0].content_json = crate::db::documents::empty_document_json();
        archive.graph.tasks[0].text = String::new();
        archive.graph.tasks[0].chosung = String::new();
        assert!(matches!(archive.validate(), Err(ArchiveError::Invalid(_))));
        archive.entries.remove(&state.state_entry);
        archive.validate().unwrap();
        // History needs its state: a revision on the stateless task is refused.
        let task = archive.graph.tasks[0].id;
        let id = Uuid::from_u128(0x7100_0000_0000_4000_8000_0000_0000_0001);
        let revision: Revision = serde_json::from_value(json!({"id":id,"target_kind":"task",
            "target_id":task,"snapshot_entry":format!("revisions/{id}.snapshot.v1"),"encoding":1,
            "content_json":archive.graph.tasks[0].content_json,"text":archive.graph.tasks[0].text,
            "reason":"manual","created_by":archive.graph.source_actor_id,"created_at":"2026-10-02T00:00:00Z"}))
        .unwrap();
        archive
            .entries
            .insert(revision.snapshot_entry.clone(), encode(&[0, 0]));
        archive.graph.revisions.push(revision);
        assert!(matches!(archive.validate(), Err(ArchiveError::Invalid(_))));
    }

    #[test]
    fn native_archive_policy_labels_and_changed_activity_stay_in_closure() {
        policy_fixture().validate().unwrap();
        let invalid = |edit: fn(&mut Archive)| {
            let mut archive = policy_fixture();
            edit(&mut archive);
            matches!(archive.validate(), Err(ArchiveError::Invalid(_)))
        };
        let unsupported = |edit: fn(&mut Archive)| {
            let mut archive = policy_fixture();
            edit(&mut archive);
            matches!(archive.validate(), Err(ArchiveError::Unsupported(_)))
        };
        assert!(invalid(|a| a.graph.labels[0].project_id = Uuid::nil()));
        assert!(invalid(|a| a.graph.labels[0].color = "black".into()));
        assert!(invalid(|a| a.graph.labels[0].name = "  ".into()));
        assert!(invalid(|a| a.graph.task_labels[0].label_id = Uuid::nil()));
        assert!(invalid(|a| {
            let row = a.graph.task_labels[0].clone();
            a.graph.task_labels.push(row)
        }));
        // Label IDs share the global content-ID space.
        assert!(invalid(|a| a.graph.labels[0].id = a.graph.tasks[0].id));
        assert!(unsupported(
            |a| a.graph.activity[1].changes[0]["to"] = json!([{"id": Uuid::nil(), "label": null}])
        ));
        assert!(unsupported(
            |a| a.graph.activity[1].changes[1]["to"] =
                json!([{"id": "ffffffff-ffff-4fff-8fff-ffffffffffff", "label": null}])
        ));
        // The stored form is a reference object, never a bare UUID.
        assert!(unsupported(|a| {
            let actor = a.graph.source_actor_id.to_string();
            a.graph.activity[1].changes[1]["to"] = json!([actor])
        }));
        assert!(unsupported(
            |a| a.graph.activity[1].changes[1]["to"][0]["label"] = json!("이름")
        ));
        assert!(unsupported(
            |a| a.graph.activity[1].changes[3]["to"]["extra"] = json!(1)
        ));
        assert!(unsupported(|a| {
            let project = a.graph.project.id;
            a.graph.activity[1].changes[3]["to"] = json!({"id": project, "label": null})
        }));
        assert!(unsupported(|a| {
            let status = a.graph.statuses[0].id.to_string();
            a.graph.activity[1].changes[3]["to"] = json!(status)
        }));
        assert!(unsupported(
            |a| a.graph.activity[1].changes[2]["field"] = json!("unknownField")
        ));
        assert!(unsupported(
            |a| a.graph.activity[1].changes[3]["field"] = json!("title")
        ));
        assert!(unsupported(
            |a| a.graph.activity[1].changes[2]["extra"] = json!(1)
        ));
        assert!(unsupported(
            |a| a.graph.activity[1].changes[2]["to"] = json!("")
        ));
        assert!(unsupported(|a| a.graph.activity[1].changes = json!([])));
        for changes in [
            json!([{"field":"milestoneId","from":null,"to":{"id":"10000000-0000-4000-8000-000000000001","label":"m"}}]),
            json!([{"field":"recurrence","from":null,"to":"yearly"}]),
            json!([{"field":"recurrence","from":null,"to":{"kind":"weekly"}}]),
            json!([{"field":"parentId","from":null,"to":{"id":"10000000-0000-4000-8000-000000000003","label":"제목"}}]),
            json!([{"field":"dueAt","from":null,"to":"tomorrow"}]),
            json!([{"field":"archived","from":false,"to":"yes"}]),
            json!([{"field":"estimate","from":null,"to":"1.2.3"}]),
            json!([{"field":"startDate","from":null,"to":"2026-13-40"}]),
            json!([{"field":"priority","from":"low","to":"urgent!"}]),
        ] {
            let mut archive = policy_fixture();
            archive.graph.activity[1].changes = changes.clone();
            assert!(
                matches!(archive.validate(), Err(ArchiveError::Unsupported(_))),
                "{changes}"
            );
        }
        assert!(unsupported(|a| a.graph.activity[1].kind = "deleted".into()));
        // Every scalar field in its stored snapshot form is portable.
        let mut archive = policy_fixture();
        archive.graph.activity[1].changes = json!([
            {"field":"title","from":"이전","to":"일반 태스크 🧪"},
            {"field":"type","from":"task","to":"task"},
            {"field":"priority","from":"low","to":"medium"},
            {"field":"startDate","from":null,"to":"2026-10-02"},
            {"field":"dueDate","from":"2026-10-04","to":null},
            {"field":"dueAt","from":null,"to":"2026-10-03T00:00:00.000Z"},
            {"field":"estimate","from":null,"to":"1.5"},
            {"field":"archived","from":false,"to":true},
            {"field":"parentId","from":null,"to":null},
            {"field":"milestoneId","from":null,"to":null},
            {"field":"recurrence","from":null,"to":null}
        ]);
        archive.validate().unwrap();
        // A recurrence change records the preset kinds.
        archive.graph.activity[1].changes =
            json!([{"field":"recurrence","from":"daily","to":"monthly"}]);
        archive.validate().unwrap();
        // Restore remaps only assignee identities to the destination actor.
        let archive = policy_fixture();
        let destination = Uuid::parse_str("40000000-0000-4000-8000-000000000001").unwrap();
        let mapped = mapped_activity_changes(
            &archive.graph.activity[1].changes,
            archive.graph.source_actor_id,
            destination,
        );
        assert_eq!(
            mapped[1]["to"],
            json!([{"id": destination.to_string(), "label": null}])
        );
        assert_eq!(mapped[0], archive.graph.activity[1].changes[0]);
        assert_eq!(mapped[3], archive.graph.activity[1].changes[3]);
    }

    #[test]
    fn native_archive_policy_changed_activity_refuses_duplicate_references() {
        // The product records assignee/label sets from PK-backed rows, so no
        // side of a change repeats an identity. A repeat - including another
        // textual spelling of the same UUID - is refused, on each of the four
        // list sides; unique references stay valid.
        policy_fixture().validate().unwrap();
        let archive = policy_fixture();
        let actor = archive.graph.source_actor_id;
        let label = archive.graph.labels[0].id;
        for (index, side, id, name) in [
            (1usize, "from", actor, None),
            (1, "to", actor, None),
            (0, "from", label, Some("검토 🧪")),
            (0, "to", label, Some("검토 🧪")),
        ] {
            // The same identity under a distinct textual spelling (simple form).
            let alias = id.simple().to_string();
            assert_ne!(alias, id.to_string());
            assert_eq!(Uuid::parse_str(&alias).unwrap(), id);
            for second in [id.to_string(), id.to_string().to_uppercase(), alias] {
                let mut archive = policy_fixture();
                archive.graph.activity[1].changes[index][side] = json!([
                    {"id": id.to_string(), "label": name},
                    {"id": second, "label": name}
                ]);
                assert!(
                    matches!(archive.validate(), Err(ArchiveError::Unsupported(_))),
                    "duplicate {side} of change {index} accepted"
                );
            }
            let mut archive = policy_fixture();
            archive.graph.activity[1].changes[index][side] =
                json!([{"id": id.to_string(), "label": name}]);
            archive.validate().unwrap();
        }
    }

    #[test]
    fn native_archive_policy_comments_stay_in_closure_and_remap_reactions() {
        policy_fixture().validate().unwrap();
        let invalid = |edit: fn(&mut Archive)| {
            let mut archive = policy_fixture();
            edit(&mut archive);
            matches!(archive.validate(), Err(ArchiveError::Invalid(_)))
        };
        assert!(invalid(|a| a.graph.comments[0].created_by = Uuid::nil()));
        assert!(invalid(
            |a| a.graph.comments[0].body = " 검토 댓글 🧪".into()
        ));
        assert!(invalid(|a| a.graph.comments[0].chosung = "x".into()));
        assert!(invalid(|a| a.graph.comments[0].body = "x".repeat(8001)));
        assert!(invalid(
            |a| a.graph.comments[1].resolved_at = Some("2026-10-02T00:00:00Z".into())
        ));
        assert!(invalid(
            |a| a.graph.comments[0].document_id = Some(Uuid::nil())
        ));
        assert!(invalid(
            |a| a.graph.comments[0].task_id = Some(a.graph.tasks[0].id)
        ));
        assert!(invalid(|a| {
            // A reply must stay on its parent's target.
            a.graph.comments[1].document_id = None;
            a.graph.comments[1].task_id = Some(a.graph.tasks[0].id);
        }));
        assert!(invalid(
            |a| a.graph.comments[1].parent_id = Some(Uuid::nil())
        ));
        assert!(invalid(|a| {
            // A two-comment reply cycle (no resolution involved).
            let reply = a.graph.comments[1].id;
            a.graph.comments[0].parent_id = Some(reply);
            a.graph.comments[0].resolved_at = None;
        }));
        assert!(invalid(|a| a.graph.comments[0].reactions =
            json!({"👍":["ffffffff-ffff-4fff-8fff-ffffffffffff"]})));
        assert!(invalid(|a| a.graph.comments[0].reactions = json!({"😀":[]})));
        assert!(invalid(|a| a.graph.comments[0].reactions = json!([])));
        assert!(invalid(|a| a.graph.comments[0].id = a.graph.labels[0].id));
        assert!(invalid(
            |a| a.graph.comments[1].parent_id = Some(a.graph.comments[1].id)
        ));
        // Scale (writers allow any reply depth; capture caps comments at
        // 10000): a 10000-deep chain listed deepest first validates and is
        // ordered parents first; the same chain closed into a cycle, and a
        // flat 10000-comment set, behave as before.
        let chain = |n: u128, reversed: bool, cycle: bool| {
            let mut archive = policy_fixture();
            let template = archive.graph.comments[0].clone();
            let id = |i: u128| Uuid::from_u128(0x7700_0000_0000_4000_8000_0000_0000_0000 + i);
            let mut comments: Vec<Comment> = (0..n)
                .map(|i| {
                    let mut c = template.clone();
                    c.id = id(i);
                    c.parent_id = (i > 0).then(|| id(i - 1));
                    if i > 0 {
                        c.resolved_at = None;
                    }
                    c
                })
                .collect();
            if cycle {
                comments[0].parent_id = Some(id(n - 1));
                comments[0].resolved_at = None;
            }
            if reversed {
                comments.reverse();
            }
            archive.graph.comments = comments;
            archive
        };
        // The shared helper refuses a repeated id itself (two roots, or a
        // root and a reply sharing an id), besides the global id check.
        let mut duplicate_roots = policy_fixture().graph.comments;
        duplicate_roots.truncate(1);
        duplicate_roots[0].parent_id = None;
        duplicate_roots.push(duplicate_roots[0].clone());
        assert!(matches!(
            comments_parent_first(&duplicate_roots),
            Err(ArchiveError::Invalid(_))
        ));
        let mut duplicate_child = policy_fixture().graph.comments;
        let reply = duplicate_child[1].clone();
        duplicate_child.push(reply);
        assert!(matches!(
            comments_parent_first(&duplicate_child),
            Err(ArchiveError::Invalid(_))
        ));
        comments_parent_first(&policy_fixture().graph.comments).unwrap();
        let deep = chain(10_000, true, false);
        deep.validate().unwrap();
        let order = comments_parent_first(&deep.graph.comments).unwrap();
        let mut seen = BTreeSet::new();
        for c in &order {
            assert!(c.parent_id.is_none_or(|p| seen.contains(&p)));
            seen.insert(c.id);
        }
        assert_eq!(order.len(), 10_000);
        assert!(matches!(
            chain(10_000, true, true).validate(),
            Err(ArchiveError::Invalid(_))
        ));
        let mut flat = chain(10_000, false, false);
        for c in &mut flat.graph.comments {
            c.parent_id = None;
        }
        flat.validate().unwrap();
        assert_eq!(
            comments_parent_first(&flat.graph.comments)
                .unwrap()
                .iter()
                .map(|c| c.id)
                .collect::<Vec<_>>(),
            flat.graph.comments.iter().map(|c| c.id).collect::<Vec<_>>()
        );
        // Cost witness on the same corpus (recorded, never a time gate): the
        // previous chain walk and rescanning publish order against the one
        // ordering pass.
        let comments = &deep.graph.comments;
        let started = std::time::Instant::now();
        let index: BTreeMap<Uuid, &Comment> = comments.iter().map(|c| (c.id, c)).collect();
        let mut walked = 0u64;
        for c in comments {
            let mut cursor = c.parent_id;
            while let Some(p) = cursor {
                walked += 1;
                cursor = index.get(&p).and_then(|parent| parent.parent_id);
            }
        }
        let previous_walk = started.elapsed();
        let started = std::time::Instant::now();
        let mut remaining: Vec<&Comment> = comments.iter().collect();
        let mut inserted = BTreeSet::new();
        let mut scanned = 0u64;
        while !remaining.is_empty() {
            let at = remaining
                .iter()
                .position(|c| {
                    scanned += 1;
                    c.parent_id.is_none_or(|id| inserted.contains(&id))
                })
                .unwrap();
            inserted.insert(remaining.remove(at).id);
        }
        let previous_publish = started.elapsed();
        let started = std::time::Instant::now();
        let _ = comments_parent_first(comments).unwrap();
        let ordering = started.elapsed();
        println!(
            "W7-COMMENT-COST {}",
            json!({"comments": comments.len(), "depth": comments.len(),
                "previousWalkSteps": walked, "previousWalkMs": previous_walk.as_secs_f64() * 1e3,
                "previousPublishChecks": scanned, "previousPublishMs": previous_publish.as_secs_f64() * 1e3,
                "orderingVisits": comments.len(), "orderingMs": ordering.as_secs_f64() * 1e3})
        );
        let archive = policy_fixture();
        let destination = Uuid::parse_str("40000000-0000-4000-8000-000000000001").unwrap();
        assert_eq!(
            mapped_comment_reactions(
                &archive.graph.comments[0].reactions,
                archive.graph.source_actor_id,
                destination
            ),
            json!({"👍":[destination.to_string()],"🎉":[]})
        );
    }

    /// Policy fixture plus a selected Zotero closure: one user-library
    /// connector, a backing wiki document under a wiki ancestor, a purged
    /// reference (NULL backing), nested and historical collections,
    /// memberships and links to a project document and task.
    pub(crate) fn zotero_fixture() -> Archive {
        let mut archive = policy_fixture();
        let at = "2026-10-02T00:00:00Z";
        let actor = archive.graph.source_actor_id;
        let connector = "50000000-0000-4000-8000-000000000001";
        let ancestor = "50000000-0000-4000-8000-000000000002";
        let backing = "50000000-0000-4000-8000-000000000003";
        let purged = "50000000-0000-4000-8000-000000000004";
        let template = serde_json::to_value(&archive.graph.documents[0]).unwrap();
        let wiki = |id: &str, parent: Option<&str>, path: String, number: i32| {
            let mut d = template.clone();
            d["id"] = json!(id);
            d["parent_id"] = json!(parent);
            d["path"] = json!(path);
            d["project_id"] = Value::Null;
            d["number"] = json!(number);
            d["title"] = json!("Zotero reference");
            d["created_by"] = json!(actor);
            serde_json::from_value::<Document>(d).unwrap()
        };
        let label = |id: &str| Uuid::parse_str(id).unwrap().simple().to_string();
        archive
            .graph
            .documents
            .push(wiki(ancestor, None, label(ancestor), 1));
        archive.graph.documents.push(wiki(
            backing,
            Some(ancestor),
            format!("{}.{}", label(ancestor), label(backing)),
            2,
        ));
        for id in [ancestor, backing] {
            let mut state = archive.graph.states[0].clone();
            state.target_id = Uuid::parse_str(id).unwrap();
            state.state_entry = format!("native/document/{id}/state.v1");
            archive
                .entries
                .insert(state.state_entry.clone(), encode(&[0, 0]));
            archive.graph.states.push(state);
        }
        let doc = archive.graph.documents[0].id;
        let task = archive.graph.tasks[0].id;
        let records = json!({
            "zotero_connectors":[{"id":connector,"library_type":"user","remote_library_id":42,
                "library_url":"https://www.zotero.org/users/42","completed_version":7,"created_at":at,"updated_at":at}],
            "zotero_references":[
                {"id":backing,"connector_id":connector,"document_id":backing,"item_key":"ABCD2345",
                 "remote_version":3,"local_version":2,
                 "bibliography":{"itemType":"book","title":"합성 연구 자료 🧪","fields":{"date":"Spring 2026","ISBN":"9780000000000"},
                    "creators":[{"creatorType":"author","firstName":"민","lastName":"김"},{"creatorType":"editor","name":"Synthetic Research Group"}],
                    "tags":[{"tag":"연구","type":0}],"relations":{}},
                 "return_url":"https://www.zotero.org/users/42/items/ABCD2345","availability":"available"},
                {"id":purged,"connector_id":connector,"document_id":null,"item_key":"EFGH4567",
                 "remote_version":5,"local_version":1,
                 "bibliography":{"itemType":"case","title":"","fields":{"caseName":"보관 판례"},"creators":[],"tags":[]},
                 "return_url":"https://www.zotero.org/users/42/items/EFGH4567","availability":"deleted"}],
            "zotero_collections":[
                {"connector_id":connector,"collection_key":"PARENT23","remote_version":2,"name":"상위 🧪","parent_key":null,"availability":"available"},
                {"connector_id":connector,"collection_key":"CHILD234","remote_version":3,"name":"하위","parent_key":"PARENT23","availability":"available"},
                {"connector_id":connector,"collection_key":"DELETED2","remote_version":1,"name":"Historical","parent_key":null,"availability":"deleted"}],
            "zotero_memberships":[
                {"connector_id":connector,"reference_id":backing,"collection_key":"CHILD234"},
                {"connector_id":connector,"reference_id":purged,"collection_key":"DELETED2"}],
            "zotero_links":[
                {"id":"50000000-0000-4000-8000-000000000005","connector_id":connector,"reference_id":backing,"document_id":doc,"task_id":null,"anchor":""},
                {"id":"50000000-0000-4000-8000-000000000006","connector_id":connector,"reference_id":backing,"document_id":null,"task_id":task,"anchor":"literal-anchor"}]
        });
        archive.graph.zotero_connectors =
            serde_json::from_value(records["zotero_connectors"].clone()).unwrap();
        archive.graph.zotero_references =
            serde_json::from_value(records["zotero_references"].clone()).unwrap();
        archive.graph.zotero_collections =
            serde_json::from_value(records["zotero_collections"].clone()).unwrap();
        archive.graph.zotero_memberships =
            serde_json::from_value(records["zotero_memberships"].clone()).unwrap();
        archive.graph.zotero_links =
            serde_json::from_value(records["zotero_links"].clone()).unwrap();
        archive
    }

    #[test]
    fn native_archive_policy_zotero_closure_validates_with_w6_policy() {
        zotero_fixture().validate().unwrap();
        // Canonical connector columns only: no credential or sync state field.
        let connector = serde_json::to_value(&zotero_fixture().graph.zotero_connectors[0]).unwrap();
        let mut keys: Vec<_> = connector.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "completed_version",
                "created_at",
                "id",
                "library_type",
                "library_url",
                "remote_library_id",
                "updated_at"
            ]
        );
        let text = serde_json::to_string(&zotero_fixture().graph).unwrap();
        for field in [
            "sealed_key",
            "credential",
            "api_key",
            "generation",
            "progress_version",
            "sync_id",
            "retry_at",
            "owner_user_id",
        ] {
            assert!(!text.contains(field), "{field}");
        }
        let fails = |edit: fn(&mut Archive)| {
            let mut archive = zotero_fixture();
            edit(&mut archive);
            archive.validate().err()
        };
        let invalid =
            |edit: fn(&mut Archive)| matches!(fails(edit), Some(ArchiveError::Invalid(_)));
        let outside = |edit: fn(&mut Archive)| matches!(fails(edit), Some(ArchiveError::Unsupported(m)) if m == "zotero closure");
        let wiki_closure = |edit: fn(&mut Archive)| matches!(fails(edit), Some(ArchiveError::Unsupported(m)) if m == "wiki closure");
        // W6 library descriptor and saved return URL policy.
        assert!(invalid(
            |a| a.graph.zotero_connectors[0].library_url = "https://www.zotero.org/users/43".into()
        ));
        assert!(invalid(|a| a.graph.zotero_connectors[0].library_url =
            "https://www.zotero.org/users/42/".into()));
        assert!(invalid(
            |a| a.graph.zotero_connectors[0].library_type = "team".into()
        ));
        assert!(invalid(
            |a| a.graph.zotero_connectors[0].completed_version = -1
        ));
        assert!(invalid(|a| {
            let mut twin = a.graph.zotero_connectors[0].clone();
            twin.id = Uuid::parse_str("50000000-0000-4000-8000-000000000009").unwrap();
            a.graph.zotero_connectors.push(twin);
        }));
        assert!(invalid(|a| a.graph.zotero_references[0].return_url =
            "https://www.zotero.org/users/42/items/EFGH4567".into()));
        assert!(invalid(|a| a.graph.zotero_references[0].return_url =
            "https://www.zotero.org/users/42/items/ABCD2345?key=x".into()));
        // W6 stored bibliography schema.
        assert!(invalid(
            |a| a.graph.zotero_references[0].bibliography["itemType"] = json!("note")
        ));
        assert!(invalid(
            |a| a.graph.zotero_references[0].bibliography["unexpected"] = json!("x")
        ));
        assert!(invalid(
            |a| a.graph.zotero_references[0].bibliography["fields"]["title"] = json!("x")
        ));
        // Keys, enums, versions and identities.
        assert!(invalid(
            |a| a.graph.zotero_references[0].item_key = "abcd2345".into()
        ));
        assert!(invalid(|a| {
            // A second reference of the same connector item.
            a.graph.zotero_references[1].item_key = "ABCD2345".into();
            a.graph.zotero_references[1].return_url =
                "https://www.zotero.org/users/42/items/ABCD2345".into();
        }));
        assert!(invalid(
            |a| a.graph.zotero_references[0].availability = "gone".into()
        ));
        assert!(invalid(|a| a.graph.zotero_references[0].local_version = 0));
        assert!(invalid(|a| a.graph.zotero_references[0].remote_version = -1));
        assert!(invalid(
            |a| a.graph.zotero_references[1].id = a.graph.zotero_references[0].id
        ));
        assert!(invalid(|a| {
            // The backing document is the reference's own identity.
            a.graph.zotero_references[0].document_id = Some(a.graph.documents[1].id);
        }));
        assert!(invalid(
            |a| a.graph.documents[2].number = a.graph.documents[1].number
        ));
        assert!(invalid(
            |a| a.graph.zotero_collections[0].parent_key = Some("CHILD234".into())
        ));
        assert!(invalid(
            |a| a.graph.zotero_collections[1].parent_key = Some("MISSING2".into())
        ));
        assert!(invalid(
            |a| a.graph.zotero_collections[0].parent_key = Some("PARENT23".into())
        ));
        assert!(invalid(
            |a| a.graph.zotero_collections[2].collection_key = "PARENT23".into()
        ));
        assert!(invalid(
            |a| a.graph.zotero_collections[0].availability = "trashed".into()
        ));
        assert!(invalid(
            |a| a.graph.zotero_collections[0].name = "x".repeat(4097)
        ));
        assert!(invalid(|a| {
            let twin = a.graph.zotero_memberships[0].clone();
            a.graph.zotero_memberships.push(twin);
        }));
        assert!(invalid(
            |a| a.graph.zotero_links[0].anchor = "가".repeat(257)
        ));
        assert!(invalid(
            |a| a.graph.zotero_links[1].anchor = "literal\0anchor".into()
        ));
        assert!(invalid(|a| a.graph.zotero_links[0].anchor = "\0".into()));
        assert!(invalid(
            |a| a.graph.zotero_links[0].task_id = Some(a.graph.tasks[0].id)
        ));
        assert!(invalid(|a| {
            a.graph.zotero_links[1].id =
                Uuid::parse_str("50000000-0000-4000-8000-00000000000a").unwrap();
            a.graph.zotero_links[1].document_id = a.graph.zotero_links[0].document_id;
            a.graph.zotero_links[1].task_id = None;
            a.graph.zotero_links[1].anchor = String::new();
        }));
        assert!(invalid(
            |a| a.graph.zotero_links[1].id = a.graph.zotero_connectors[0].id
        ));
        // A wiki document under a project document (or vice versa) is refused.
        assert!(invalid(
            |a| a.graph.documents[1].parent_id = Some(a.graph.documents[0].id)
        ));
        // Closure: every target inside the archive, and wiki documents exactly
        // the backing documents and their ancestors.
        assert!(outside(
            |a| a.graph.zotero_references[0].connector_id = Uuid::nil()
        ));
        assert!(outside(
            |a| a.graph.zotero_links[0].document_id = Some(Uuid::nil())
        ));
        assert!(outside(
            |a| a.graph.zotero_links[1].task_id = Some(Uuid::nil())
        ));
        assert!(outside(
            |a| a.graph.zotero_links[0].reference_id = Uuid::nil()
        ));
        assert!(outside(
            |a| a.graph.zotero_memberships[0].collection_key = "MISSING2".into()
        ));
        assert!(outside(
            |a| a.graph.zotero_memberships[0].reference_id = Uuid::nil()
        ));
        assert!(outside(
            |a| a.graph.zotero_collections[0].connector_id = Uuid::nil()
        ));
        assert!(wiki_closure(|a| {
            // A wiki document neither backing a reference nor its ancestor.
            let mut extra = a.graph.documents[1].clone();
            extra.id = Uuid::parse_str("50000000-0000-4000-8000-000000000007").unwrap();
            extra.path = extra.id.simple().to_string();
            extra.number = 9;
            a.graph.documents.push(extra);
        }));
        assert!(outside(|a| {
            // A reference whose backing document is not carried.
            a.graph.documents.remove(2);
        }));
        assert!(wiki_closure(|a| {
            // A plain project archive cannot carry an unselected wiki document.
            a.graph.zotero_references[0].document_id = None;
        }));
        assert!(matches!(
            fails(|a| {
                for n in 0..8 {
                    let mut c = a.graph.zotero_connectors[0].clone();
                    c.id = Uuid::from_u128(0x6000_0000_0000_4000_8000_0000_0000_0000 + n);
                    c.remote_library_id = 100 + n as i64;
                    c.library_url = format!("https://www.zotero.org/users/{}", 100 + n);
                    a.graph.zotero_connectors.push(c);
                }
            }),
            Some(ArchiveError::Limit)
        ));
        // Removing the whole selection leaves the ordinary project archive.
        let mut archive = zotero_fixture();
        archive.graph.zotero_links.clear();
        archive.graph.zotero_memberships.clear();
        archive.graph.zotero_collections.clear();
        archive.graph.zotero_references.clear();
        archive.graph.zotero_connectors.clear();
        assert!(matches!(
            archive.validate(),
            Err(ArchiveError::Unsupported(m)) if m == "wiki closure"
        ));
    }

    /// Policy fixture plus a personal-input capture: the task's origin wiki
    /// document (outside the project), its task_origins row and the retired
    /// capture receipt whose targets are all inside the archive.
    fn origin_fixture() -> Archive {
        let mut archive = policy_fixture();
        let origin = Uuid::parse_str("60000000-0000-4000-8000-000000000001").unwrap();
        let request = Uuid::parse_str("60000000-0000-4000-8000-000000000002").unwrap();
        let mut d = archive.graph.documents[0].clone();
        d.id = origin;
        d.project_id = None;
        d.parent_id = None;
        d.path = origin.simple().to_string();
        d.number = 1;
        d.title = "개인 입력 🧪".into();
        archive.graph.documents.push(d);
        let mut state = archive.graph.states[0].clone();
        state.target_id = origin;
        state.state_entry = format!("native/document/{origin}/state.v1");
        archive
            .entries
            .insert(state.state_entry.clone(), encode(&[0, 0]));
        archive.graph.states.push(state);
        let task = archive.graph.tasks[0].id;
        let project = archive.graph.project.id;
        archive.graph.origins =
            serde_json::from_value(json!([{"task_id":task,"document_id":origin,
            "request_id":request,"request_hash":"a".repeat(64),"anchor":null,
            "created_at":"2026-10-02T00:00:00Z","updated_at":"2026-10-02T00:00:00Z"}]))
            .unwrap();
        archive.graph.personal_input_commands =
            serde_json::from_value(json!([{"request_id":request,
            "request_hash":"0123456789abcdef".repeat(4),"intent":"task","document_id":origin,
            "task_id":task,"project_id":project,"created_at":"2026-10-02T00:00:00Z"}]))
            .unwrap();
        archive
    }

    #[test]
    fn native_archive_policy_origin_wiki_and_retired_receipts_stay_in_closure() {
        origin_fixture().validate().unwrap();
        let fails = |edit: fn(&mut Archive)| {
            let mut archive = origin_fixture();
            edit(&mut archive);
            archive.validate().err()
        };
        let invalid =
            |edit: fn(&mut Archive)| matches!(fails(edit), Some(ArchiveError::Invalid(_)));
        let receipt = |edit: fn(&mut Archive)| matches!(fails(edit), Some(ArchiveError::Unsupported(m)) if m == "personal input commands");
        let wiki = |edit: fn(&mut Archive)| matches!(fails(edit), Some(ArchiveError::Unsupported(m)) if m == "wiki closure");
        // Receipt shape: the stored SHA-256 hex, intent and its target rule.
        assert!(invalid(
            |a| a.graph.personal_input_commands[0].request_hash = "A".repeat(64)
        ));
        assert!(invalid(
            |a| a.graph.personal_input_commands[0].request_hash = "a".repeat(63)
        ));
        assert!(invalid(
            |a| a.graph.personal_input_commands[0].intent = "upload".into()
        ));
        assert!(invalid(
            |a| a.graph.personal_input_commands[0].intent = "note".into()
        ));
        assert!(invalid(
            |a| a.graph.personal_input_commands[0].created_at = "now".into()
        ));
        assert!(invalid(|a| {
            let twin = a.graph.personal_input_commands[0].clone();
            a.graph.personal_input_commands.push(twin);
        }));
        // Every recorded target must be inside the archive.
        assert!(receipt(
            |a| a.graph.personal_input_commands[0].document_id = Some(Uuid::nil())
        ));
        assert!(receipt(
            |a| a.graph.personal_input_commands[0].task_id = Some(Uuid::nil())
        ));
        assert!(receipt(
            |a| a.graph.personal_input_commands[0].project_id = Some(Uuid::nil())
        ));
        assert!(receipt(|a| {
            let r = &mut a.graph.personal_input_commands[0];
            (r.document_id, r.task_id, r.project_id) = (None, None, None);
        }));
        // The origin wiki document travels only as that task's origin.
        assert!(wiki(|a| a.graph.origins.clear()));
        assert!(wiki(|a| {
            let mut extra = a.graph.documents[1].clone();
            extra.id = Uuid::parse_str("60000000-0000-4000-8000-000000000003").unwrap();
            extra.path = extra.id.simple().to_string();
            extra.number = 2;
            a.graph.documents.push(extra);
        }));
        assert!(wiki(|a| a.graph.origins[0].document_id = Uuid::nil()));
        // A receipt is optional bookkeeping: none is still a valid archive.
        let mut archive = origin_fixture();
        archive.graph.personal_input_commands.clear();
        archive.validate().unwrap();
    }

    #[test]
    fn native_archive_policy_current_estimate_unit_and_restore_revision_metadata() {
        // 049: absent and SQL NULL decode alike; only an integral 'minutes'
        // estimate carries a unit; the exact decimal text travels.
        let mut task = serde_json::to_value(&policy_fixture().graph.tasks[0]).unwrap();
        assert!(task.get("estimate_unit").is_none());
        task["estimate_unit"] = Value::Null;
        let decoded: Task = serde_json::from_value(task.clone()).unwrap();
        assert_eq!(decoded.estimate_unit, None);
        task["estimate_unit_typo"] = json!("minutes");
        assert!(serde_json::from_value::<Task>(task).is_err());
        let estimate = |value: Option<&str>, unit: Option<&str>| {
            let mut archive = policy_fixture();
            archive.graph.tasks[0].estimate = value.map(|v| json!(v));
            archive.graph.tasks[0].estimate_unit = unit.map(str::to_owned);
            archive.validate()
        };
        for (value, unit) in [
            (Some("90"), Some("minutes")),
            (Some("0"), Some("minutes")),
            (Some("2147483647"), Some("minutes")),
            (Some("90.000"), Some("minutes")),
            (Some("123456789012.123456"), None),
            (None, None),
        ] {
            estimate(value, unit).unwrap();
        }
        for (value, unit) in [
            (Some("1.5"), Some("minutes")),
            (Some("2147483648"), Some("minutes")),
            (None, Some("minutes")),
            (Some("90"), Some("hours")),
            (Some("90"), Some("")),
        ] {
            assert!(
                matches!(estimate(value, unit), Err(ArchiveError::Invalid(_))),
                "{value:?} {unit:?}"
            );
        }
        let mut archive = policy_fixture();
        archive.graph.tasks[0].estimate = Some(json!(90));
        assert!(matches!(archive.validate(), Err(ArchiveError::Invalid(_))));
        let mut minutes = policy_fixture().graph.tasks[0].clone();
        minutes.estimate = Some(json!("90"));
        minutes.estimate_unit = Some("minutes".into());
        assert_eq!(
            serde_json::to_value(&minutes).unwrap()["estimate_unit"],
            "minutes"
        );

        // 050: restore provenance on a 'restore' row only, adjacent tails
        // inside the archived target history, unique correlation.
        let restore_fixture = || {
            let mut archive = policy_fixture();
            let doc = archive.graph.documents[0].id;
            archive.graph.states[0].snapshot_cutoff_seq = 5;
            archive.graph.states[0].tail_seq = 5;
            let body = archive.graph.documents[0].content_json.clone();
            let text = archive.graph.documents[0].text.clone();
            for (n, reason) in [(1u128, "manual"), (2, "restore")] {
                let id = Uuid::from_u128(0x7000_0000_0000_4000_8000_0000_0000_0000 + n);
                let mut r: Revision = serde_json::from_value(json!({"id":id,"target_kind":"document",
                    "target_id":doc,"snapshot_entry":format!("revisions/{id}.snapshot.v1"),"encoding":1,
                    "content_json":body,"text":text,"reason":reason,"created_by":archive.graph.source_actor_id,
                    "created_at":"2026-10-02T00:00:00Z","restored_from_id":null,"restore_correlation_id":null,
                    "restore_base_tail_seq":null,"restore_committed_tail_seq":null}))
                .unwrap();
                if reason == "restore" {
                    r.restored_from_id =
                        Some(Uuid::from_u128(0x7000_0000_0000_4000_8000_0000_0000_0001));
                    r.restore_correlation_id =
                        Some(Uuid::from_u128(0x7000_0000_0000_4000_8000_0000_0000_0009));
                    r.restore_base_tail_seq = Some(3);
                    r.restore_committed_tail_seq = Some(4);
                }
                archive
                    .entries
                    .insert(r.snapshot_entry.clone(), encode(&[0, 0]));
                archive.graph.revisions.push(r);
            }
            archive
        };
        restore_fixture().validate().unwrap();
        let manual = serde_json::to_value(&restore_fixture().graph.revisions[0]).unwrap();
        assert!(manual.get("restored_from_id").is_none());
        let invalid = |edit: fn(&mut Archive)| {
            let mut archive = restore_fixture();
            edit(&mut archive);
            matches!(archive.validate(), Err(ArchiveError::Invalid(_)))
        };
        assert!(invalid(
            |a| a.graph.revisions[1].restore_correlation_id = None
        ));
        assert!(invalid(|a| a.graph.revisions[1].restored_from_id = None));
        assert!(invalid(
            |a| a.graph.revisions[1].restore_committed_tail_seq = Some(5)
        ));
        assert!(invalid(|a| {
            a.graph.revisions[1].restore_base_tail_seq = Some(5);
            a.graph.revisions[1].restore_committed_tail_seq = Some(6);
        }));
        assert!(invalid(
            |a| a.graph.revisions[1].restore_base_tail_seq = Some(-1)
        ));
        assert!(invalid(|a| a.graph.revisions[1].created_by = None));
        assert!(invalid(
            |a| a.graph.revisions[0].restored_from_id = a.graph.revisions[1].restored_from_id
        ));
        assert!(invalid(|a| a.graph.revisions[1].reason = "manual".into()));
        assert!(invalid(|a| a.graph.revisions[0].reason = "replay".into()));
        assert!(invalid(|a| {
            let mut twin = a.graph.revisions[1].clone();
            twin.id = Uuid::from_u128(0x7000_0000_0000_4000_8000_0000_0000_0003);
            twin.snapshot_entry = format!("revisions/{}.snapshot.v1", twin.id);
            a.entries
                .insert(twin.snapshot_entry.clone(), encode(&[0, 0]));
            a.graph.revisions.push(twin);
        }));
    }

    /// Adds a second ordinary task (state, creation activity, baseline item)
    /// to an archive and returns its id.
    fn with_task(archive: &mut Archive, n: u128) -> Uuid {
        let g = &mut archive.graph;
        let id = Uuid::from_u128(0x7300_0000_0000_4000_8000_0000_0000_0000 + n);
        let mut task = g.tasks[0].clone();
        (task.id, task.number, task.sort_key) = (id, 2 + n as i32, format!("a{n}"));
        g.project.next_number = task.number + 1;
        g.tasks.push(task);
        let mut state = g
            .states
            .iter()
            .find(|s| s.target_kind == "task")
            .unwrap()
            .clone();
        (state.target_id, state.state_entry) = (id, format!("native/task/{id}/state.v1"));
        archive
            .entries
            .insert(state.state_entry.clone(), encode(&[0, 0]));
        g.states.push(state);
        let mut created = g.activity[0].clone();
        (created.id, created.task_id) = (Uuid::from_u128(id.as_u128() + 0x100), id);
        g.activity.push(created);
        let mut item = g.collection_items[0].clone();
        (item.id, item.task_id) = (Uuid::from_u128(id.as_u128() + 0x200), Some(id));
        g.collection_items.push(item);
        id
    }

    #[test]
    fn native_archive_policy_milestones_and_dependencies_stay_in_closure() {
        let fixture = || {
            let mut archive = policy_fixture();
            let (a, b) = (with_task(&mut archive, 1), with_task(&mut archive, 2));
            let g = &mut archive.graph;
            let (project, first) = (g.project.id, g.tasks[0].id);
            let milestone = Uuid::from_u128(0x7400_0000_0000_4000_8000_0000_0000_0001);
            g.milestones = serde_json::from_value(json!([{"id":milestone,"project_id":project,
                "name":"1차 마감 🧪","due_date":"2026-10-31","sort_key":"V",
                "created_at":"2026-10-02T00:00:00Z","updated_at":"2026-10-02T00:00:00Z"}]))
            .unwrap();
            g.tasks[0].milestone_id = Some(milestone);
            g.dependencies = serde_json::from_value(json!([
                {"blocker_id":first,"blocked_id":a,"type":"FS","lag_days":0},
                {"blocker_id":a,"blocked_id":b,"type":"SS","lag_days":2},
                {"blocker_id":first,"blocked_id":b,"type":"FF","lag_days":0}]))
            .unwrap();
            g.activity[1].changes.as_array_mut().unwrap().push(
                json!({"field":"milestoneId","from":null,"to":{"id":milestone,"label":"1차 마감 🧪"}}),
            );
            archive
        };
        fixture().validate().unwrap();
        let invalid = |edit: fn(&mut Archive)| {
            let mut archive = fixture();
            edit(&mut archive);
            matches!(archive.validate(), Err(ArchiveError::Invalid(_)))
        };
        // Milestones: this project, the writer's name/date/key rules, unique.
        assert!(invalid(|a| a.graph.milestones[0].project_id = Uuid::nil()));
        assert!(invalid(|a| a.graph.milestones[0].name = "   ".into()));
        assert!(invalid(|a| a.graph.milestones[0].name = "가".repeat(201)));
        assert!(invalid(
            |a| a.graph.milestones[0].due_date = Some("10/31".into())
        ));
        assert!(invalid(|a| a.graph.milestones[0].sort_key = String::new()));
        // The writer only stores canonical keys (documents::between): no
        // trailing '0', fractional alphabet only.
        assert!(invalid(|a| a.graph.milestones[0].sort_key = "a0".into()));
        assert!(invalid(|a| {
            let copy = a.graph.milestones[0].clone();
            a.graph.milestones.push(copy);
        }));
        // A task's milestone is an archived one.
        assert!(invalid(
            |a| a.graph.tasks[0].milestone_id = Some(Uuid::nil())
        ));
        // Dependencies: inside, no self edge, types, lag, pair, no cycle.
        assert!(invalid(|a| a.graph.dependencies[0].blocked_id = Uuid::nil()));
        assert!(invalid(|a| {
            a.graph.dependencies[0].blocked_id = a.graph.dependencies[0].blocker_id;
        }));
        assert!(invalid(|a| a.graph.dependencies[0].r#type = "SF".into()));
        assert!(invalid(|a| a.graph.dependencies[0].lag_days = -1));
        assert!(invalid(|a| {
            let copy = a.graph.dependencies[0].clone();
            a.graph.dependencies.push(copy);
        }));
        assert!(invalid(|a| {
            // b -> first closes first -> a -> b.
            let (first, b) = (
                a.graph.dependencies[0].blocker_id,
                a.graph.dependencies[1].blocked_id,
            );
            a.graph.dependencies.push(
                serde_json::from_value(
                    json!({"blocker_id":b,"blocked_id":first,"type":"FS","lag_days":0}),
                )
                .unwrap(),
            );
        }));
        // The milestone history reference stays inside the archive.
        let unsupported = |edit: fn(&mut Archive)| {
            let mut archive = fixture();
            edit(&mut archive);
            matches!(archive.validate(), Err(ArchiveError::Unsupported(m)) if m == "non-baseline task activity")
        };
        assert!(unsupported(|a| {
            let last = a.graph.activity[1].changes.as_array().unwrap().len() - 1;
            a.graph.activity[1].changes[last]["to"]["id"] = json!(Uuid::nil());
        }));
        assert!(unsupported(|a| {
            let last = a.graph.activity[1].changes.as_array().unwrap().len() - 1;
            a.graph.activity[1].changes[last]["to"]["label"] = json!("");
        }));
        // A purged milestone the history names travels in the typed purged
        // list (strictly ascending, named, not live, of its own kind); the
        // same reference unlisted stays the activity refusal above.
        let (first, second) = (
            Uuid::parse_str("10000000-0000-4000-8000-0000000000f1").unwrap(),
            Uuid::parse_str("10000000-0000-4000-8000-0000000000f2").unwrap(),
        );
        let purged = |listed: Vec<Uuid>, labels: Vec<Uuid>| {
            let mut archive = fixture();
            let last = archive.graph.activity[1].changes.as_array().unwrap().len() - 1;
            archive.graph.activity[1].changes[last]["from"] =
                json!({"id": first, "label": "이전 이정표"});
            archive.graph.activity[1].changes[last]["to"]["id"] = json!(second);
            archive.graph.purged_milestone_refs = listed;
            archive.graph.purged_label_refs = labels;
            archive.validate()
        };
        purged(vec![first, second], vec![]).unwrap();
        for (case, listed, labels) in [
            ("unsorted", vec![second, first], vec![]),
            ("duplicate", vec![first, first, second], vec![]),
            (
                "padded",
                vec![
                    first,
                    second,
                    Uuid::parse_str("ffffffff-ffff-4fff-bfff-ffffffffffff").unwrap(),
                ],
                vec![],
            ),
            ("cross-kind", vec![first, second], vec![first]),
        ] {
            assert!(
                matches!(purged(listed, labels), Err(ArchiveError::Invalid(_))),
                "{case}"
            );
        }
        assert!(matches!(
            purged(vec![first], vec![]),
            Err(ArchiveError::Unsupported(m)) if m == "non-baseline task activity"
        ));
        let mut archive = fixture();
        archive.graph.purged_milestone_refs = vec![archive.graph.milestones[0].id];
        assert!(
            matches!(archive.validate(), Err(ArchiveError::Invalid(_))),
            "a live milestone is never purged"
        );
        // One id named as both a label and a milestone, listed in both
        // lists or in one, and an id that is a live row of the other kind,
        // are never purged references.
        let both = |labels: Vec<Uuid>, milestones: Vec<Uuid>| {
            let mut archive = fixture();
            archive.graph.activity[1]
                .changes
                .as_array_mut()
                .unwrap()
                .insert(
                    0,
                    json!({"field":"labelIds","from":[],"to":[{"id": first, "label": null}]}),
                );
            let last = archive.graph.activity[1].changes.as_array().unwrap().len() - 1;
            archive.graph.activity[1].changes[last]["from"] = json!({"id": first, "label": null});
            archive.graph.purged_label_refs = labels;
            archive.graph.purged_milestone_refs = milestones;
            archive.validate()
        };
        assert!(matches!(
            both(vec![first], vec![first]),
            Err(ArchiveError::Invalid(_))
        ));
        assert!(matches!(
            both(vec![first], vec![]),
            Err(ArchiveError::Unsupported(m)) if m == "non-baseline task activity"
        ));
        let opposite = |listed: bool| {
            let mut archive = fixture();
            let live_label = archive.graph.labels[0].id;
            let last = archive.graph.activity[1].changes.as_array().unwrap().len() - 1;
            archive.graph.activity[1].changes[last]["from"] =
                json!({"id": live_label, "label": null});
            if listed {
                archive.graph.purged_milestone_refs = vec![live_label];
            }
            archive.validate()
        };
        assert!(matches!(opposite(true), Err(ArchiveError::Invalid(_))));
        assert!(matches!(
            opposite(false),
            Err(ArchiveError::Unsupported(m)) if m == "non-baseline task activity"
        ));
        // Distinct purged ids are capped per kind (MAX_ENTRIES), each one
        // really named by the history (50 per list side, the writers' cap);
        // repeated references to one id do not count again.
        let named = |count: usize, listed: usize| {
            let mut archive = fixture();
            let ids: Vec<Uuid> = (0..count)
                .map(|i| Uuid::from_u128(0x2000_0000_0000_4000_8000_0000_0000_0000 + i as u128))
                .collect();
            let template = archive.graph.activity[1].clone();
            for (n, chunk) in ids.chunks(MAX_TASK_REFS).enumerate() {
                let mut entry = template.clone();
                entry.id = Uuid::from_u128(0x2100_0000_0000_4000_8000_0000_0000_0000 + n as u128);
                let refs: Vec<Value> = chunk
                    .iter()
                    .map(|id| json!({"id": id, "label": null}))
                    .collect();
                let repeated = vec![refs[0].clone()];
                entry.changes = json!([{"field":"labelIds","from":refs,"to":repeated}]);
                archive.graph.activity.push(entry);
            }
            archive.graph.purged_label_refs = ids[..listed].to_vec();
            archive.validate()
        };
        named(MAX_ENTRIES, MAX_ENTRIES).unwrap();
        assert!(matches!(
            named(MAX_ENTRIES + 1, MAX_ENTRIES),
            Err(ArchiveError::Limit)
        ));
        assert!(matches!(
            named(MAX_ENTRIES + 1, MAX_ENTRIES + 1),
            Err(ArchiveError::Limit)
        ));
        // References are counted before anything is collected.
        let mut archive = fixture();
        let many: Vec<Value> = (0..=MAX_GRAPH_BYTES / HISTORY_REFERENCE_BYTES)
            .map(|_| json!(null))
            .collect();
        archive.graph.activity[1].changes = json!([{"field":"labelIds","from":many,"to":[]}]);
        assert!(matches!(
            activity_reference_count(&archive.graph.activity),
            Err(ArchiveError::Limit)
        ));
        // Recurrence: the writers' preset travels; any other shape is invalid.
        for kind in ["daily", "weekly", "monthly"] {
            let mut archive = fixture();
            archive.graph.tasks[0].recurrence = Some(json!({"kind": kind}));
            archive.validate().unwrap();
        }
        for preset in [
            json!({"kind":"yearly"}),
            json!({"kind":"weekly","interval":2}),
            json!({}),
            json!("weekly"),
            json!({"kind":null}),
        ] {
            let mut archive = fixture();
            archive.graph.tasks[0].recurrence = Some(preset.clone());
            assert!(
                matches!(archive.validate(), Err(ArchiveError::Invalid(_))),
                "{preset}"
            );
        }
        // Cost: the iterative cycle check walks a 250-task project (the state
        // object bound) with a long chain plus every forward edge within 40
        // steps, and still finds a single closing back edge.
        let mut archive = policy_fixture();
        let mut ids = vec![archive.graph.tasks[0].id];
        for n in 1..250 {
            ids.push(with_task(&mut archive, n));
        }
        let mut edges = Vec::new();
        for (i, blocker) in ids.iter().enumerate() {
            for blocked in ids.iter().skip(i + 1).take(40) {
                edges.push(
                    json!({"blocker_id":blocker,"blocked_id":blocked,"type":"FS","lag_days":0}),
                );
            }
        }
        assert!(edges.len() > 9_000);
        archive.graph.dependencies = serde_json::from_value(json!(edges)).unwrap();
        archive.validate().unwrap();
        archive.graph.dependencies.push(
            serde_json::from_value(
                json!({"blocker_id":ids[249],"blocked_id":ids[0],"type":"FS","lag_days":0}),
            )
            .unwrap(),
        );
        assert!(matches!(archive.validate(), Err(ArchiveError::Invalid(m)) if m == "dependencies"));
    }

    #[test]
    fn native_archive_policy_document_tags_follow_the_writers() {
        let tag = Uuid::from_u128(0x7700_0000_0000_4000_8000_0000_0000_0001);
        let fixture = || {
            let mut archive = policy_fixture();
            let doc = archive.graph.documents[0].id;
            archive.graph.document_tags = serde_json::from_value(json!([{"id":tag,"name":"Ref 참고 🧪",
                "color":"blue","created_at":"2026-10-02T00:00:00Z","updated_at":"2026-10-02T00:00:00Z"}]))
            .unwrap();
            archive.graph.document_tag_assignments =
                serde_json::from_value(json!([{"document_id":doc,"tag_id":tag}])).unwrap();
            archive
        };
        fixture().validate().unwrap();
        let invalid = |edit: fn(&mut Archive)| {
            let mut archive = fixture();
            edit(&mut archive);
            matches!(archive.validate(), Err(ArchiveError::Invalid(m)) if m == "document tags")
        };
        assert!(invalid(|a| a.graph.document_tags[0].name = " Ref".into()));
        assert!(invalid(|a| a.graph.document_tags[0].name = "가".repeat(101)));
        assert!(invalid(|a| a.graph.document_tags[0].color = "black".into()));
        assert!(invalid(|a| {
            let copy = a.graph.document_tags[0].clone();
            a.graph.document_tags.push(copy);
        }));
        assert!(invalid(
            |a| a.graph.document_tag_assignments[0].document_id = Uuid::nil()
        ));
        assert!(invalid(
            |a| a.graph.document_tag_assignments[0].tag_id = Uuid::nil()
        ));
        assert!(invalid(|a| {
            let copy = a.graph.document_tag_assignments[0].clone();
            a.graph.document_tag_assignments.push(copy);
        }));
        // A tag no archived document carries is workspace data, not this
        // archive's.
        assert!(invalid(|a| a.graph.document_tag_assignments.clear()));
    }

    #[test]
    fn native_archive_policy_collections_follow_the_writers() {
        let id = |n: u128| Uuid::from_u128(0x7600_0000_0000_4000_8000_0000_0000_0000 + n);
        let fixture = || {
            let mut archive = policy_fixture();
            let g = &mut archive.graph;
            let (actor, collection, item) = (
                g.source_actor_id,
                g.collections[0].id,
                g.collection_items[0].id,
            );
            let at = "2026-10-02T00:00:00Z";
            let field = |n: u128, key: &str, kind: &str| {
                json!({"id":id(n),"collection_id":collection,"key":key,
                "name":format!("필드 {n}"),"description":null,"type":kind,"sort_key":format!("{n:03}"),"version":1,
                "deleted_at":null,"created_at":at,"updated_at":at})
            };
            // A project rename: the collection keeps its original name.
            g.collections[0].name = "처음 이름".into();
            g.collections[0].version = 1;
            g.collection_items[0].version = 4;
            g.collection_fields = serde_json::from_value(json!([
                field(1, "stage", "select"),
                field(2, "score", "number"),
                field(3, "owner", "user"),
                field(4, "memo", "paragraph"),
                field(5, "due_on", "date")
            ]))
            .unwrap();
            g.collection_options = serde_json::from_value(json!([
                {"id":id(11),"collection_id":collection,"field_id":id(1),"key":"o_1","label":"준비","sort_key":"000","deleted_at":null},
                {"id":id(12),"collection_id":collection,"field_id":id(1),"key":"o_2","label":"보관됨","sort_key":"001","deleted_at":at}])).unwrap();
            g.collection_values = serde_json::from_value(json!([
                {"collection_id":collection,"item_id":item,"field_id":id(2),"field_type":"number","value_text":null,"value_number":"1.5","value_date":null,"value_ts":null,"value_bool":null},
                {"collection_id":collection,"item_id":item,"field_id":id(4),"field_type":"paragraph","value_text":"메모 🧪","value_number":null,"value_date":null,"value_ts":null,"value_bool":null}])).unwrap();
            // An archived option may stay chosen.
            g.collection_choices = serde_json::from_value(json!([{"collection_id":collection,"item_id":item,"field_id":id(1),"field_type":"select","option_id":id(12)}])).unwrap();
            g.collection_people = serde_json::from_value(json!([{"collection_id":collection,"item_id":item,"field_id":id(3),"field_type":"user","user_id":actor}])).unwrap();
            g.collection_views = serde_json::from_value(json!([{"id":id(21),"collection_id":collection,"owner_id":actor,"visibility":"shared",
                "name":"보드","type":"board","config":{"query":{"filters":{},"sort":[]},"groupBy":id(1).to_string(),"dateBy":id(5).to_string()},
                "version":2,"created_at":at,"updated_at":at}])).unwrap();
            archive
        };
        fixture().validate().unwrap();
        let fails = |edit: fn(&mut Archive)| {
            let mut archive = fixture();
            edit(&mut archive);
            archive.validate().err()
        };
        let invalid =
            |edit: fn(&mut Archive)| matches!(fails(edit), Some(ArchiveError::Invalid(_)));
        let unsupported = |edit: fn(&mut Archive), reason: &str| matches!(fails(edit), Some(ArchiveError::Unsupported(m)) if m == reason);
        // Collections: deleted / missing task collection refused typed.
        assert!(unsupported(
            |a| a.graph.collections[0].deleted_at = Some("2026-10-02T00:00:00Z".into()),
            "collections"
        ));
        assert!(unsupported(
            |a| a.graph.collections[0].kind = "document".into(),
            "collections"
        ));
        // Another project's collection stays the typed refusal; a workspace
        // wiki collection is a document collection holding archived wiki
        // documents only, never empty and never a project document.
        let wiki = |items: bool, kind: &'static str| {
            let mut archive = fixture();
            let mut c = archive.graph.collections[0].clone();
            c.id = id(31);
            c.project_id = None;
            c.kind = kind.into();
            c.name = "위키 모음".into();
            archive.graph.collections.push(c);
            if items {
                let mut i = archive.graph.collection_items[0].clone();
                i.id = id(32);
                i.collection_id = id(31);
                i.task_id = None;
                i.document_id = Some(archive.graph.documents[0].id);
                archive.graph.collection_items.push(i);
            }
            archive.validate().err()
        };
        assert!(matches!(
            wiki(false, "document"),
            Some(ArchiveError::Invalid(_))
        ));
        assert!(matches!(
            wiki(true, "document"),
            Some(ArchiveError::Invalid(_))
        ));
        assert!(matches!(
            wiki(false, "task"),
            Some(ArchiveError::Invalid(_))
        ));
        let mut archive = fixture();
        let mut other = archive.graph.collections[0].clone();
        other.id = id(33);
        other.project_id = Some(Uuid::nil());
        other.kind = "document".into();
        archive.graph.collections.push(other);
        assert!(
            matches!(archive.validate(), Err(ArchiveError::Unsupported(m)) if m == "collections")
        );
        // The task collection keeps the project name contract (trim only to
        // check): a leading space is a valid raw name, a blank one is not.
        let mut archive = fixture();
        archive.graph.collections[0].name = " 이름".into();
        archive.validate().unwrap();
        assert!(invalid(|a| a.graph.collections[0].name = "   ".into()));
        assert!(invalid(|a| a.graph.collection_items[0].version = 0));
        // Fields: key, name, description, type, unique key, collection.
        assert!(invalid(
            |a| a.graph.collection_fields[0].key = "Stage".into()
        ));
        assert!(invalid(
            |a| a.graph.collection_fields[1].key = "stage".into()
        ));
        assert!(invalid(
            |a| a.graph.collection_fields[0].name = String::new()
        ));
        assert!(invalid(
            |a| a.graph.collection_fields[0].description = Some("가".repeat(2001))
        ));
        assert!(invalid(
            |a| a.graph.collection_fields[0].r#type = "formula".into()
        ));
        assert!(invalid(
            |a| a.graph.collection_fields[0].collection_id = Uuid::nil()
        ));
        // Options only on choice fields, unique key.
        assert!(invalid(|a| a.graph.collection_options[0].field_id =
            Uuid::from_u128(0x7600_0000_0000_4000_8000_0000_0000_0002)));
        assert!(invalid(|a| a.graph.collection_options[1].key = "o_1".into()));
        // Values: type/table match, canonical number, one row per cell.
        assert!(invalid(
            |a| a.graph.collection_values[0].value_number = Some("1e3".into())
        ));
        assert!(invalid(
            |a| a.graph.collection_values[0].value_number = Some(".5".into())
        ));
        assert!(invalid(
            |a| a.graph.collection_values[0].value_number = Some("NaN".into())
        ));
        assert!(invalid(
            |a| a.graph.collection_values[0].field_type = "text".into()
        ));
        assert!(invalid(
            |a| a.graph.collection_values[1].value_text = Some("x".repeat(10_001))
        ));
        assert!(invalid(|a| {
            a.graph.collection_values[0].value_bool = Some(true);
        }));
        assert!(invalid(|a| {
            let copy = a.graph.collection_values[0].clone();
            a.graph.collection_values.push(copy);
        }));
        assert!(invalid(
            |a| a.graph.collection_values[0].item_id = Uuid::nil()
        ));
        // Choices: an option of the field, at most one for select.
        assert!(invalid(
            |a| a.graph.collection_choices[0].option_id = Uuid::nil()
        ));
        assert!(invalid(|a| {
            let mut second = a.graph.collection_choices[0].clone();
            second.option_id = Uuid::from_u128(0x7600_0000_0000_4000_8000_0000_0000_000b);
            a.graph.collection_choices.push(second);
        }));
        // A cell uses one table only.
        assert!(invalid(|a| {
            let mut choice = a.graph.collection_choices[0].clone();
            choice.field_id = Uuid::from_u128(0x7600_0000_0000_4000_8000_0000_0000_0002);
            choice.field_type = "number".into();
            a.graph.collection_choices.push(choice);
        }));
        // People: the source actor only (typed), on people fields.
        assert!(unsupported(
            |a| a.graph.collection_people[0].user_id = Uuid::nil(),
            "collection people"
        ));
        assert!(invalid(
            |a| a.graph.collection_people[0].field_type = "select".into()
        ));
        // Views: owner (typed), visibility, type, canonical config, live fields.
        assert!(unsupported(
            |a| a.graph.collection_views[0].owner_id = Uuid::nil(),
            "collection views"
        ));
        assert!(invalid(
            |a| a.graph.collection_views[0].visibility = "public".into()
        ));
        assert!(invalid(
            |a| a.graph.collection_views[0].r#type = "list".into()
        ));
        assert!(invalid(
            |a| a.graph.collection_views[0].config["extra"] = json!(1)
        ));
        assert!(invalid(
            |a| a.graph.collection_views[0].config["groupBy"] =
                json!(Uuid::from_u128(0x7600_0000_0000_4000_8000_0000_0000_0002).to_string())
        ));
        assert!(unsupported(
            |a| a.graph.collection_views[0].config["query"]["filters"]["statusId"] =
                json!(Uuid::nil().to_string()),
            "view references"
        ));
        // Option catalog cap (PATCH_OPTIONS_MAX, archived options included).
        let with_options = |total: u128| {
            let mut archive = fixture();
            let template = archive.graph.collection_options[1].clone();
            for n in 2..total {
                let mut option = template.clone();
                (option.id, option.key) = (id(1000 + n), format!("o_{}", n + 1));
                archive.graph.collection_options.push(option);
            }
            archive.validate()
        };
        with_options(200).unwrap();
        assert!(matches!(with_options(201), Err(ArchiveError::Invalid(_))));
        // Names: the task collection follows the project name contract (200
        // chars, the 028 trigger copies the project's name), not the 100
        // UTF-16 collection writer limit.
        let mut archive = fixture();
        archive.graph.collections[0].name = "가".repeat(101);
        archive.validate().unwrap();
        assert!(invalid(|a| a.graph.collections[0].name = "가".repeat(201)));
        // UTF-16 limits (the writer's limited_string): astral characters
        // count twice.
        let mut archive = fixture();
        archive.graph.collection_values[1].value_text = Some("😀".repeat(5000));
        archive.graph.collection_fields[0].description = Some("😀".repeat(1000));
        archive.validate().unwrap();
        assert!(invalid(
            |a| a.graph.collection_values[1].value_text = Some("😀".repeat(5001))
        ));
        assert!(invalid(
            |a| a.graph.collection_fields[0].description = Some("😀".repeat(1001))
        ));
        // A numeric text with a scale the writer did not choose is still a
        // plain finite decimal.
        let mut archive = fixture();
        archive.graph.collection_values[0].value_number = Some("-1.50".into());
        archive.validate().unwrap();
        // A view whose date field was deleted later stays valid as stored.
        let mut archive = fixture();
        archive.graph.collection_fields[4].deleted_at = Some("2026-10-02T00:00:00Z".into());
        archive.validate().unwrap();
        // Query references: custom option/people values and field sorts on
        // this collection's fields (an archived option included), the source
        // actor (remapped); outside ones are typed refusals.
        let with_query = |filters: Value, sort: Value| {
            let mut archive = fixture();
            archive.graph.collection_views[0].config["query"] =
                json!({"filters":filters,"sort":sort});
            archive.validate()
        };
        let stage = id(1).to_string();
        let archived_option = id(12).to_string();
        let owner = id(3).to_string();
        let actor = fixture().graph.source_actor_id.to_string();
        with_query(
            json!({"custom":[{"fieldId":stage,"operator":"equals","value":archived_option},{"fieldId":owner,"operator":"equals","value":actor}]}),
            json!([{"field":id(2).to_string(),"direction":"desc"}]),
        )
        .unwrap();
        for (filters, sort) in [
            (
                json!({"custom":[{"fieldId":stage,"operator":"equals","value":Uuid::nil().to_string()}]}),
                json!([]),
            ),
            (
                json!({"custom":[{"fieldId":owner,"operator":"equals","value":Uuid::nil().to_string()}]}),
                json!([]),
            ),
            (
                json!({"custom":[{"fieldId":Uuid::nil().to_string(),"operator":"empty"}]}),
                json!([]),
            ),
            (
                json!({}),
                json!([{"field":Uuid::nil().to_string(),"direction":"asc"}]),
            ),
        ] {
            assert!(
                matches!(with_query(filters.clone(), sort.clone()), Err(ArchiveError::Unsupported(m)) if m == "view references"),
                "{filters} {sort}"
            );
        }
        // Typed mapping: the assignee and a people value (also in the
        // uppercase spelling the writer accepts) are remapped; a text field
        // whose literal text is the source id, and an option value, are not.
        let memo = id(4).to_string();
        let upper = actor.to_uppercase();
        let config = json!({"query":{"filters":{"assigneeId":actor,"custom":[
            {"fieldId":owner,"operator":"equals","value":upper},
            {"fieldId":memo,"operator":"equals","value":actor},
            {"fieldId":stage,"operator":"equals","value":archived_option}]},"sort":[]},"groupBy":null,"dateBy":null});
        let mut archive = fixture();
        archive.graph.collection_views[0].config = config.clone();
        archive.validate().unwrap();
        let people = |field: Uuid| field == id(3);
        let destination = Uuid::from_u128(42).to_string();
        let mapped = mapped_collection_view_config(
            &config,
            &people,
            fixture().graph.source_actor_id,
            Uuid::from_u128(42),
        );
        assert_eq!(mapped["query"]["filters"]["assigneeId"], json!(destination));
        assert_eq!(
            mapped["query"]["filters"]["custom"][0]["value"],
            json!(destination)
        );
        assert_eq!(
            mapped["query"]["filters"]["custom"][1]["value"],
            json!(actor)
        );
        assert_eq!(
            mapped["query"]["filters"]["custom"][2]["value"],
            json!(archived_option)
        );
        // Option/people values use the writer's 36-character UUID text only.
        let compact_option = archived_option.replace('-', "");
        let compact_actor = actor.replace('-', "");
        for filters in [
            json!({"custom":[{"fieldId":stage,"operator":"equals","value":compact_option}]}),
            json!({"custom":[{"fieldId":owner,"operator":"equals","value":compact_actor}]}),
            json!({"custom":[{"fieldId":owner,"operator":"equals","value":format!("{{{actor}}}")}]}),
        ] {
            assert!(
                matches!(
                    with_query(filters.clone(), json!([])),
                    Err(ArchiveError::Invalid(_))
                ),
                "{filters}"
            );
        }
        // Writer shapes: a number field equals text, a sort on an option
        // field, a task filter on a document collection are Invalid.
        for (filters, sort) in [
            (
                json!({"custom":[{"fieldId":id(2).to_string(),"operator":"equals","value":"1.5"}]}),
                json!([]),
            ),
            (
                json!({"custom":[{"fieldId":id(5).to_string(),"operator":"equals","value":3}]}),
                json!([]),
            ),
            (
                json!({"custom":[{"fieldId":stage,"operator":"equals","value":"not-a-uuid"}]}),
                json!([]),
            ),
            (json!({}), json!([{"field":stage,"direction":"asc"}])),
        ] {
            assert!(
                matches!(
                    with_query(filters.clone(), sort.clone()),
                    Err(ArchiveError::Invalid(_))
                ),
                "{filters} {sort}"
            );
        }
        let mut documents = fixture();
        let mut collection = documents.graph.collections[0].clone();
        (collection.id, collection.kind, collection.name) =
            (id(31), "document".into(), "자료".into());
        documents.graph.collections.push(collection);
        let mut view = documents.graph.collection_views[0].clone();
        (view.id, view.collection_id, view.r#type) = (id(32), id(31), "table".into());
        view.config =
            json!({"query":{"filters":{"openOnly":true},"sort":[]},"groupBy":null,"dateBy":null});
        documents.graph.collection_views.push(view);
        assert!(matches!(documents.validate(), Err(ArchiveError::Invalid(m)) if m == "view query"));
        // A soft-deleted field keeps its values (history), when no view uses it.
        let mut archive = fixture();
        archive.graph.collection_fields[1].deleted_at = Some("2026-10-02T00:00:00Z".into());
        archive.validate().unwrap();
    }

    #[test]
    fn native_archive_policy_saved_views_stay_owner_private_and_in_closure() {
        let fixture = || {
            let mut archive = policy_fixture();
            let g = &mut archive.graph;
            let (actor, project) = (g.source_actor_id, g.project.id);
            let (status, label) = (g.statuses[0].id, g.labels[0].id);
            let view = Uuid::from_u128(0x7500_0000_0000_4000_8000_0000_0000_0001);
            // The writer's canonical config (view_query_to_json of its parse).
            g.views = serde_json::from_value(json!([{"id":view,"project_id":project,"user_id":actor,
                "name":"내 보기 🧪","type":"board","config":{"filters":{"statusId":status.to_string(),
                "assigneeId":actor.to_string(),"labelId":label.to_string(),"openOnly":true,"title":"검토"},
                "sort":[{"field":"due","direction":"asc"}]},
                "created_at":"2026-10-02T00:00:00Z","updated_at":"2026-10-02T00:00:00Z"}]))
            .unwrap();
            archive
        };
        fixture().validate().unwrap();
        let fails = |edit: fn(&mut Archive)| {
            let mut archive = fixture();
            edit(&mut archive);
            archive.validate().err()
        };
        let invalid =
            |edit: fn(&mut Archive)| matches!(fails(edit), Some(ArchiveError::Invalid(_)));
        let unsupported = |edit: fn(&mut Archive), reason: &str| matches!(fails(edit), Some(ArchiveError::Unsupported(m)) if m == reason);
        // Another person's private view is refused, never dropped.
        assert!(unsupported(
            |a| a.graph.views[0].user_id = Uuid::nil(),
            "views"
        ));
        // Writer shape: project, type, trimmed name, canonical config.
        assert!(invalid(|a| a.graph.views[0].project_id = Uuid::nil()));
        assert!(invalid(|a| a.graph.views[0].r#type = "timeline".into()));
        assert!(invalid(|a| a.graph.views[0].name = " 내 보기".into()));
        assert!(invalid(
            |a| a.graph.views[0].config["filters"]["openOnly"] = json!(false)
        ));
        assert!(invalid(
            |a| a.graph.views[0].config["filters"]["unknown"] = json!(1)
        ));
        assert!(invalid(|a| {
            let copy = a.graph.views[0].clone();
            a.graph.views.push(copy);
        }));
        // References stay inside the archive.
        assert!(unsupported(
            |a| a.graph.views[0].config["filters"]["statusId"] = json!(Uuid::nil().to_string()),
            "view references"
        ));
        assert!(unsupported(
            |a| a.graph.views[0].config["filters"]["labelId"] = json!(Uuid::nil().to_string()),
            "view references"
        ));
        assert!(unsupported(
            |a| a.graph.views[0].config["filters"]["milestoneId"] = json!(Uuid::nil().to_string()),
            "view references"
        ));
        assert!(unsupported(
            |a| a.graph.views[0].config["filters"]["assigneeId"] = json!(Uuid::nil().to_string()),
            "view references"
        ));
        assert!(unsupported(
            |a| a.graph.views[0].config["sort"] =
                json!([{"field":Uuid::nil().to_string(),"direction":"asc"}]),
            "view references"
        ));
        assert!(unsupported(
            |a| a.graph.views[0].config["filters"]["custom"] =
                json!([{"fieldId":Uuid::nil().to_string(),"operator":"empty"}]),
            "view references"
        ));
        // A purged label/milestone that a stored filter names (a purge never
        // touches views) travels as a listed purged reference named by the
        // view; unlisted it stays a view reference outside the archive, a
        // listed id nothing names is invalid, and a purged id is never a live
        // task label.
        let (gone_label, gone_milestone) = (
            Uuid::from_u128(0x7500_0000_0000_4000_8000_0000_0000_00f1),
            Uuid::from_u128(0x7500_0000_0000_4000_8000_0000_0000_00f2),
        );
        let purged = |labels: Vec<Uuid>, milestones: Vec<Uuid>, as_task_label: bool| {
            let mut archive = fixture();
            let filters = &mut archive.graph.views[0].config["filters"];
            filters["labelId"] = json!(gone_label.to_string());
            filters["milestoneId"] = json!(gone_milestone.to_string());
            archive.graph.purged_label_refs = labels;
            archive.graph.purged_milestone_refs = milestones;
            if as_task_label {
                let task_id = archive.graph.tasks[0].id;
                archive.graph.task_labels.push(TaskLabel {
                    task_id,
                    label_id: gone_label,
                });
            }
            archive.validate()
        };
        purged(vec![gone_label], vec![gone_milestone], false).unwrap();
        // The filter count is bounded by the view rows before collection.
        let mut archive = fixture();
        archive.graph.views[0].config["filters"]["milestoneId"] = json!(gone_milestone.to_string());
        let (views, collection_views) = (&archive.graph.views, &archive.graph.collection_views);
        assert_eq!(
            view_filter_ref_bound(views, collection_views).unwrap(),
            2 * (views.len() + collection_views.len())
        );
        assert_eq!(view_filter_refs(views, collection_views).len(), 2);
        assert!(matches!(
            purged(vec![gone_label], vec![], false),
            Err(ArchiveError::Unsupported(m)) if m == "view references"
        ));
        assert!(matches!(
            purged(vec![gone_label], vec![gone_milestone], true),
            Err(ArchiveError::Invalid(_))
        ));
        assert!(matches!(
            purged(
                vec![
                    gone_label,
                    Uuid::from_u128(0x7500_0000_0000_4000_8000_0000_0000_00f3)
                ],
                vec![gone_milestone],
                false
            ),
            Err(ArchiveError::Invalid(_))
        ));
        assert!(matches!(
            purged(
                vec![gone_label, gone_milestone],
                vec![gone_milestone],
                false
            ),
            Err(ArchiveError::Invalid(_))
        ));
        // "me" stays "me"; the source actor's id becomes the destination's.
        let mut archive = fixture();
        archive.graph.views[0].config["filters"]["assigneeId"] = json!("me");
        archive.validate().unwrap();
        let (source, destination) = (archive.graph.source_actor_id, Uuid::from_u128(42));
        let config = fixture().graph.views[0].config.clone();
        let no_people = |_: Uuid| false;
        assert_eq!(
            mapped_project_view_config(&config, &no_people, source, destination)["filters"]
                ["assigneeId"],
            json!(destination.to_string())
        );
        assert_eq!(
            mapped_project_view_config(
                &archive.graph.views[0].config,
                &no_people,
                source,
                destination
            ),
            archive.graph.views[0].config
        );
    }

    /// Policy fixture plus the source actor's task time: a closed entry, a
    /// stopped run whose closed segment projected that entry, its receipt and
    /// a correction audit row.
    fn task_time_fixture() -> Archive {
        let mut archive = policy_fixture();
        let g = &mut archive.graph;
        let (actor, task, ws) = (g.source_actor_id, g.tasks[0].id, g.source_workspace_id);
        let id = |n: u128| Uuid::from_u128(0x7200_0000_0000_4000_8000_0000_0000_0000 + n);
        g.time_entries = serde_json::from_value(json!([{"id":id(1),"task_id":task,"user_id":actor,
            "started_at":"2026-10-02T09:00:00Z","ended_at":"2026-10-02T10:00:00Z","duration_seconds":3600,"note":"기록 🧪"}]))
        .unwrap();
        g.timer_runs = serde_json::from_value(json!([{"id":id(2),"user_id":actor,"task_id":task,"status":"stopped",
            "version":3,"started_at":"2026-10-02T09:00:00Z","stopped_at":"2026-10-02T10:00:00Z","note":null}]))
        .unwrap();
        g.timer_segments = serde_json::from_value(json!([{"id":id(3),"run_id":id(2),"user_id":actor,"task_id":task,
            "started_at":"2026-10-02T09:00:00Z","ended_at":"2026-10-02T10:00:00Z","time_entry_id":id(1)}]))
        .unwrap();
        g.timer_commands = serde_json::from_value(json!([{"request_id":id(4),"user_id":actor,"request_hash":"b".repeat(64),
            "run_id":id(2),"result":{"runId":id(2),"status":"stopped"},"created_at":"2026-10-02T10:00:00Z"}]))
        .unwrap();
        g.timer_audit = serde_json::from_value(json!([{"id":id(5),"user_id":actor,"request_id":id(6),"workspace_id":ws,
            "task_id":task,"time_entry_id":id(1),"verb":"time.correct","before_value":{"recordId":id(1),"kind":"manual"},
            "after_value":{"recordId":id(1),"kind":"manual","revision":2},"reason":"정정","created_at":"2026-10-02T11:00:00Z"}]))
        .unwrap();
        archive
    }

    #[test]
    fn native_archive_policy_task_time_rows_stay_in_actor_and_closure() {
        task_time_fixture().validate().unwrap();
        let fails = |edit: fn(&mut Archive)| {
            let mut archive = task_time_fixture();
            edit(&mut archive);
            archive.validate().err()
        };
        let invalid =
            |edit: fn(&mut Archive)| matches!(fails(edit), Some(ArchiveError::Invalid(_)));
        // Another author's entry cannot travel in a single-author archive.
        assert!(matches!(
            fails(|a| a.graph.time_entries[0].user_id = Uuid::nil()),
            Some(ArchiveError::Unsupported(m)) if m == "time entries"
        ));
        // 034 CHECK mirrors: positive whole-second duration, range, note.
        assert!(invalid(
            |a| a.graph.time_entries[0].duration_seconds = Some(3599)
        ));
        assert!(invalid(|a| a.graph.time_entries[0].duration_seconds = None));
        assert!(invalid(
            |a| a.graph.time_entries[0].ended_at = Some("2026-10-02T08:00:00Z".into())
        ));
        assert!(invalid(
            |a| a.graph.time_entries[0].note = Some("x".repeat(2001))
        ));
        assert!(invalid(|a| a.graph.time_entries[0].task_id = Uuid::nil()));
        assert!(invalid(|a| {
            // Two open entries of one person.
            let mut open = a.graph.time_entries[0].clone();
            (open.ended_at, open.duration_seconds) = (None, None);
            let mut other = open.clone();
            other.id = Uuid::from_u128(9);
            a.graph.time_entries.extend([open, other]);
            a.graph.time_entries.remove(0);
        }));
        // 048 runs: status/version/stopped rules, one unfinished per person.
        assert!(invalid(|a| a.graph.timer_runs[0].status = "lost".into()));
        assert!(invalid(|a| a.graph.timer_runs[0].version = 0));
        assert!(invalid(|a| a.graph.timer_runs[0].stopped_at = None));
        assert!(invalid(
            |a| a.graph.timer_runs[0].stopped_at = Some("2026-10-02T08:00:00Z".into())
        ));
        assert!(invalid(|a| a.graph.timer_runs[0].user_id = Uuid::nil()));
        // W5 release keeps the open 034 row and drops only its reservation,
        // and a later start counts unfinished runs plus reservations: a
        // released open entry with a later running or paused run is valid,
        // a reserved one with an unfinished run is not.
        let released_open_with = |status: &str| {
            let mut archive = task_time_fixture();
            let g = &mut archive.graph;
            g.time_entries[0].ended_at = None;
            g.time_entries[0].duration_seconds = None;
            g.timer_runs[0].status = status.into();
            g.timer_runs[0].stopped_at = None;
            g.timer_segments[0].time_entry_id = None;
            if status == "running" {
                g.timer_segments[0].ended_at = None;
            }
            archive
        };
        for status in ["running", "paused"] {
            released_open_with(status).validate().unwrap();
            let mut reserved = released_open_with(status);
            let g = &mut reserved.graph;
            g.timer_legacy_open =
                serde_json::from_value(json!([{"time_entry_id":g.time_entries[0].id,
                "user_id":g.source_actor_id,"task_id":g.tasks[0].id}]))
                .unwrap();
            assert!(
                matches!(reserved.validate(), Err(ArchiveError::Invalid(m)) if m == "task time"),
                "{status}"
            );
        }
        // Segments stay inside their run/task and project an entry of it.
        assert!(invalid(|a| a.graph.timer_segments[0].run_id = Uuid::nil()));
        assert!(invalid(
            |a| a.graph.timer_segments[0].time_entry_id = Some(Uuid::nil())
        ));
        assert!(invalid(|a| a.graph.timer_segments[0].ended_at = None));
        // A legacy reservation belongs to an open entry of the same task.
        assert!(invalid(|a| {
            let entry = a.graph.time_entries[0].id;
            a.graph.timer_legacy_open = serde_json::from_value(json!([{"time_entry_id":entry,
                "user_id":a.graph.source_actor_id,"task_id":a.graph.tasks[0].id}]))
            .unwrap();
        }));
        // Receipts and audit: immutable shapes, actor and locators inside.
        assert!(invalid(
            |a| a.graph.timer_commands[0].request_hash = "b".repeat(63)
        ));
        assert!(invalid(
            |a| a.graph.timer_commands[0].run_id = Some(Uuid::nil())
        ));
        assert!(invalid(|a| a.graph.timer_commands[0].user_id = Uuid::nil()));
        // A historical workspace locator needs a selected task or entry on the
        // same audit row (MOVE provenance); alone it is outside the closure.
        assert!(invalid(|a| {
            a.graph.timer_audit[0].workspace_id = Some(Uuid::nil());
            a.graph.timer_audit[0].task_id = None;
            a.graph.timer_audit[0].time_entry_id = None;
        }));
        let mut historical = task_time_fixture();
        historical.graph.timer_audit[0].workspace_id = Some(Uuid::from_u128(77));
        historical.validate().unwrap();
        historical.graph.timer_audit[0].time_entry_id = None;
        historical.validate().unwrap();
        assert!(invalid(|a| a.graph.timer_audit[0].reason = String::new()));
        assert!(invalid(
            |a| a.graph.timer_audit[0].time_entry_id = Some(Uuid::nil())
        ));
        // The legacy release shape: an entry-only audit (workspace and task
        // NULL) and a receipt without a run are valid.
        let mut archive = task_time_fixture();
        archive.graph.timer_audit[0].workspace_id = None;
        archive.graph.timer_audit[0].task_id = None;
        archive.graph.timer_commands[0].run_id = None;
        archive.validate().unwrap();
        // Typed JSON association (writer value shapes): references inside the
        // archive, of the recorded kind, on the audit's task.
        assert!(invalid(
            |a| a.graph.timer_audit[0].before_value["recordId"] = json!(Uuid::nil())
        ));
        assert!(invalid(
            |a| a.graph.timer_audit[0].after_value["kind"] = json!("segment")
        ));
        assert!(invalid(
            |a| a.graph.timer_audit[0].before_value["kind"] = json!("x")
        ));
        assert!(invalid(
            |a| a.graph.timer_audit[0].after_value["recordId"] = json!(7)
        ));
        assert!(invalid(
            |a| a.graph.timer_audit[0].after_value["runId"] = json!(Uuid::nil())
        ));
        assert!(invalid(
            |a| a.graph.timer_audit[0].after_value["timeEntryId"] = json!(Uuid::nil())
        ));
        assert!(invalid(|a| {
            // The audit names another selected task than its record's.
            let other = with_task(a, 1);
            a.graph.timer_audit[0].task_id = Some(other);
            a.graph.timer_audit[0].time_entry_id = None;
        }));
        assert!(invalid(|a| {
            // The entry column on another selected task than the audit's.
            let other = with_task(a, 1);
            a.graph.timer_audit[0].task_id = Some(other);
            a.graph.timer_audit[0].before_value = json!({});
            a.graph.timer_audit[0].after_value = json!({});
        }));
        // Valid writer shapes: a segment correction, a NULL-locator cleanup
        // naming its run and closed segment, a start (no run yet, no closed
        // segment), an estimate (no record) and a release (entry column).
        let shapes = [
            (
                true,
                false,
                json!({"recordId":Uuid::from_u128(0x7200_0000_0000_4000_8000_0000_0000_0003),"kind":"segment"}),
                json!({}),
            ),
            (
                false,
                false,
                json!({"version":2}),
                json!({"runId":Uuid::from_u128(0x7200_0000_0000_4000_8000_0000_0000_0002),"recordId":Uuid::from_u128(0x7200_0000_0000_4000_8000_0000_0000_0003),"kind":"segment"}),
            ),
            (
                true,
                false,
                json!({"runId":null,"expectedVersion":0}),
                json!({"runId":Uuid::from_u128(0x7200_0000_0000_4000_8000_0000_0000_0002),"recordId":null,"kind":"segment"}),
            ),
            (
                true,
                false,
                json!({"value":null,"unit":null,"updatedAt":"2026-10-02T00:00:00Z"}),
                json!({"value":"90","unit":"minutes","updatedAt":"2026-10-02T01:00:00Z"}),
            ),
            (
                false,
                true,
                json!({"reserved":true}),
                json!({"timeEntryId":Uuid::from_u128(0x7200_0000_0000_4000_8000_0000_0000_0001),"released":true}),
            ),
        ];
        for (with_task_locator, with_entry, before, after) in shapes {
            let mut archive = task_time_fixture();
            let audit = &mut archive.graph.timer_audit[0];
            if !with_task_locator {
                (audit.workspace_id, audit.task_id) = (None, None);
            }
            if !with_entry {
                audit.time_entry_id = None;
            }
            (audit.before_value, audit.after_value) = (before.clone(), after.clone());
            archive
                .validate()
                .unwrap_or_else(|e| panic!("{before} {after}: {e:?}"));
        }
        // A released legacy reservation (open entry without one) is valid.
        let mut archive = task_time_fixture();
        archive.graph.timer_segments.clear();
        archive.graph.time_entries[0].ended_at = None;
        archive.graph.time_entries[0].duration_seconds = None;
        archive.validate().unwrap();
    }

    #[test]
    fn native_archive_policy_rejects_invalid_sql_metadata_before_storage_effects() {
        let mut archive = policy_fixture();
        archive.graph.statuses[0].created_at = "not-a-date".into();
        assert!(matches!(archive.validate(), Err(ArchiveError::Invalid(_))));
        let mut archive = policy_fixture();
        archive.graph.tasks[0].parent_id = Some(archive.graph.tasks[0].id);
        assert!(matches!(archive.validate(), Err(ArchiveError::Invalid(_))));
        let mut archive = policy_fixture();
        archive.graph.activity[0].channel = "arbitrary".into();
        assert!(matches!(
            archive.validate(),
            Err(ArchiveError::Unsupported(_))
        ));
        let mut archive = policy_fixture();
        archive.graph.tasks[0].number = archive.graph.documents[0].number;
        assert!(matches!(archive.validate(), Err(ArchiveError::Invalid(_))));
    }
    #[test]
    fn native_archive_capture_binding_matches_independent_literal_and_scope() {
        let mut archive = policy_fixture();
        let state = archive.graph.states[0].clone();
        let target = |state: &NativeState, snapshot: &[u8]| NativeTargetInput {
            snapshot: snapshot.to_vec(),
            cutoff: state.snapshot_cutoff_seq,
            ..archive_target(&archive, state).unwrap()
        };
        let expected = "bee1095c56ac76cd799032a74bbb010b193274364dd8fdae3fb7b5768865d8dd";
        let base = target(&state, &[0, 0]);
        assert_eq!(native_capture_binding(&archive, &base), expected);
        assert_ne!(
            native_capture_binding(&archive, &target(&state, &[0, 1])),
            expected
        );
        let mut changed = state.clone();
        changed.snapshot_cutoff_seq = 1;
        assert_ne!(
            native_capture_binding(&archive, &target(&changed, &[0, 0])),
            expected
        );
        archive.graph.source_actor_id =
            Uuid::parse_str("ffffffff-ffff-4fff-8fff-ffffffffffff").unwrap();
        assert_ne!(native_capture_binding(&archive, &base), expected);
    }
    #[test]
    fn native_archive_report_binding_incomplete_and_foreign_ref_fail_closed() {
        use collab_engine::archive_history::*;
        let archive = policy_fixture();
        let limits = collab_engine::Limits::default();
        let binding = "0000000000000000000000000000000000000000000000000000000000000000";
        let mut report = NativeArchiveInventory {
            binding: binding.into(),
            schema_version: 1,
            complete: true,
            references: vec![RetainedReference {
                owner: NativeId {
                    client: 10,
                    clock: 0,
                },
                declaration: NativeId {
                    client: 10,
                    clock: 2,
                },
                kind: ReferenceKind::Document,
                certainty: ReferenceCertainty::FixedKind,
                value: "10000000-0000-4000-8000-000000000002".into(),
            }],
            unavailable: vec![],
            diagnostics: vec![],
            work: InventoryWork::default(),
        };
        // Pure policy: this does not certify the synthetic native payload.
        validate_native_inventory(&archive, &report, binding, &limits).unwrap();
        for kind in [UnavailableKind::Gc, UnavailableKind::Deleted] {
            report.unavailable = vec![UnavailableRange {
                id: NativeId {
                    client: 10,
                    clock: 4,
                },
                len: 1,
                kind,
            }];
            assert!(
                matches!(validate_native_inventory(&archive, &report, binding, &limits),
                Err(ArchiveError::Unsupported(message)) if message.contains("captured confirmation"))
            );
        }
        report.unavailable.clear();
        validate_native_inventory(&archive, &report, binding, &limits).unwrap();
        report.binding = "changed".into();
        assert!(matches!(
            validate_native_inventory(&archive, &report, binding, &limits),
            Err(ArchiveError::Invalid(_))
        ));
        report.binding = binding.into();
        report.complete = false;
        assert!(matches!(
            validate_native_inventory(&archive, &report, binding, &limits),
            Err(ArchiveError::Unsupported(_))
        ));
        report.complete = true;
        report.references[0].value = "ffffffff-ffff-4fff-8fff-ffffffffffff".into();
        assert!(matches!(
            validate_native_inventory(&archive, &report, binding, &limits),
            Err(ArchiveError::Unsupported(_))
        ));
        report.references[0].value = "10000000-0000-4000-8000-000000000002".into();
        report.references[0].certainty = ReferenceCertainty::Potential;
        assert!(matches!(
            validate_native_inventory(&archive, &report, binding, &limits),
            Err(ArchiveError::Unsupported(_))
        ));
    }
    #[test]
    fn native_archive_preserved_reports_reject_association_reference_and_unavailable_tamper() {
        use collab_engine::archive_history::*;
        let archive = policy_fixture();
        assert!(archive.graph.native_inventory.is_none()); // legacy missing Option
        assert!(inventory_index(None, archive.graph.states.len())
            .unwrap()
            .is_none());
        let report = NativeArchiveInventory {
            binding: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            schema_version: 1,
            complete: true,
            references: vec![RetainedReference {
                owner: NativeId {
                    client: 10,
                    clock: 0,
                },
                declaration: NativeId {
                    client: 10,
                    clock: 2,
                },
                kind: ReferenceKind::Document,
                certainty: ReferenceCertainty::FixedKind,
                value: "10000000-0000-4000-8000-000000000002".into(),
            }],
            unavailable: vec![UnavailableRange {
                id: NativeId {
                    client: 20,
                    clock: 0,
                },
                len: 2,
                kind: UnavailableKind::Gc,
            }],
            diagnostics: vec![],
            work: InventoryWork::default(),
        };
        assert!(same_inventory_semantics(&report, &report));
        assert!(inventory_index(Some(std::slice::from_ref(&report)), 2).is_err());
        assert!(inventory_index(Some(&[report.clone(), report.clone()]), 2).is_err());
        let mut rebound = report.clone();
        rebound.binding = "1111111111111111111111111111111111111111111111111111111111111111".into();
        assert!(!same_inventory_semantics(&report, &rebound));
        let mut changed = report.clone();
        changed.references[0].kind = ReferenceKind::Task;
        assert!(!same_inventory_semantics(&report, &changed));
        changed = report.clone();
        changed.unavailable[0].len = 1;
        assert!(!same_inventory_semantics(&report, &changed));
        changed = report.clone();
        changed.unavailable.clear();
        assert!(!same_inventory_semantics(&report, &changed));
        let mut preserved = archive;
        preserved.graph.native_inventory = Some(vec![report.clone(), rebound]);
        let encoded = serde_json::to_vec(&preserved.graph).unwrap();
        let decoded: Graph = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            decoded.native_inventory.as_ref().unwrap()[0].unavailable,
            report.unavailable
        );
        assert_eq!(
            decoded.native_inventory.as_ref().unwrap()[0].references,
            report.references
        );
    }
}
