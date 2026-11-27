//! Cross-workspace and cross-organization container copy (`copy-to-workspace`).
//!
//! Copies a live `Board`, `Wiki`, or `Course` into another workspace opened by the
//! caller's session (including across organizations or into the caller's personal space),
//! respecting `organizations.allow_copy_out`, PostgreSQL RLS handle boundaries, and
//! target organization storage limits.

use std::collections::{HashMap, HashSet};

use fvoci_model::{
    Block, BlockError, BlockType, Container, ContainerKind, ContainerKinds, ContainerMember,
    ContainerVisibility, ContainersAndBlocks, CopiedContainerResult,
    CopyContainerToWorkspaceRequest, CourseScheduleUpdate, OrgKind, PageNode, PageTree, Renewed,
    Session, TEMPLATE_SCHEDULE_PROPERTY, ValidatedCourseSchedule, WorkspaceRole, add_days, is_id,
    renew_containers_and_blocks_ids_told,
};
use fvoci_store::{CopyChanges, StreamBatch, StreamCreateError};
use tokio::sync::mpsc;

use super::accept_language::Reader;
use super::container_copies::{counted_as, titles_in};
use super::loose_objects::WrittenPlace;
use super::pages::refused_without_member;
use super::templates::{TEMPLATE_MARKS, changes_of_copy};
use super::{App, MAX_REFERENCE_IDS};
use crate::auth::now_millis;
use crate::error::{AppError, Reason};
use crate::metrics::{ContainerWork, CourseSource};

mod sanitize;

use sanitize::{build_member_name_map, sanitize_copied_blocks};

const COPY_STREAM_BATCH: usize = 64;

struct SourceSnapshot {
    container: Container,
    blocks: Vec<Block>,
    schedule: Option<CourseScheduleUpdate>,
    external_titles: HashMap<String, String>,
    member_names: HashMap<String, String>,
}

struct AuthorizedCopy {
    source_org_app: App,
    source_app: App,
    source_org_id: String,
    source_kind: OrgKind,
    target_app: App,
    target_org_id: String,
    target_ws_id: String,
}

impl App {
    /// Copies a shared container (`Board`, `Wiki`, or `Course`) into `targetWorkspaceId`
    /// across workspaces or organizations (`Personal <-> Shared`), preserving hierarchy,
    /// files, views, and course schedules while stripping comments and non-present member
    /// references.
    ///
    /// # Errors
    ///
    /// Returns bad-request, forbidden, or not-found errors when validation or access
    /// checks fail, and propagates storage or database errors.
    pub async fn copy_container_to_workspace(
        &self,
        session: &Session,
        source_ws_id: &str,
        container_id: &str,
        request: &CopyContainerToWorkspaceRequest,
        accept_language: Option<&str>,
    ) -> Result<CopiedContainerResult, AppError> {
        let auth = self
            .authorize_workspace_copy(session, source_ws_id, container_id, request)
            .await?;
        let snapshot = self
            .read_source_snapshot(
                &auth.source_org_app,
                &auth.source_app,
                source_ws_id,
                container_id,
                &session.user_id,
            )
            .await?;
        // A copy out of a shared organization is recorded before it is made, in the
        // organization it leaves: what cannot be recorded is not copied out, and a copy
        // that fails afterwards stays recorded. An entry too many is the safer error, for
        // the entry may be all that organization keeps of what left it.
        if auth.source_org_id != auth.target_org_id && auth.source_kind == OrgKind::Shared {
            auth.source_org_app
                .store()
                .record_container_copied_out(&session.user_id, container_id, &auth.target_org_id)
                .await?;
        }
        self.stage_and_commit_workspace_copy(
            &auth.source_app,
            &auth.target_app,
            &auth.target_org_id,
            &auth.target_ws_id,
            &session.user_id,
            request.title.as_deref(),
            accept_language,
            snapshot,
        )
        .await
    }

