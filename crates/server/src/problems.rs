//! The kinds of problem the API answers with, and the pages that describe them.
//!
//! The `type` of a problem is `/problems/<name>` under the path the server is served
//! at, and that address answers with a page of text about the kind. The list is made
//! from [`ErrorKind::ALL`] and [`Reason::ALL`], which is all there is: an error has the
//! type of its reason, or else of its kind.

use std::fmt::Write as _;

use axum::Router;
use axum::extract::{OriginalUri, Path};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
use axum::http::{HeaderValue, Uri};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::error::{AppError, ErrorKind, PROBLEMS_PATH, Reason, type_reference};
use crate::error_response::problems_only;

mod sign_in;
use sign_in::describe_sign_in;

/// What a kind of problem is the kind of: every error either has a reason, and is the
/// problem of that reason whatever its status, or has none and is the problem of its
/// status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// The errors of this status that have no reason.
    Status(u16),
    /// The errors with this `reason`; the description says which statuses they come
    /// with.
    Reason(&'static str),
}

/// One kind of problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProblemType {
    /// The last part of the `type`: `not-found`, `last-admin`.
    pub name: String,
    /// The `title` of every problem of the kind.
    pub title: &'static str,
    /// What the kind is the kind of.
    pub basis: Basis,
    /// What the kind means and what a client can do about it, in English.
    pub description: &'static str,
}

impl ProblemType {
    /// The `type` of the problems of this kind, on a server at the root of its host.
    pub fn type_reference(&self) -> String {
        type_reference(&self.name)
    }

    /// The page about the kind, on a server that is served under `base_path`.
    fn page(&self, base_path: &str) -> String {
        let mut page = format!(
            "{}\n\ntype: {base_path}{}\n",
            self.title,
            self.type_reference()
        );
        // Writing to a string cannot fail.
        let _ = match self.basis {
            Basis::Status(status) => writeln!(page, "status: {status}"),
            Basis::Reason(reason) => writeln!(page, "reason: {reason}"),
        };
        let _ = writeln!(page, "\n{}", self.description);
        page
    }
}

impl From<ErrorKind> for ProblemType {
    fn from(kind: ErrorKind) -> Self {
        Self {
            name: kind.name().to_owned(),
            title: kind.title(),
            basis: Basis::Status(kind.status()),
            description: describe_kind(kind),
        }
    }
}

impl From<Reason> for ProblemType {
    fn from(reason: Reason) -> Self {
        Self {
            name: reason.name(),
            title: reason.title(),
            basis: Basis::Reason(reason.as_str()),
            description: describe_reason(reason),
        }
    }
}

/// Every kind of problem: those of the statuses, in their order, then those of the
/// reasons.
pub fn all() -> Vec<ProblemType> {
    let kinds = ErrorKind::ALL.into_iter().map(ProblemType::from);
    let reasons = Reason::ALL.iter().copied().map(ProblemType::from);
    kinds.chain(reasons).collect()
}

/// The kind of problem with this name, if there is one.
pub fn find(name: &str) -> Option<ProblemType> {
    all().into_iter().find(|problem| problem.name == name)
}

fn describe_kind(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::BadRequest => {
            "The request is malformed or asks for what cannot be: a body that is not the JSON \
             the operation takes, or a member, a parameter or a header with a value it cannot \
             have. Where the refusal is about something of the request, `errors` lists it; a \
             check of the request's shape lists everything it refuses at once."
        }
        ErrorKind::Unauthorized => {
            "The request has no session, or its session is over: sign in and send the request \
             again. The websocket endpoint answers so to a handshake that has no `Origin` \
             header and no bearer token. A shared board's link needs no session: what it does \
             not open is not found."
        }
        ErrorKind::Forbidden => {
            "The user can see what the request is about, but may not do this with it."
        }
        ErrorKind::NotFound => {
            "What the request is about does not exist, or the user may not know that it does: \
             the two are answered alike. A path that is no operation of the API is answered so \
             as well."
        }
        ErrorKind::MethodNotAllowed => {
            "The path exists, but not with this method. The `Allow` header lists the methods \
             it has."
        }
        ErrorKind::Conflict => {
            "The request is fine, but the state of things is against it: what it would make is \
             there already, or what it changes has changed since it was read. Read again and \
             decide anew."
        }
        ErrorKind::PreconditionFailed => {
            "A condition the request sets in its headers does not hold. The download of a \
             file answers so to an `If-Unmodified-Since` before the time of the answer."
        }
        ErrorKind::PayloadTooLarge => {
            "The body of the request, or what the request names, is larger than the server \
             takes. `limit` is the most it takes."
        }
        ErrorKind::RangeNotSatisfiable => {
            "The `Range` header of the request is no range of bytes, or the range is not in \
             the file. For a range that is not in the file, the `Content-Range` header of the \
             answer tells the file's length."
        }
        ErrorKind::RateLimited => {
            "Too many attempts. `retryAfter`, and the `Retry-After` header, say how many \
             seconds to wait before trying again."
        }
        ErrorKind::Internal => {
            "The server failed. The answer does not say how; `requestId` is the name of the \
             request in the server's log."
        }
        ErrorKind::ServiceUnavailable => {
            "The server cannot serve for now: what it depends on does not answer. The \
             readiness check (`GET /api/v1/ready`) answers so while the database or the \
             file storage cannot be used; `detail` names which."
        }
    }
}

