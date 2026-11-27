//! Wire types shared by the fvoci server and web client.
//!
//! The types here are what the server and its clients exchange as JSON (using camelCase
//! and `snake_case` wire spellings as documented on each type). The crate does
//! no I/O. Functions that stamp a time take the current time in milliseconds since the
//! Unix epoch as an argument; the one place that reads a clock is [`new_id`], whose
//! identifiers begin with the time they were made.

mod account_deletion;
mod allowlist;
mod api_token;
mod archive;
mod auth;
mod block;
mod block_ids;
mod board_ops;
mod body;
mod body_markdown;
mod body_version;
mod card;
mod card_property;
mod category;
mod client_config;
mod comment_version;
mod container;
mod container_teams;
mod containers_and_blocks;
mod course_exam;
mod course_meeting;
mod course_schedule;
mod date_value;
mod deleted_item;
mod diary;
mod file_lease;
mod file_version;
mod files;
mod id;
mod invitation;
mod linked;
mod name;
mod notification;
mod oidc;
mod org_admin;
mod org_kind;
mod organization;
mod overview;
mod page;
mod page_tree;
mod person_name;
mod problem;
mod public_containers;
mod public_link;
mod refusal;
mod relation;
mod search;
mod serde_util;
mod settings;
mod subscription;
mod table_grid;
mod team;
mod time_entry;
mod today;
mod unit;
mod unit_progress;
mod unit_review;
mod upload;
mod user;
mod view_fields;
mod workspace;