    async fn authorize_workspace_copy(
        &self,
        session: &Session,
        source_ws_id: &str,
        container_id: &str,
        request: &CopyContainerToWorkspaceRequest,
    ) -> Result<AuthorizedCopy, AppError> {
        let target_ws_id = request.target_workspace_id.trim();
        if target_ws_id.is_empty() {
            return Err(AppError::bad_request("targetWorkspaceId is required")
                .with_field("targetWorkspaceId"));
        }
        if !is_id(target_ws_id) {
            return Err(AppError::not_found("workspace"));
        }
        let source_entered = self
            .enter_workspace(session, source_ws_id)
            .await?
            .ok_or_else(|| AppError::not_found("board"))?;
        let copyable_kinds = ContainerKinds::SHARED;
        let source_app = source_entered
            .app
            .clone()
            .in_workspace(source_ws_id)
            .reaching(copyable_kinds);
        let access = source_app
            .container_access(&session.user_id, container_id)
            .await?
            .ok_or_else(|| AppError::not_found("board"))?;
        if !access.grant.sees() || access.grant.manages_only() {
            return Err(AppError::not_found("board"));
        }
        if !access.actor.works_there()
            || (!access.container.is_template && !access.has_membership())
        {
            return Err(AppError::forbidden("access denied to copy board"));
        }
        let target_entered = self
            .enter_workspace(session, target_ws_id)
            .await?
            .ok_or_else(|| AppError::not_found("workspace"))?;
        if target_entered.role == WorkspaceRole::Guest {
            return Err(AppError::not_found("workspace"));
        }
        let source_org_id = source_app
            .store()
            .tenant()
            .org_id()
            .ok_or_else(|| AppError::not_found("board"))?;
        let target_org_id = target_entered
            .app
            .store()
            .tenant()
            .org_id()
            .ok_or_else(|| AppError::not_found("workspace"))?;
        if source_org_id != target_org_id && source_entered.kind == OrgKind::Shared {
            let org = source_entered.app.organization().await?;
            if !org.allow_copy_out {
                return Err(AppError::forbidden(
                    "copying containers outside this organization is disabled",
                )
                .with_reason(Reason::CopyOutDisabled));
            }
        }
        Ok(AuthorizedCopy {
            source_org_app: source_entered.app,
            source_app,
            source_org_id,
            source_kind: source_entered.kind,
            target_app: target_entered
                .app
                .in_workspace(target_ws_id)
                .reaching(copyable_kinds),
            target_org_id,
            target_ws_id: target_ws_id.to_owned(),
        })
    }