fn describe_reason(reason: Reason) -> &'static str {
    match reason {
        Reason::Csrf => {
            "Status 403. A request that changes something lacks the header `X-Requested-With: XMLHttpRequest`, by which the server knows that no page of another site made a browser send it. A request with a bearer token needs none."
        }
        Reason::ForeignOrigin => {
            "Status 403. The request, or the websocket handshake, has an `Origin` header that is neither the server's own origin nor one the server was configured to accept."
        }
        Reason::InvalidCredentials => {
            "The user name, the email address or the password is wrong; the answer does not say which. Status 401 from a login. Status 400 from a change of the password, whose session is good: `errors` names `/oldPassword`."
        }
        Reason::InvitationRequired
        | Reason::InvitationUnavailable
        | Reason::TooManyRecipients
        | Reason::InvitationMailLimit
        | Reason::MailQueueFull => describe_invitation(reason).unwrap_or_default(),
        Reason::UsernameTaken => {
            "Status 409. Another user has the user name. `errors` names `/username`."
        }
        Reason::EmailTaken => {
            "Status 409. Another user has the email address. `errors` names `/email`."
        }
        Reason::LastAdmin => {
            "Status 409. The change would leave the board, the workspace or the server without an administrator. Make somebody else one first."
        }
        Reason::AccountDeletionPending => {
            "Status 401, with a session that is good: its account was asked to be deleted and waits. Every operation answers so but `getAccountDeletion`, which says until when it waits, `cancelAccountDeletion`, which puts the account in use again as it was, and `logout`."
        }
        Reason::SignInAgain => {
            "Status 403, asking to delete an account that has no password (`requestAccountDeletion`) or to give it its first one (`setPassword`): the session was not made through a provider in the last five minutes. Sign in through the provider again (`startOidcLogin`) and send the request from the session that comes of it."
        }
        Reason::AdminRequired
        | Reason::TeamSelfJoining
        | Reason::GuestRoleLimit
        | Reason::GuestCannotJoinTeam
        | Reason::SharingDisabled
        | Reason::TokenForbiddenEndpoint
        | Reason::TokenScopeInsufficient
        | Reason::ApiTokensDisallowed
        | Reason::TokenRateLimited
        | Reason::TokenExpiredForOrganization
        | Reason::TooManyApiTokens
        | Reason::ProviderChanged => describe_access(reason).unwrap_or_default(),
        Reason::SecretKeyRequired
        | Reason::OidcIssuerUnverified
        | Reason::OidcUnavailable
        | Reason::OidcStateInvalid
        | Reason::OidcDenied
        | Reason::OidcTokenRefused
        | Reason::OidcTokenInvalid
        | Reason::OidcEmailMissing
        | Reason::OidcEmailUnverified
        | Reason::OidcEmailTaken
        | Reason::OidcJoinClosed
        | Reason::OidcIdentityTaken
        | Reason::OidcEmailMismatch
        | Reason::OidcTrialAccount
        | Reason::OidcTrialStale
        | Reason::OidcCarryOverRefused
        | Reason::EmailUnverified
        | Reason::PasswordRequired
        | Reason::PasswordNotSet
        | Reason::LastSignIn
        | Reason::SsoRequired
        | Reason::SsoUnproven
        | Reason::SsoEnforced => describe_sign_in(reason).unwrap_or_default(),
        Reason::RelationNotAList
        | Reason::RelationToItself
        | Reason::RelationRepeated
        | Reason::RelationTooMany
        | Reason::RelationNotACard
        | Reason::RelationCardNotFound
        | Reason::RelationCycle => describe_relation(reason).unwrap_or_default(),
        Reason::WorkspaceNameTaken
        | Reason::TeamNameTaken
        | Reason::DefaultWorkspace
        | Reason::LastOrgAdmin
        | Reason::SoleOrgAdmin
        | Reason::AloneInOrganization
        | Reason::LastInstanceAdmin
        | Reason::CannotRemoveSelf
        | Reason::PasswordSignInRequired
        | Reason::PublicLinksOff
        | Reason::CopyOutDisabled => describe_organization(reason).unwrap_or_default(),
        // The rest are the reasons of uploads and archives, of the search of bodies, of
        // the limits of a wiki's tree and of a body edited together, which the functions
        // below name one by one: a reason that is none of theirs fails the tests.
        _ => describe_the_rest(reason),
    }
}

