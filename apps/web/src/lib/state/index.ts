/**
 * What screens use of the state: the stores, the model, and the errors.
 *
 * The implementations of the backend (`memory/`, later the one that calls
 * the server) are not exported from here. Only `src/app/backend.ts` chooses
 * one; a lint rule keeps every other file from importing them.
 */
export {
  BackendError,
  CARD_BATCH_LIMIT,
  isBackendError,
  isConflict,
  isOwnSpace,
  isPasswordSignInRequired,
  isSignedOut,
  permissionsOfRole,
  READ_ONLY_PERMISSIONS,
  ssoOrgOf,
  type Account,
  type AccountSettings,
  type AccountSettingsPatch,
  type AccountToday,
  type Attachment,
  type Backend,
  type BoardDraft,
  type BoardViewWriteResult,
  type CardMovesInput,
  type CardMovesOutput,
  type ContainerPermissions,
  type ContainerRole,
  type CopiedContainerOutput,
  type CopyToWorkspaceInput,
  type TodayCardItem,
  type TodayMeetingItem,
  type TodayReviewEntry,
  type TodayUnitEntry,
  type TodayWorkspaceMeta,
  type WorkspaceToday,
  type CardDetail,
  type CardDraft,
  type CardPatch,
  type CardSummary,
  type Category,
  type Change,
  type Comment,
  type CommentVersion,
  type DeletedItem,
  type AcceptedInvitation,
  type AccountDeletion,
  type InvalidInput,
  type LeavingOrganization,
  type LinkedIdentity,
  type InvitationPreview,
  type LoginOptions,
  type LoginOrganization,
  type NewAccount,
  type OrgKind,
  type OrgRole,
  type Registration,
  type SignInOrganization,
  type PublicLink,
  type UserOrganization,
  type WorkspaceInfo,
  type UploadedFile,
  type User,
  type ViewDraft,
} from './backend.ts';
export { IMPORT_FORMATS, emptyImportCounts } from './import-types.ts';
export type {
  ImportCounts,
  Imported,
  ImportFormat,
  ImportOptions,
  ImportSummary,
  ImportWarning,
  ImportWarningCode,
} from './import-types.ts';
export type { Registered } from './account-types.ts';
export { BoardStore, type CardDrop } from './board.svelte.ts';
export { clearCardsProperty, setCardsProperty } from './board-bulk.ts';
export { addViewAtEnd, duplicateView, moveView } from './view-moves.ts';
export { type Shareable } from './member-writes.ts';
export { boardPresence } from './board-presence.svelte.ts';
export type {
  BodyVersion,
  BodyVersionCause,
  BodyVersionContent,
  BodyVersions,
  KeptBodyVersion,
} from './body-versions.ts';
export { ACTIVITY_PAGE, type CardActivityKind, type CardActivityLine } from './activity-types.ts';
export { CardPreviews } from './card-previews.svelte.ts';
export { type CommentEdit } from './card-parts.ts';
export { CardStore } from './card.svelte.ts';
export { PageStore } from './page.svelte.ts';
export { DiaryStore, firstDiary, newDiary } from './diary.svelte.ts';
export { EntryStore } from './entry.svelte.ts';
export { DiaryCalendar } from './diary-calendar.svelte.ts';
export { MOODS, NOTE_MAX_CHARS } from './diary-types.ts';
export type {
  DayNotes,
  DiaryDay,
  DiaryEntry,
  DiaryEntryDetail,
  DiaryRun,
  Mood,
} from './diary-types.ts';
export { CollaborativeBodyState } from './collab-body.svelte.ts';
export { CommentThreadStore } from './comment-thread.svelte.ts';
export type { CommentThreadStore as PageComments } from './comment-thread.svelte.ts';
export { WikiStore, type PageLooks, type WikiLooks } from './wiki.svelte.ts';
export { createWiki } from './wiki-parts.ts';
export type { PageDraft, PageMove, WikiDraft, WikiPage, WikiPageDetail } from './wiki-types.ts';
export { CourseStore, type CourseLooks, type CourseStoreTab } from './course.svelte.ts';
export { CourseMeetingsState, type CourseEventWriteResult } from './course-meetings.svelte.ts';
export { CourseScheduleState, type ScheduleWriteResult } from './course-schedule.svelte.ts';
export { DeviceToday } from './device-today.svelte.ts';
export { LearningTodayStore } from './learning-today.svelte.ts';
export { TodayHubStore, type TodayBoard, type TodayDiary } from './today-hub.svelte.ts';
export {
  chooseTimeZone,
  followsDeviceTimeZone,
  syncDeviceTimeZone,
} from './time-zone-sync.svelte.ts';
export { UnitStore } from './unit.svelte.ts';
export type { CommentThreadStore as UnitComments } from './comment-thread.svelte.ts';
export { createCourse } from './course-parts.ts';
export { createFromTemplate, type ContainerDraft, type NewKind } from './new-container.ts';
export {
  AMOUNT_UNITS,
  ATTENDANCE_STATUSES,
  compareCourseExams,
  compareCourseMeetings,
  COURSE_EXAM_DESCRIPTION_MAX_CHARS,
  COURSE_EXAM_MAX_SCORE_LIMIT,
  COURSE_EXAM_TITLE_MAX_CHARS,
  COURSE_MEETING_LAST_MS,
  COURSE_MEETING_LOCATION_MAX_CHARS,
  COURSE_MEETING_MAX_UNITS,
  COURSE_MEETING_NOTES_MAX_CHARS,
  COURSE_MEETING_TITLE_MAX_CHARS,
  courseReviewIntervals,
  DEFAULT_REVIEW_INTERVALS,
  EXAM_SCORE_NOTE_MAX_CHARS,
  hasControlChars,
  hasDisallowedNoteControlChars,
  isAttendanceStatus,
  MAX_REVIEW_INTERVAL_DAYS,
  MAX_SCHEDULE_SPAN_DAYS,
  MIN_REVIEW_INTERVAL_DAYS,
  STUDY_DAYS_ALL,
  STUDY_DAYS_FRIDAY,
  STUDY_DAYS_MONDAY,
  STUDY_DAYS_SATURDAY,
  STUDY_DAYS_SUNDAY,
  STUDY_DAYS_THURSDAY,
  STUDY_DAYS_TUESDAY,
  STUDY_DAYS_WEDNESDAY,
  STUDY_DAYS_WEEKDAYS,
  STUDY_WEEKDAYS,
  suggestedNextIntervalDays,
  UNIT_AMOUNT_MAX,
  UNIT_AMOUNT_MIN,
  UNIT_NOTE_CONTENT_MAX_CHARS,
  UNIT_NOTE_KINDS,
  UNIT_PROGRESS_POSITION_MAX_CHARS,
  validateUnitNoteContent,
} from './course-types.ts';
export {
  courseReviews,
  CourseReviewsStore,
  todayReviews,
  TodayReviewsStore,
} from './unit-reviews.svelte.ts';
export type {
  AmountUnit,
  AttendanceStatus,
  CompletedUnitReview,
  CompleteUnitReview,
  CourseDraft,
  CourseExam,
  CourseExamPatch,
  CourseExamsApi,
  CourseMeeting,
  CourseMeetingPatch,
  CourseMeetingsApi,
  CourseProgressSummary,
  CourseReviews,
  CourseSchedule,
  CourseScheduleUpdate,
  CourseScheduleView,
  Courses,
  CourseUnit,
  CourseUnitDetail,
  CreateUnitNote,
  LearningTodayItem,
  ListCourseReviewsQuery,
  MeetingAttendance,
  MeetingAttendanceUpdate,
  MemberCourseProgress,
  MyExamScore,
  MyExamScoreUpdate,
  NewCourseExam,
  NewCourseMeeting,
  PatchUnitNote,
  RedistributeScheduleRequest,
  ReviewStatusFilter,
  ScheduledUnit,
  ScheduleUnitReview,
  StudyWeekdayKey,
  TodayReviewItem,
  UnitDraft,
  UnitKind,
  UnitMove,
  UnitNote,
  UnitNoteContentError,
  UnitNoteKind,
  UnitPatch,
  UnitProgress,
  UnitProgressUpdate,
  UnitReview,
  UnitScheduleUpdate,
  WorkspaceLearningToday,
} from './course-types.ts';
export type { PresenceSource, TreeItem, TreeSource } from './tree-source.ts';
export {
  changeName,
  confirmPassword,
  passwordChanged,
  setFirstPassword,
  rereadEmail,
} from './account.ts';
export type { EmailChange } from './email-change.ts';
export type {
  ApiToken,
  ApiTokens,
  ApiTokenScope,
  ApiTokenWorkspace,
  IssuedApiToken,
  MemberApiToken,
  NewApiToken,
  OrgApiTokens,
} from './api-tokens.ts';
export { whileHere } from './here.ts';
export { administers, isGuest, peopleOf, providePeople, type PeopleAccess } from './people.ts';
export { OrganizationNames, organizationNames } from './organization-names.svelte.ts';
export { BlockWatch } from './block-watch.svelte.ts';
export type { WatchedKind, Watches } from './watch-types.ts';
export { MAX_FAVORITES } from './favorite-types.ts';
export type { Favorite, FavoriteKind, Favorites } from './favorite-types.ts';
export { WorkspaceFavorites, workspaceFavorites } from './workspace-favorites.svelte.ts';
export {
  copyOutPlace,
  copyPlaces,
  forgetCopyRefusal,
  keepsCopiesIn,
  noteCopyOut,
} from './copy-out.svelte.ts';
export type {
  GuestContainerRole,
  Invitation,
  InvitationMail,
  People,
  WorkspaceMember,
  WorkspaceRole,
} from './people-types.ts';
export { newCardMaker, registerNewCard, type NewCardMaker } from './new-card.ts';
export {
  referenceTitles,
  ReferenceTitles,
  type ReferencePlaces,
} from './reference-titles.svelte.ts';
export {
  boardsToShow,
  embeddedBoards,
  EmbeddedBoards,
  viewsOfBoard,
  RELEASE_AFTER_MS,
  type HeldBoard,
} from './embedded-board.svelte.ts';
export type { LinkedBlock } from './references.ts';
export { noteMembership, readerOf } from './memberships.svelte.ts';
export {
  byCreation,
  byTitle,
  openToJoinOf,
  type SidebarCategory,
  type SidebarReader,
} from './order.ts';
export {
  TRANSFER_STAYS,
  UploadKept,
  type Saying,
  type Transfer,
  type TransferOptions,
  type UploadFailure,
} from './transfers.svelte.ts';
export {
  isOpen,
  leaveOrganization,
  startOrganization,
  type EnterStatus,
} from './workspace-entry.ts';
export {
  containerTrash,
  cardTrash,
  pageTrash,
  unitTrash,
  entryTrash,
  Trash,
  type TrashStatus,
} from './trash.svelte.ts';
export { nextCardNotices, type NextCardNotice } from './next-cards.svelte.ts';
export { setBoardDoneRule, setBoardDueProperty } from './board-rules.ts';
export {
  TimeTracker,
  timeTracker,
  type SwitchedTimerNotice,
  type TimeSessionWall,
} from './time.svelte.ts';
export type {
  CreateTimeEntryRequest,
  ListTimeEntriesQuery,
  StartTimeEntryRequest,
  StartedTimeEntry,
  TimeEntry,
  TimeEntryKind,
  TimeEntryKindTotals,
  TimeEntryList,
  TimeEntryPatch,
  TimeEntrySummary,
  TimeOfBackend,
  TimeTargetKind,
  TimeTracking,
} from './time-types.ts';
export {
  Workspace,
  type ActiveContainer,
  type LoadStatus,
  type Problem,
  type ProblemAction,
  type WorkspaceOptions,
} from './workspace.svelte.ts';
export { MAX_CACHED_BOARDS, BoardCache } from './board-cache.ts';
export { MAX_CACHED_WIKIS, WikiCache } from './wiki-cache.ts';
export {
  MAX_CACHED_WORKSPACES,
  WorkspaceCache,
  type WorkspaceSnapshot,
} from './workspace-cache.ts';
export type { CacheLimits } from './workspace-caches.ts';