    async fn read_source_snapshot(
        &self,
        source_org_app: &Self,
        source_app: &Self,
        source_ws_id: &str,
        container_id: &str,
        user_id: &str,
    ) -> Result<SourceSnapshot, AppError> {
        source_app.write_container_bodies(container_id).await?;
        let container = source_app.container(container_id).await?;
        if container.kind == ContainerKind::Diary {
            return Err(AppError::not_found("board"));
        }
        let mut blocks = source_app.blocks_for_container(container_id).await?;
        blocks.retain(|b| b.block_type != BlockType::Comment && b.deleted_at.is_none());
        blocks.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));

        let schedule = read_source_course_schedule(source_app, &container).await?;
        let internal_ids: HashSet<&str> = blocks.iter().map(|b| b.id.as_str()).collect();
        let mut external_ref_ids = Vec::new();
        let mut seen_refs = HashSet::new();
        for block in &blocks {
            if let Some(body) = &block.body {
                for ref_id in body.reference_ids() {
                    if !internal_ids.contains(ref_id)
                        && is_id(ref_id)
                        && seen_refs.insert(ref_id.to_owned())
                    {
                        external_ref_ids.push(ref_id.to_owned());
                    }
                }
            }
        }
        // A reference to something outside the copy becomes its title, as the copier is
        // told it: of a board, wiki or course they are a member of, as only a member
        // copies out (never of a diary, nor of an open board they only read).
        let ref_app = source_org_app
            .clone()
            .in_workspace(source_ws_id)
            .reaching(ContainerKinds::SHARED);
        let mut membership: HashMap<String, bool> = HashMap::new();
        let mut external_titles = HashMap::new();
        for chunk in external_ref_ids.chunks(MAX_REFERENCE_IDS) {
            for linked in ref_app.resolve_references(user_id, chunk).await? {
                let member = if let Some(member) = membership.get(&linked.container_id) {
                    *member
                } else {
                    let member = ref_app
                        .container_access(user_id, &linked.container_id)
                        .await?
                        .is_some_and(|access| access.has_membership());
                    membership.insert(linked.container_id.clone(), member);
                    member
                };
                if member {
                    external_titles.insert(linked.id, linked.title);
                }
            }
        }

        let members = source_org_app.workspace_members(source_ws_id).await?;
        let member_names = build_member_name_map(&members);

        Ok(SourceSnapshot {
            container,
            blocks,
            schedule,
            external_titles,
            member_names,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "target staging and single-transaction commit require source/target handles and snapshot"
    )]
    async fn stage_and_commit_workspace_copy(
        &self,
        source_app: &Self,
        target_app: &Self,
        target_org_id: &str,
        target_ws_id: &str,
        user_id: &str,
        custom_title: Option<&str>,
        accept_language: Option<&str>,
        snapshot: SourceSnapshot,
    ) -> Result<CopiedContainerResult, AppError> {
        let now = now_millis();
        let reader = target_app.reader_at(user_id, accept_language, now).await?;
        let changes = changes_of_copy(&snapshot.container, reader.today.as_deref());
        let source_id = snapshot.container.id.clone();
        let source_is_template = snapshot.container.is_template;
        let kind = snapshot.container.kind;

        if kind == ContainerKind::Wiki {
            check_wiki_copy_limits(target_app, &snapshot.blocks)?;
        }
        let prepared = build_copied_container(
            snapshot.container,
            target_ws_id,
            user_id,
            custom_title,
            &reader,
            changes,
            now,
        )?;
        let schedules = build_copied_schedules(snapshot.schedule, changes.days_later)?;
        let (mut containers, mut blocks, all_copies, new_container_id) =
            renew_copied_stream(prepared, snapshot.blocks)?;
        let referenced_files = collect_referenced_files(&blocks);
        // Read before any file is written: nothing written is left to the housekeeping.
        let present_users = target_present_users(target_app, target_ws_id, user_id).await?;
        let (file_map, staged_files, written_places) = target_app
            .pre_copy_files_to_container(
                source_app,
                &source_id,
                target_org_id,
                &new_container_id,
                &referenced_files,
                user_id,
            )
            .await?;
        let missing_files_count = referenced_files
            .iter()
            .filter(|fid| !file_map.contains_key(fid.as_str()))
            .count();
        if let Err(error) = sanitize_copied_blocks(
            &mut blocks,
            &containers[0],
            &file_map,
            &all_copies,
            &snapshot.external_titles,
            &snapshot.member_names,
            &present_users,
            changes.days_later,
            reader.language,
            user_id,
            now,
        ) {
            release_written_places(written_places).await;
            return Err(error);
        }
        let ordered_blocks = order_parents_before_children(blocks, &new_container_id);
        let block_ids: Vec<String> = ordered_blocks.iter().map(|b| b.id.clone()).collect();
        let members = commit_copied_stream(
            target_app,
            &mut containers,
            &staged_files,
            &schedules,
            user_id,
            ordered_blocks,
            written_places,
        )
        .await?;
        let course_source = if source_is_template {
            CourseSource::Template
        } else {
            CourseSource::Duplicate
        };
        finish_workspace_copy(
            target_app,
            target_ws_id,
            user_id,
            kind,
            course_source,
            containers,
            block_ids,
            members,
            missing_files_count,
        )
        .await
    }
}

type RenewedStream = (Vec<Container>, Vec<Block>, HashMap<String, String>, String);

fn renew_copied_stream(
    container: Container,
    blocks: Vec<Block>,
) -> Result<RenewedStream, AppError> {
    let Renewed {
        containers_and_blocks: ContainersAndBlocks { containers, blocks },
        renamed,
    } = renew_containers_and_blocks_ids_told(
        ContainersAndBlocks {
            containers: vec![container],
            blocks,
        },
        |_| true,
    );
    let id = containers
        .first()
        .map(|c| c.id.clone())
        .ok_or_else(|| AppError::internal("missing renewed container"))?;
    Ok((containers, blocks, renamed, id))
}