/// The description of a reason of uploads and archives, of the search of bodies, of the
/// limits of a wiki's tree or of a body edited together.
fn describe_the_rest(reason: Reason) -> &'static str {
    describe_transfer(reason)
        .or_else(|| describe_search(reason))
        .or_else(|| describe_page_tree(reason))
        .or_else(|| describe_diary(reason))
        .unwrap_or_else(|| describe_editing_together(reason))
}

/// The descriptions of the reasons of a diary: its one entry a day, and how many
/// entries and diaries there may be; and of how many units, chapters and emoji.
fn describe_diary(reason: Reason) -> Option<&'static str> {
    Some(match reason {
        Reason::DayTaken => {
            "Status 409. The day of the diary has an entry, and a diary has one entry a day: \
             when an entry is restored from the trash to a day that was written anew since. \
             Nothing was changed. Deleting the entry of the day makes room for the one in \
             the trash. (Making the entry of a day that has one is no error: \
             `createDiaryEntry` answers with the entry there is. An archive that holds two \
             entries of one day is a bad request, as any archive that is wrong in itself.)"
        }
        Reason::TooManyEntries => {
            "Status 400. The diary would have more entries than a diary may, which `limit` \
             tells (40000; entries in the trash are not counted): when an entry is made, \
             restored or imported. Nothing was changed."
        }
        Reason::TooManyDiaries => {
            "Status 400. The space would have more diaries than a space may, which `limit` \
             tells (20; diaries in the trash are not counted): when a diary is made, restored \
             or imported. Nothing was changed."
        }
        Reason::TooManyUnits => {
            "Status 400. The course would have more units than a course may, which `limit` \
             tells (2000; units in the trash and chapters are not counted): when a unit is \
             made, restored or imported. Nothing was changed. Deleting units makes room."
        }
        Reason::TooManyChapters => {
            "Status 400. The course would have more chapters than a course may, which `limit` \
             tells (200; chapters in the trash are not counted): when a chapter is made, \
             restored or imported. Nothing was changed. Deleting chapters makes room."
        }
        Reason::TooManyReactions => {
            "Status 400. The block has as many different emoji as a block may, which `limit` \
             tells (20): when a reaction adds one it has not (`addReaction`). Nothing changed."
        }
        _ => return None,
    })
}

/// The descriptions of the reasons of the limits of a wiki's tree, and of how many
/// organizations an account makes.
fn describe_page_tree(reason: Reason) -> Option<&'static str> {
    Some(match reason {
        Reason::PageTooDeep => {
            "Status 400. The page, or the lowest of the pages below it, would be deeper in its \
             wiki than the organization's limit, which `limit` tells (a page at the top is at \
             depth 1; `wiki_page_depth`, 10 unless configured): when a page is made, moved, \
             restored or duplicated. `errors` names `/parentId` when a page is made or moved; a \
             restore and a copy name no parent. Nothing was changed."
        }
        Reason::CardTooDeep => {
            "Status 400. The card, or the lowest sub-card below it, would be deeper on its board \
             than the limit (`limit`, 3: top-level card is depth 1, child is depth 2, grandchild \
             is depth 3): when a card is made, moved, or imported. `errors` names `/parentId`. \
             Nothing was changed."
        }
        Reason::RecurrenceNeedsBoardRules => {
            "Status 400. The card is to repeat (`recurrence`) on a board without a rule of done \
             (`doneWhen`) or a due date it names (`dueDatePropertyId`): the board's settings \
             give it both. `errors` names `/recurrence`. Nothing was changed."
        }
        Reason::TooManyPages => {
            "Status 400. The wiki would have more pages than the organization's limit, which \
             `limit` tells (`wiki_pages`, 5000 unless configured; pages in the trash are not \
             counted): when a page is made, restored or duplicated. Nothing was changed. \
             Deleting pages makes room."
        }
        Reason::TooManyOrganizations => {
            "Status 400. The account made as many organizations as one account may, which \
             `limit` tells: the ones in use that it started, by registering with the name \
             of one or by `createOrganization`. An organization it was invited into is not \
             counted, and neither is its own space. Nothing was made."
        }
        Reason::TooManyFavorites => {
            "Status 400. The user has as many stars in the workspace as a user may, which \
             `limit` tells (100): when a star is put on something (`putFavorite`). Nothing \
             was changed. Taking a star off makes room."
        }
        _ => return None,
    })
}