pub use account_deletion::{
    ACCOUNT_DELETION_GRACE_DAYS, AccountDeletion, AccountDeletionAsked, DeleteAccountRequest,
    DeletionProof, Leaving, LeavingOrganization, OrganizationName,
};
pub use allowlist::{body_node_types, is_allowed_href, is_file_id};
pub use api_token::{
    API_TOKEN_DEFAULT_DAYS, API_TOKEN_MAX_DAYS, API_TOKEN_MAX_LIVE, API_TOKEN_NAME_MAX_CHARS,
    API_TOKEN_PREFIX, API_TOKEN_SHOWN_CHARS, ApiToken, ApiTokenScope, ApiTokenWorkspace,
    IssuedApiToken, MemberApiToken, NewApiToken, ParseApiTokenScopeError,
};
pub use archive::{ArchiveHeader, ArchiveLine};
pub use auth::{
    AuthParamError, ChangePasswordRequest, CheckedAccount, ConfirmPasswordRequest,
    DISPLAY_NAME_MAX_LENGTH, LoginRequest, MAXIMUM_EMAIL_BYTES, MAXIMUM_PASSWORD_BYTES,
    MINIMUM_PASSWORD_LENGTH, NewAccount, NewOrganization, ORGANIZATION_NAME_MAX_LENGTH,
    RegisterRequest, Registration, SetPasswordRequest, USERNAME_MAX_LENGTH, check_new_password,
    display_name, is_email_valid, organization_name,
};
pub use block::{
    BLOCK_FIELDS_MAX_CHARS, BLOCK_TITLE_MAX_CHARS, Block, BlockError, BlockPatch, BlockPatchBatch,
    BlockType, CARD_FIELDS_MAX_CHARS, COMMENT_AGENT_TOKEN_NAME, ParseBlockTypeError,
    PropertyChanges, stamp_modification_metadata,
};
pub use block_ids::{
    UnmatchedRelation, collect_block_references, generate_block_ids, renew_block_ids,
    rewrite_block_ids,
};
pub use board_ops::{
    BlankBoardScaffolding, BoardLocale, CardBatchIds, CardDuePoint, CardMovesRequest,
    CardMovesResult, CategoryContainerUpdate, CategoryPatch, ContainerPermissions,
    CopiedContainerResult, CopyContainerToWorkspaceRequest, DoneWhen, DuplicateAs, MAX_CARD_BATCH,
    NewCardAttachment, RestoredCards, default_blank_board_scaffolding, extract_card_due_point,
    is_card_done,
};
pub use body::{
    BODY_MAX_BYTES, BODY_MAX_DEPTH, BODY_MAX_NODES, BODY_VERSION, Body, BodyChange, BodyError,
    BodySummary, TaskCount,
};
pub use body_markdown::{ComplexBlock, ComplexBlockReason};
pub use body_version::{
    BodyVersion, BodyVersionCause, BodyVersionContent, KeptBodyVersion, KeptOutcome,
    ParseBodyVersionCauseError,
};
pub use card::{
    CARD_PREVIEW_TEXT_LENGTH, Card, CardError, CardPatch, CardPreview, CompactCard, MAX_CARD_DEPTH,
};
pub use card_property::{
    CardPropertyDefinition, CardPropertyError, DATE_PROPERTY_TYPE, ParsePropertyColorError,
    ParsePropertyTypeError, PersonProperties, PropertyColor, PropertyOption, PropertyType,
    assigned_users, clear_values_of_another_kind, validate_block_properties,
    validate_card_property_definitions, validate_card_property_kinds,
    validate_card_property_values,
};
pub use category::{
    CATEGORY_CONTAINERS_SORT_ORDER_GAP, Category, CategoryContainerMetadata, CategoryContainers,
    CategoryError, CategoryType, ContainerCategoryWebsocketData, DEFAULT_CATEGORY_NAME,
    SidebarVisibility,
};
pub use client_config::ClientConfig;
pub use comment_version::CommentVersion;
pub use container::{
    Container, ContainerError, ContainerKind, ContainerKinds, ContainerMember,
    ContainerMemberPatch, ContainerPatch, ContainerRole, ContainerSearchField, ContainerVisibility,
    MemberRef, MinimumRole, ParseContainerKindError, ParseContainerSearchFieldError,
    ParseContainerVisibilityError,
};
pub use container_teams::{ContainerTeam, ContainerTeamRole};
pub use containers_and_blocks::{
    ContainersAndBlocks, ContainersAndBlocksError, DeleteContainersAndBlocks,
    PatchContainersAndBlocks, Renewed, generate_containers_and_blocks_ids,
    renew_containers_and_blocks_ids, renew_containers_and_blocks_ids_told,
};
pub use course_exam::{
    COURSE_EXAM_DESCRIPTION_MAX_CHARS, COURSE_EXAM_MAX_SCORE_LIMIT, COURSE_EXAM_TITLE_MAX_CHARS,
    CourseExam, CourseExamChange, CourseExamPatch, EXAM_SCORE_NOTE_MAX_CHARS, ExamDateChange,
    MyExamScore, MyExamScoreUpdate, NewCourseExam, ValidatedMyExamScore, ValidatedNewExam,
};
pub use course_meeting::{
    AttendanceStatus, COURSE_MEETING_LOCATION_MAX_CHARS, COURSE_MEETING_MAX_UNITS,
    COURSE_MEETING_NOTES_MAX_CHARS, COURSE_MEETING_TITLE_MAX_CHARS, CourseMeeting,
    CourseMeetingChange, CourseMeetingPatch, MeetingAttendance, MeetingAttendanceUpdate,
    NewCourseMeeting, OptionalIdChange, UnknownAttendanceStatus, ValidatedNewMeeting,
};
pub use course_schedule::{
    AmountUnit, CourseOutlineNodes, CourseSchedule, CourseScheduleUpdate, CourseScheduleView,
    FieldChange, LearningTodayItem, MAX_SCHEDULE_SPAN_DAYS, ParseAmountUnitError, RedistributePlan,
    RedistributeScheduleRequest, STUDY_DAYS_ALL, STUDY_DAYS_FRIDAY, STUDY_DAYS_MONDAY,
    STUDY_DAYS_SATURDAY, STUDY_DAYS_SUNDAY, STUDY_DAYS_THURSDAY, STUDY_DAYS_TUESDAY,
    STUDY_DAYS_WEDNESDAY, STUDY_DAYS_WEEKDAYS, ScheduledUnit, StudyDays,
    TEMPLATE_SCHEDULE_PROPERTY, UNIT_AMOUNT_FIELD, UNIT_AMOUNT_MAX, UNIT_AMOUNT_MIN,
    UNIT_AMOUNT_UNIT_FIELD, UNIT_SCHEDULED_DATE_FIELD, UnitScheduleUpdate, UnitWeight,
    ValidatedCourseSchedule, WorkspaceLearningToday, build_course_schedule_view,
    build_workspace_learning_today, distribute_units, outline_units, shift_unit_scheduled_date,
    study_dates_in_range, today_in_time_zone,
};
pub use date_value::{
    DateValueError, add_days, check_date_value, date_property_ids, day_at, days_between,
    is_calendar_date, move_dates, utc_date,
};
pub use deleted_item::DeletedItem;
pub use diary::{
    DAYS_PER_LIST, DIARIES_PER_SPACE, DeletedDiaryEntry, DiaryDate, DiaryDay, DiaryEntry,
    DiaryEntryPatch, DiaryMaking, DiaryRun, ENTRIES_PER_DIARY, ENTRY_BODY_UPDATED_AT_FIELD,
    ENTRY_DATE_FIELD, ENTRY_DAYS_AHEAD, ENTRY_MOOD_FIELD, ENTRY_NOTE_MAX_CHARS, ENTRY_PLACE_FIELD,
    ENTRY_WEATHER_FIELD, MonthDay, Mood, MoodChange, NewDiaryEntry, ParseDiaryDateError,
    ParseMonthDayError, ParseMoodError, entry_body_updated_at, entry_date, entry_mood,
    is_empty_entry,
};
pub use file_lease::{FileLease, FileLeaseRequest, HeldFileLease};
pub use file_version::{FileVersion, FileVersionBase, NewFileVersion};
pub use files::{FileKey, FileRecord, ListedFile, is_storage_path};
pub use id::{is_id, new_id};
pub use invitation::{
    AcceptInvitation, AcceptedInvitation, GUEST_INVITATION_USES, INVITATION_LIFETIME_MILLIS,
    Invitation, InvitationMail, InvitationPreview, InvitationToken, NewInvitation,
};
pub use linked::{LinkedBlock, LinkedKind};
pub use name::{Named, required_name};
pub use notification::{
    Notification, NotificationKind, NotificationSubject, UnknownNotificationKind,
    UnreadNotifications,
};
pub use oidc::{
    LinkedIdentity, LoginOptions, LoginOrganization, OIDC_CLIENT_FIELD_MAX_BYTES,
    OIDC_ISSUER_MAX_BYTES, OidcFormerProvider, OidcFormerProviderEnd, OidcPendingProvider,
    OidcProvider, OidcProviderSettings, OidcStart, OidcStartRequest, UnlinkRequest,
};
pub use org_admin::{
    AuditAction, AuditPerson, AuditRecord, AuditRecordActor, MAX_NAME_CHARS, OrgMember,
    OrgRoleChange, OrgWorkspace, Organization, OrganizationPatch, UnknownAuditAction,
    WorkspaceName,
};
pub use org_kind::{OrgKind, UnknownOrgKind};
pub use organization::{
    OrgRole, UnknownRole, UserOrganization, UserWorkspace, WorkspaceMember, WorkspaceRole,
    WorkspaceRoleChange,
};
pub use overview::{OverviewContainer, OverviewPerson, OverviewWorkspace, WorkspaceOverview};
pub use page::{
    DeletedPage, NewPage, OrderError, PAGE_ICON_FIELD, PAGE_ORDER_FIELD, Page, PageDuplicate,
    PageMove, PageNode, PagePatch, order_between, sort_pages,
};
pub use page_tree::{DEFAULT_PAGE_DEPTH, DEFAULT_WIKI_PAGES, PageLimits, PageTree, Place};
pub use person_name::{
    Credential, MAX_CREDENTIALS, NAME_PART_MAX_LENGTH, NameShown, PersonName, PersonNameChange,
    Title, UnknownCode,
};
pub use problem::{InvalidInput, PROBLEM_MEDIA_TYPE, Problem, json_pointer, pointer_token};
pub use public_containers::PublicContainers;
pub use public_link::{PublicLink, PublicLinkChange};
pub use refusal::{NAME_SHOWN_CHARS, Refusal, RefusalReason, Refusals, shown_name};
pub use relation::{
    ParseRelationKindError, RELATION_LAG_MAX, RELATION_LAG_MIN, RELATION_LIMIT, RelationError,
    RelationKind, RelationLink, RelationLinkError, RelationProblem, RelationProperties,
    check_relation_value, closes_relation_cycle, parse_relation_link, relation_link_card,
    relation_links_to_value, relation_property_ids, tidy_copied_relations,
};
pub use search::{SearchHit, SearchKind, SnippetPart, UnknownSearchKind};
pub use settings::{
    CLIENT_STATE_MAX_ENTRIES, CLIENT_STATE_MAX_NAME_LENGTH, CLIENT_STATE_MAX_VALUE_BYTES, CardOpen,
    ClientStateError, Density, HourCycle, Language, ParseSettingError, SettingName, SettingsError,
    Theme, TimeZone, UserSettings, UserSettingsPatch, WeekStart, check_client_state_name,
    check_client_state_value,
};
pub use subscription::{Subscriber, Subscription, SubscriptionError};
pub use table_grid::{
    CellFit, Reach, RowFit, TABLE_MAX_COLUMNS, TABLE_MAX_ROWS, TableGrid, TableTooLarge,
};
pub use team::{Team, TeamMember, TeamName, team_name, team_name_key};
pub use time_entry::{
    ActiveTimeEntry, CreateTimeEntryRequest, MAX_TIME_MILLIS, ParseTimeEntryKindError,
    ParseTimeTargetKindError, StartTimeEntryRequest, StartedTimeEntry, TIME_ENTRY_NOTE_MAX_CHARS,
    TimeEntry, TimeEntryKind, TimeEntryKindTotals, TimeEntryList, TimeEntryPatch, TimeEntrySummary,
    TimeTargetKind,
};
pub use today::{
    AccountToday, TODAY_SECTION_LIMIT, TodayCardItem, TodayMeetingItem, TodayReviewEntry,
    TodayUnitEntry, TodayWorkspaceMeta, WorkspaceToday,
};
pub use unit::{
    CHAPTERS_PER_COURSE, COURSE_DEPTH, CourseLimits, DeletedUnit, NewUnit, NotUnitBlockError,
    UNITS_PER_COURSE, Unit, UnitKind, UnitMove, UnitNode, UnitPatch, sort_units,
};
pub use unit_progress::{
    CompletionChange, CourseProgressSummary, LastPositionChange, MemberCourseProgress,
    UNIT_PROGRESS_POSITION_MAX_CHARS, UnitProgress, UnitProgressChange, UnitProgressUpdate,
};
pub use unit_review::{
    COURSE_REVIEW_INTERVALS_PROPERTY, CompleteUnitReview, CompletedUnitReview, CreateUnitNote,
    DEFAULT_REVIEW_INTERVALS, MAX_REVIEW_INTERVAL_DAYS, MIN_REVIEW_INTERVAL_DAYS, NewUnitNote,
    NewUnitReview, ParseReviewStatusError, ParseUnitNoteKindError, PatchUnitNote,
    ReviewStatusFilter, ScheduleUnitReview, TodayReviewItem, UNIT_NOTE_CONTENT_MAX_CHARS, UnitNote,
    UnitNoteChange, UnitNoteKind, UnitReview, course_review_intervals,
    suggested_next_interval_days,
};
pub use upload::{MAX_UPLOAD_PARTS, Upload, UploadPart, UploadPurpose};
pub use user::{ClientStatePatch, EndedSessionScope, InstanceRole, LoginOrigin, Me, Session, User};
pub use view_fields::{
    BoardViewDuplicate, BoardViewMove, BoardViewWrite, FILTER_DATE_TODAY, FILTER_PERSON_ME,
    GanttZoom, NewBoardView, ParseViewTypeError, SHARED_VIEW_MEMBERS, VIEW_ORDER_MAX_CHARS,
    ViewFieldsError, ViewType, drop_malformed_view_fields, gantt_dependency_property_ids,
    place_view_at, sort_board_views, spread_view_orders, validate_view_fields,
    validate_view_filter_values, view_order_of,
};
pub use workspace::{GuestInvitationsChange, Workspace};

/// A JSON object with free-form keys, as used for block fields and board properties.
pub type JsonMap = serde_json::Map<String, serde_json::Value>;