fn check_wiki_copy_limits(target_app: &App, blocks: &[Block]) -> Result<(), AppError> {
    let limits = target_app.limits().pages;
    let tree: PageTree = blocks
        .iter()
        .filter(|b| b.block_type == BlockType::Page)
        .cloned()
        .map(PageNode::from)
        .collect();
    if tree.len() > usize::try_from(limits.pages).unwrap_or(usize::MAX) {
        return Err(AppError::from(BlockError::TooManyPages {
            limit: limits.pages,
        }));
    }
    for block in blocks.iter().filter(|b| b.block_type == BlockType::Page) {
        if tree.depth(&block.id) > usize::try_from(limits.depth).unwrap_or(usize::MAX) {
            return Err(refused_without_member(fvoci_store::StoreError::from(
                BlockError::PageTooDeep {
                    limit: limits.depth,
                },
            )));
        }
    }
    Ok(())
}

async fn target_present_users(
    target_app: &App,
    target_ws_id: &str,
    user_id: &str,
) -> Result<HashSet<String>, AppError> {
    let target_members = target_app.workspace_members(target_ws_id).await?;
    let mut present_users: HashSet<String> = target_members
        .into_iter()
        .filter(|m| m.role != WorkspaceRole::Guest)
        .map(|m| m.user_id)
        .collect();
    present_users.insert(user_id.to_owned());
    Ok(present_users)
}

#[expect(
    clippy::too_many_arguments,
    reason = "post-commit sidebar placement, metrics, and broadcasts share the committed output"
)]
async fn finish_workspace_copy(
    target_app: &App,
    target_ws_id: &str,
    user_id: &str,
    kind: ContainerKind,
    course_source: CourseSource,
    containers: Vec<Container>,
    block_ids: Vec<String>,
    members: Vec<fvoci_model::ContainerMember>,
    missing_files_count: usize,
) -> Result<CopiedContainerResult, AppError> {
    let created_container = containers
        .into_iter()
        .next()
        .ok_or_else(|| AppError::internal("missing committed container"))?;
    // The copy is committed: a sidebar that could not file it is no failure of the copy,
    // which would be made twice if the caller were told to try again.
    if let Err(error) = target_app
        .add_containers_to_default_category(
            user_id,
            target_ws_id,
            std::slice::from_ref(&created_container.id),
        )
        .await
    {
        tracing::error!(
            %error,
            container_id = %created_container.id,
            "the copied container was not filed under the default category"
        );
    }
    target_app
        .metrics()
        .container_copied(ContainerWork::Duplicate, counted_as(kind));
    if kind == ContainerKind::Course {
        target_app.metrics().course_created(course_source);
    }
    announce_workspace_copy(
        target_app,
        target_ws_id,
        created_container.clone(),
        block_ids,
        members,
    );
    Ok(CopiedContainerResult {
        container: created_container,
        missing_files_count,
    })
}

async fn read_source_course_schedule(
    source_app: &App,
    container: &Container,
) -> Result<Option<CourseScheduleUpdate>, AppError> {
    if container.kind != ContainerKind::Course {
        return Ok(None);
    }
    if let Some(schedule) = source_app
        .store()
        .get_course_schedule(&container.id)
        .await?
    {
        Ok(Some(CourseScheduleUpdate {
            start_date: schedule.start_date,
            target_date: schedule.target_date,
            study_days: i32::from(schedule.study_days),
            auto_assign: false,
        }))
    } else {
        let Some(raw) = container.properties.get(TEMPLATE_SCHEDULE_PROPERTY) else {
            return Ok(None);
        };
        let mut parsed: CourseScheduleUpdate = serde_json::from_value(raw.clone())
            .map_err(|err| AppError::bad_request(format_args!("invalid course schedule: {err}")))?;
        parsed.auto_assign = false;
        Ok(Some(parsed))
    }
}

fn build_copied_schedules(
    schedule: Option<CourseScheduleUpdate>,
    days_later: i64,
) -> Result<Vec<(usize, ValidatedCourseSchedule)>, AppError> {
    let Some(mut schedule) = schedule else {
        return Ok(Vec::new());
    };
    if days_later != 0 {
        if let Some(shifted) = add_days(&schedule.start_date, days_later) {
            schedule.start_date = shifted;
        }
        if let Some(shifted) = add_days(&schedule.target_date, days_later) {
            schedule.target_date = shifted;
        }
    }
    let validated = schedule.check_request()?;
    Ok(vec![(0, validated)])
}