/// The descriptions of the reasons of the search of bodies.
fn describe_search(reason: Reason) -> Option<&'static str> {
    Some(match reason {
        Reason::SearchNotConfigured => {
            "Status 404. The administration socket's reindex, on a server that is configured \
             with no search server: there is no index to make. (A search on such a server \
             searches its database.)"
        }
        Reason::SearchUnavailable => {
            "Status 503. The search was not answered within three seconds: the search \
             server did not answer it, or refused it, or (on a server without a search \
             server) the database did not finish it in time. Or the search server the server \
             is configured with cannot hold its index (it has no nori plugin, or holds another \
             index under the name), which the server says in its log when it finds it: every \
             search is answered so until that is put right and the server started again, and \
             the database does not stand in for it. Nothing else of the server depends on \
             it: what is written meanwhile is found once the search answers again. Try again \
             later."
        }
        _ => return None,
    })
}

/// The descriptions of the reasons of uploads in parts and of archives.
fn describe_transfer(reason: Reason) -> Option<&'static str> {
    Some(match reason {
        Reason::TooManyUploads => {
            "Status 429. The user has as many uploads in parts open as one user may. Complete \
             or abandon one first. No wait is told: what makes room is the user's own doing."
        }
        Reason::UploadsPending => {
            "Status 429. The user's open uploads in parts hold back as many bytes as one \
             user's may. Complete or abandon one first."
        }
        Reason::ServerUploadsPending => {
            "Status 429. The open uploads in parts of all users hold back as many bytes as the \
             server lets them. Try again later."
        }
        Reason::UploadInParts => {
            "Status 413. The file is larger than one request takes (`limit`): send it as an \
             upload in parts, which takes a file up to the largest the server stores."
        }
        Reason::FileVersionStale => {
            "Status 409. A version of a file was written from a version (`baseVersion`) that \
             is the file's newest no more: somebody wrote one meanwhile. Nothing is changed, \
             and an uploaded file that was to become the version (`addFileVersion`) is still \
             a file of the board. Read the versions again (`getFileVersions`), and write \
             from the newest or attach the uploaded file on its own."
        }
        Reason::FileBeingEdited => {
            "Status 409. The file's lease (`holdFileLease`) is held by another editor: \
             somebody else, or the caller in another window, has the file open in the \
             editor. `getFileLease` says who, where the caller may know. Nothing keeps a \
             version from being written meanwhile (`addFileVersion` is held to \
             `baseVersion`); the lease is let go of when that editor is closed, and ends \
             90 seconds after it was last held on to otherwise."
        }
        Reason::ExportRunning => {
            "Status 429. The user has an export running, and runs one at a time. Try again \
             when it is done."
        }
        Reason::ServerExportsBusy => {
            "Status 429. The server runs as many exports as it may at once. Try again later."
        }
        Reason::ImportRunning => {
            "Status 429. The user has an import running, and runs one at a time. Try again \
             when it is done."
        }
        Reason::ServerImportsBusy => {
            "Status 429. The server runs as many imports as it may at once. Try again later."
        }
        Reason::FileTooLarge => {
            "Status 413. The file is larger than the largest the server stores (`limit`, the \
             `maxFileBytes` of `getClientConfig`), sent in one request or in parts."
        }
        Reason::StorageFull => {
            "Status 413. Storing the upload or the archive would take the organization's \
             stored files and open uploads past its storage quota (`limit`, in bytes; \
             `max_org_storage_bytes`). Delete unused files or purge the trash first."
        }
        Reason::ArchiveTooLarge => {
            "Status 413. The archive is larger than the largest the server imports (`limit`, \
             the `maxImportBytes` of `getClientConfig`)."
        }
        Reason::ArchiveEntryTooLarge => {
            "Status 413. An entry of the archive unpacks to more than an entry of its kind \
             may (`limit`, in bytes): a file to more than the largest the server stores (the \
             `maxFileBytes` of `getClientConfig`), a `container.jsonl` to more than the server \
             reads of one. Nothing of the archive is stored."
        }
        Reason::RequestTooLarge => {
            "Status 413. The body of the request is larger than its route reads (`limit`, in \
             bytes), by the length it declares or by what it sends. A request without a \
             session is read no further than the sign-in and the registration need. A file \
             is not sent in such a body: `uploadFile` and the parts of an upload have limits \
             and reasons of their own."
        }
        Reason::DatabaseBusy => {
            "Status 503. The database did not take a statement of the request in the time \
             it had: another write held an SQLite file for longer than the busy timeout of \
             `database_connection`, or no connection to the database was to be had (all in \
             use, or the database not reached). That statement did nothing, and may be \
             taken a moment later; what the request had done before it is done."
        }
        _ => return None,
    })
}

/// The descriptions of the reasons somebody is not let in for, and an invitation's mail is
/// not sent for.
fn describe_invitation(reason: Reason) -> Option<&'static str> {
    Some(match reason {
        Reason::InvitationRequired => {
            "Status 403. The server has one organization, which its first user started by \
             registering. Everybody after gets in by the link of an invitation, and this \
             registration is none."
        }
        Reason::InvitationUnavailable => {
            "Status 404. The link of the invitation lets nobody in. The answer is the same \
             whether there never was such a link, it ended after its seven days, it was \
             used as often as it may be, it was revoked, or who made it may no longer \
             invite. Ask for a new link."
        }
        Reason::TooManyRecipients => {
            "Status 400. An invitation mail request names more recipients than \
             `invitation_mail_recipients` allows (20 unless configured), which `limit` \
             tells. `errors` names `/recipients`. Nothing was sent: send fewer at a time."
        }
        Reason::InvitationMailLimit => {
            "Status 429. The workspace queued as many invitation messages within the last \
             hour as `invitation_mail_per_hour` allows (100 unless configured), counting the \
             recipients of this request. Nothing of it was sent. `Retry-After` and \
             `retryAfter` say when enough of the earlier messages are an hour old."
        }
        Reason::MailQueueFull => {
            "Status 503. The server holds as many invitation messages waiting to be sent as \
             `mail_invitation_queue` allows. Nothing of this request was sent. Try again later."
        }
        _ => return None,
    })
}