fn build_copied_container(
    mut container: Container,
    target_ws_id: &str,
    user_id: &str,
    custom_title: Option<&str>,
    reader: &Reader,
    changes: CopyChanges<'_>,
    now: i64,
) -> Result<Container, AppError> {
    let source_was_template = container.is_template;
    for mark in changes.without_properties {
        container.properties.remove(*mark);
    }
    for mark in TEMPLATE_MARKS {
        container.properties.remove(mark);
    }
    let title = match custom_title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(explicit) => explicit.to_owned(),
        None if source_was_template => container.title,
        None => (titles_in(reader.language, container.kind).copy_of)(&container.title),
    };
    target_ws_id.clone_into(&mut container.workspace_id);
    container.visibility = ContainerVisibility::Private;
    container.title = title;
    container.is_template = false;
    container.is_builtin = false;
    container.template_version = 0;
    user_id.clone_into(&mut container.created_by);
    user_id.clone_into(&mut container.modified_by);
    container.created_at = now;
    container.updated_at = now;
    container.deleted_at = None;
    if container.id.is_empty() {
        container.id = fvoci_model::new_id();
    }
    container.validate().map_err(AppError::bad_request)?;
    Ok(container)
}

fn collect_referenced_files(blocks: &[Block]) -> Vec<String> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for block in blocks {
        for file_id in block.shown_files() {
            if !file_id.is_empty() && seen.insert(file_id.clone()) {
                files.push(file_id);
            }
        }
    }
    files
}

fn order_parents_before_children(blocks: Vec<Block>, container_id: &str) -> Vec<Block> {
    let all_ids: HashSet<String> = blocks.iter().map(|b| b.id.clone()).collect();
    let mut emitted = HashSet::with_capacity(blocks.len());
    let mut ordered = Vec::with_capacity(blocks.len());
    let mut remaining = blocks;
    while !remaining.is_empty() {
        let before = remaining.len();
        let mut deferred = Vec::new();
        for block in remaining {
            let ready = block.parent_id.is_empty()
                || block.parent_id == container_id
                || !all_ids.contains(&block.parent_id)
                || emitted.contains(&block.parent_id);
            if ready {
                emitted.insert(block.id.clone());
                ordered.push(block);
            } else {
                deferred.push(block);
            }
        }
        if deferred.len() == before {
            ordered.extend(deferred);
            break;
        }
        remaining = deferred;
    }
    ordered
}

async fn commit_copied_stream(
    target_app: &App,
    containers: &mut [Container],
    staged_files: &[fvoci_model::FileRecord],
    schedules: &[(usize, ValidatedCourseSchedule)],
    user_id: &str,
    ordered_blocks: Vec<Block>,
    written_places: Vec<WrittenPlace>,
) -> Result<Vec<ContainerMember>, AppError> {
    let (tx, rx) = mpsc::channel(2);
    let producer = tokio::spawn(async move {
        for chunk in ordered_blocks.chunks(COPY_STREAM_BATCH) {
            if tx.send(StreamBatch::Batch(chunk.to_vec())).await.is_err() {
                return;
            }
        }
        let _ = tx.send(StreamBatch::Done).await;
    });

    let write_res = target_app
        .store()
        .create_imported_containers_blocks_files_members_and_schedules_streamed(
            containers,
            staged_files,
            &[],
            schedules,
            user_id,
            rx,
        )
        .await;
    let _ = producer.await;

    match write_res {
        Ok(members) => {
            for place in written_places {
                place.settled_in_transaction();
            }
            Ok(members)
        }
        Err(error) => {
            release_written_places(written_places).await;
            Err(match error {
                StreamCreateError::Producer(err) => err,
                StreamCreateError::Incomplete => {
                    AppError::internal("container copy stream ended before completion")
                }
                StreamCreateError::Store(err) => AppError::from(err),
            })
        }
    }
}

async fn release_written_places(places: Vec<WrittenPlace>) {
    for place in places {
        place.give_up().await;
    }
}

fn announce_workspace_copy(
    target_app: &App,
    target_ws_id: &str,
    container: Container,
    block_ids: Vec<String>,
    members: Vec<ContainerMember>,
) {
    let app = target_app.clone();
    let ws_id = target_ws_id.to_owned();
    target_app.spawn_background(async move {
        app.ws()
            .broadcast_container_change(&ws_id, &container)
            .await;
        for member in &members {
            app.ws()
                .broadcast_member_change(&ws_id, &member.container_id, member)
                .await;
        }
    });
    target_app.announce_blocks(target_ws_id, block_ids);
}