/// The descriptions of the reasons the administrators of an organization or of a
/// workspace are refused for.
fn describe_organization(reason: Reason) -> Option<&'static str> {
    Some(match reason {
        Reason::PasswordSignInRequired => {
            "Status 401, with a session that is good, from the routes under the \
             `/w/{workspaceId}` of the user's own space: the account has a password, and \
             its space takes only sessions that showed it, while this one was made \
             through an organization's provider and has not. A provider is an \
             organization's, and the space is the person's alone. The session stays, and \
             opens what is of the organizations it opened before: give the password in \
             it (`confirmPassword`) and it opens the space too, as a session made with \
             the password (`login`) does. `getMyWorkspaces` says beforehand that it is closed so \
             (`passwordRequired`). An account without a password (`hasPassword` of \
             `getMe`) is refused nothing so. What is shared from the space by a public \
             link is read without a session and is not closed."
        }
        Reason::WorkspaceNameTaken => {
            "Status 409. Another workspace of the organization that is in use has the name; \
             one in the trash leaves its name to be taken. `errors` names `/name`, or \
             `/organizationName` where the first registration names the organization so."
        }
        Reason::TeamNameTaken => {
            "Status 409. Another team of the workspace has the name, in the same or another \
             letter case: `Design` and `DESIGN` are one name. `errors` names `/name`."
        }
        Reason::DefaultWorkspace => {
            "Status 409. The organization's default workspace, which whoever joins the \
             organization gets into, is not deleted."
        }
        Reason::LastOrgAdmin => {
            "Status 409. The change would leave the organization without an administrator. \
             Make somebody else one first. From `requestAccountDeletion`: the account is \
             the only administrator of an organization that has other people in it, or \
             of the server's own first organization, and `orgId` names that organization, \
             the first by identifier if there are several (`getAccountDeletion` lists \
             them all). Another organization that has nobody else does not stand in the \
             way: it goes with the account."
        }
        Reason::SoleOrgAdmin => {
            "Status 409, leaving or removing a member from an organization \
             (`leaveOrganization`, `removeOrgMember`): the account is its only \
             administrator, and other people are in it. Make another member its \
             administrator first (`putOrgRole`); where the others are guests of its \
             workspaces only, invite a member first. Nothing changed."
        }
        Reason::AloneInOrganization => {
            "Status 409, leaving or removing a member from an organization \
             (`leaveOrganization`, `removeOrgMember`): nobody else is in it, and it \
             would be left empty. Invite somebody to stay with it first. An organization \
             of one goes with its account when that is deleted \
             (`requestAccountDeletion`), but the server's own first one. Nothing changed."
        }
        Reason::LastInstanceAdmin => {
            "Status 409, leaving or removing a member from the server's own first \
             organization (`leaveOrganization`, `removeOrgMember`): the account is the \
             server's only administrator, which it is as an administrator of that \
             organization. Make somebody else an administrator of the organization \
             first. Nothing changed."
        }
        Reason::CannotRemoveSelf => {
            "Status 409, removing a member from an organization (`removeOrgMember`): an \
             administrator cannot remove themselves here. Leave the organization through \
             `leaveOrganization` instead. Nothing changed."
        }
        Reason::PublicLinksOff => {
            "Status 403. The organization has turned its public links off: no board of it is \
             shared by a link until they are on again. Stopping a board's sharing is not \
             refused."
        }
        Reason::CopyOutDisabled => {
            "Status 403. The organization has turned off copying containers out to another \
             organization, including to a user's personal space (`allowCopyOut`)."
        }
        _ => return None,
    })
}

/// The descriptions of the reasons a value of a card relation is refused for.
fn describe_relation(reason: Reason) -> Option<&'static str> {
    Some(match reason {
        Reason::RelationNotAList => {
            "Status 400. The value of a card relation property (of the `type` `relation`) is \
             `null` or a list of relation link objects (`{\"card\", \"kind\", \"lagDays\"}`); \
             a string, a legacy list of card identifier strings, or another non-link list is \
             refused here (malformed link objects fail request validation with \
             `/problems/bad-request`). `errors` names where the request has the card's values \
             (a block's `fields` or `updatedFields`, a card's `properties` or \
             `updatedProperties`); the words name the property."
        }
        Reason::RelationToItself => {
            "Status 400. A card relation names its own card. `errors` as for \
             `relation-not-a-list`."
        }
        Reason::RelationRepeated => {
            "Status 400. A card relation names a card twice. `errors` as for \
             `relation-not-a-list`."
        }
        Reason::RelationTooMany => {
            "Status 400. A card relation names more cards than one may (`limit`) and one it \
             did not name before. `errors` as for `relation-not-a-list`."
        }
        Reason::RelationNotACard => {
            "Status 400. A card relation names a block of the board that is no card. \
             `errors` as for `relation-not-a-list`."
        }
        Reason::RelationCardNotFound => {
            "Status 400. A card relation names no card of the board: none of that identifier, \
             one removed from the trash, or another board's, which the answer does not tell \
             apart. A card in the trash may be named, and what a value named before is not \
             asked for again. `errors` as for `relation-not-a-list`."
        }
        Reason::RelationCycle => {
            "Status 400. A card's ordering relation property (`orderingRelations` of the board) \
             would form a dependency cycle among the cards of the board. `errors` as for \
             `relation-not-a-list`."
        }
        _ => return None,
    })
}

/// What the reasons of a body edited together mean.
fn describe_editing_together(reason: Reason) -> &'static str {
    match reason {
        Reason::UpdateRefused => {
            "Status 400. The document of a card's body that is edited together does not take \
             the update: it is no update, writes outside the body, or would leave a document \
             that is no body. The document is as it was."
        }
        Reason::UpdateOutOfStep => {
            "Status 409. The update builds on changes the document of the body does not \
             have: the editor that made it holds another document, or one that was made \
             again. Load the document again and make the change there."
        }
        Reason::UpdateTooLarge => {
            "Status 413. The update is larger than the server takes (setting \
             `collab_update_bytes`); `limit` is the limit in bytes. Send the change in \
             smaller updates."
        }
        Reason::DocumentTooLarge => {
            "Status 413. With the update the document of the body, or the body, would be \
             larger than it may be (setting `collab_state_bytes`, and the limit of a body), \
             or the document would take more steps to read than it may; `limit` is the \
             limit in bytes."
        }
        Reason::TableTooLarge => {
            "Status 400. A table in the body has, or with the update would have, more than \
             200 rows or more than 20 columns. Nothing was changed."
        }
        Reason::TooManyDocuments => {
            "Status 503. The server holds as many documents of bodies being edited as it \
             may (settings `collab_workers`, `collab_documents_per_worker`, \
             `collab_worker_memory_bytes`). Try again when others are closed."
        }
        Reason::DocumentUnavailable => {
            "Status 503. The document of the body cannot be worked on for now: the process \
             that holds the documents ended or did not answer in time, which drops what was \
             under way, or did so again and again with this document, which is closed for \
             a while; or a write of the card's whole body came while what its editors typed \
             could not be written to the card first, and was not done; or another process \
             wrote the stored document since this one read it (a server that lost the lock \
             of its database and has not gone down yet), and this one leaves it alone. The \
             document is as it was stored before. Try again later."
        }
        Reason::ComplexBlocksUseAppend => {
            "Status 409. A body is written whole from Markdown (`bodyMarkdown`) over one \
             with blocks its Markdown does not hold as they are (`complexBlocks` of \
             `getCard` with `format=markdown`): merged cells, cells of more than a line, \
             formulas Markdown cannot write. Nothing was changed. Add Markdown after the \
             body (`appendCardMarkdown`), or write it as JSON (`body`)."
        }
        Reason::RoomHasActiveEditors => {
            "Status 409. A personal API token writes a card's body whole while people may \
             write in it, which would lose what they typed and have not sent yet. Nothing \
             was changed. Add Markdown after the body (`appendCardMarkdown`), which keeps \
             what they type, or try again once they have left."
        }
        other => describe_rooms(other),
    }
}

/// What the reasons mean that only the rooms of the websocket give (`docRefused`,
/// `docNack`, `docLeft`).
fn describe_rooms(reason: Reason) -> &'static str {
    match reason {
        Reason::DocumentReadOnly => {
            "Over the websocket only (`docNack`). The connection is in the room of the \
             card's body to read: its user reads the card's board and may not change its \
             cards. The update went to nobody; the editor that made it holds a change the \
             document does not, and comes into the room anew to drop it."
        }
        Reason::ConnectionTooSlow => {
            "Over the websocket only (`docLeft`, `docRefused`). As many messages wait \
             for the connection as may (settings `collab_queue_messages`, and twice \
             `collab_state_bytes` in bytes), and it was put out of the room, or not let \
             in, so that the others go on. The frames that waited for it of the room \
             are dropped. Come into the room again: the whole document is sent anew."
        }
        Reason::TooManyDocumentsJoined => {
            "Over the websocket only (`docRefused`). The connection is in as many rooms \
             as one connection may be (setting `collab_documents_per_connection`). Leave \
             one, or open another connection."
        }
        Reason::DocumentFull => {
            "Over the websocket only (`docRefused`). As many connections are in the room \
             of the card's body as may be (setting `collab_connections_per_document`). \
             Try again when one has left."
        }
        Reason::TooManyConnectionsJoined => {
            "Over the websocket only (`docRefused`). As many of the user's own connections \
             are in the room of the card's body as one user's may be (setting \
             `collab_connections_per_user`): the card is open in that many tabs or \
             devices. Close one of them."
        }
        Reason::TooManyFrames => {
            "Over the websocket only (`docRefused`, `docLeft`). The connection sent the \
             rooms more than it may: as many frames a second as the setting \
             `collab_frames_per_second` says, and as many at once after a pause, a \
             `joinDoc` and a sync step 1 counting as five. A `joinDoc` past it is \
             refused; a frame past it puts the connection out of the room the frame was \
             for, since the room no longer has what the connection sent. Send less often, \
             and come in again."
        }
        other => unreachable!("{other:?} is no reason of the rooms"),
    }
}

/// How long a cache may keep a page: the pages change with a release of the server.
const PAGE_CACHE_CONTROL: &str = "public, max-age=3600";

/// A page of plain text that anyone may read and a cache may keep for a while.
fn text_page(text: String) -> Response {
    (
        [
            (
                CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
            (CACHE_CONTROL, HeaderValue::from_static(PAGE_CACHE_CONTROL)),
        ],
        text,
    )
        .into_response()
}

/// The path the server is served under, from the address a page of this module was
/// asked at: what comes before [`PROBLEMS_PATH`].
fn base_path_of(asked: &Uri) -> &str {
    let path = asked.path();
    path.rfind(PROBLEMS_PATH).map_or("", |at| &path[..at])
}

/// The routes of the pages: the list at [`PROBLEMS_PATH`] and a page for each kind
/// below it. A name that is no text is refused as a problem, like any other error.
pub(crate) fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route(PROBLEMS_PATH, get(handle_problems))
        .route(&format!("{PROBLEMS_PATH}/{{name}}"), get(handle_problem))
        .layer(middleware::from_fn(problems_only))
}

/// The page about the kind of problem `name`: what the `type` of a problem points at.
async fn handle_problem(
    OriginalUri(asked): OriginalUri,
    Path(name): Path<String>,
) -> Result<Response, AppError> {
    let problem = find(&name).ok_or_else(|| AppError::not_found("problem type"))?;
    Ok(text_page(problem.page(base_path_of(&asked))))
}

/// The list of every kind of problem, a line for each: its `type` and its title.
async fn handle_problems(OriginalUri(asked): OriginalUri) -> Response {
    let base_path = base_path_of(&asked);
    let mut page =
        "The kinds of problem this API answers with (RFC 9457). Each is described at its type.\n\n"
            .to_owned();
    for problem in all() {
        // Writing to a string cannot fail.
        let _ = writeln!(
            page,
            "{base_path}{}  {}",
            problem.type_reference(),
            problem.title
        );
    }
    text_page(page)
}

fn describe_access(reason: Reason) -> Option<&'static str> {
    Some(match reason {
        Reason::AdminRequired => "Status 403. Only an administrator of the server may do this.",
        Reason::TeamSelfJoining => {
            "Status 403. While joined as organization administrator, you cannot add yourself to a team. Another workspace administrator may add you. Direct board roles end with the administrative joining."
        }
        Reason::GuestRoleLimit => "Status 403. A guest of a workspace has no role above editor.",
        Reason::GuestCannotJoinTeam => {
            "Status 409. A guest of a workspace cannot be added to a team: only workspace members who work there belong to one."
        }
        Reason::SharingDisabled => {
            "Status 403. Sharing a board with whoever has its link is turned off in the configuration of this server."
        }
        Reason::TokenForbiddenEndpoint => {
            "Status 403, for a request with a personal API token (`Authorization: Bearer fvoci_pat_...`) to an operation that is not open to one: what is of the account (its password, sessions, tokens, notifications and settings), of who is on what (members, invitations, groups), of what is shared (public links), and of an administrator (`/org/...`). Status 401 for the handshake of the websocket, which a token opens for nothing. A token is sent in the `Authorization` header alone; in the session cookie it opens nothing. Sign in, or do it in the app."
        }
        Reason::TokenScopeInsufficient => {
            "Status 403, for a request with a personal API token that reads only (`scope` `read`) to an operation that writes. Make a token that reads and writes (`createApiToken` with `scope` `write`)."
        }
        Reason::ApiTokensDisallowed => {
            "Status 403, for a request with a personal API token to a workspace whose organization takes only sessions made through its provider and gives its workspaces to no token (`allowApiTokens` of the organization off). An administrator of it switches that on."
        }
        Reason::TokenRateLimited => {
            "Status 429, for a request with a personal API token: the token, or the tokens that reach the workspace's organization together, sent as many requests as they may in a minute. Wait for as many seconds as `Retry-After` says."
        }
        Reason::TokenExpiredForOrganization => {
            "Status 401, for a request with a personal API token to a workspace whose organization takes only sessions made through its provider: the token was made more than 90 days ago, which such an organization lets no token be, also one made to last longer before the organization took only such sessions. Its owner makes another."
        }
        Reason::TooManyApiTokens => {
            "Status 400, for making a personal API token: the account holds as many tokens that open anything (not taken back, not past their end) as one may, which `limit` tells (25). Nothing was made. Taking one back makes room."
        }
        Reason::ProviderChanged => {
            "Status 409, for making a personal API token from a session made through the organization's provider: the provider has been removed, switched off or changed since the session was made. Nothing was made, and the session stays signed in for the rest. Sign in through the provider again (`startOidcLogin`) and make the token from the session that comes of it."
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
