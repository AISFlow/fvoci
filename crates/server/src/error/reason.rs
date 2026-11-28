//! The reasons of the refusals: the value of a problem's `reason` member, by which a
//! client tells the causes of one status apart, and the name and title of its problem.

/// Defines [`Reason`] with, for each variant, the value of the `reason` member and the
/// `title` of its problem, and the list of every variant made from the same lines: a
/// reason cannot be there without being listed and titled.
macro_rules! reasons {
    ($($(#[$doc:meta])* $variant:ident => $value:literal, $title:literal;)+) => {
        /// Why a request was refused, where clients must tell causes of one kind apart.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Reason {
            $($(#[$doc])* $variant,)+
        }

        impl Reason {
            /// Every reason, in the order they are defined in.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// The value of the body's `reason` member.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value,)+
                }
            }

            /// The `title` of the problem an error with this reason is.
            pub fn title(self) -> &'static str {
                match self {
                    $(Self::$variant => $title,)+
                }
            }
        }
    };
}

reasons! {
    /// The request lacks the header of the cross-site request check.
    Csrf => "csrf", "Cross-site request check failed";
    /// The request, or the websocket handshake, comes from a page of another origin.
    ForeignOrigin => "foreign_origin", "Request from another origin";
    /// The user name, email address or password of a login is wrong.
    InvalidCredentials => "invalid_credentials", "Invalid credentials";
    /// The server has its organization: who is not its first user gets in by an invitation.
    InvitationRequired => "invitation_required", "Invitation required";
    /// The link of an invitation lets nobody in: there is none, or it ended.
    InvitationUnavailable => "invitation_unavailable", "Invitation not available";
    /// The user name is taken.
    UsernameTaken => "username_taken", "User name taken";
    /// The email address is taken.
    EmailTaken => "email_taken", "Email address taken";
    /// The password is right, of a registration whose email address is not confirmed
    /// yet: there is no account until it is.
    EmailUnverified => "email_unverified", "Email address not confirmed";
    /// A confirmation link was opened in another browser than its registration was made
    /// from, or an account without a password asks to leave an organization.
    PasswordRequired => "password_required", "Password required";
    /// The change would leave the board, the workspace or the instance without an
    /// administrator.
    LastAdmin => "last_admin", "Last administrator";
    /// The session is of an account that waits to be deleted: it opens the way back
    /// and nothing else.
    AccountDeletionPending => "account_deletion_pending", "Account waits to be deleted";
    /// An account without a password asks for what its session, not made through a
    /// provider just now, does not prove to be its person's.
    SignInAgain => "sign_in_again", "Sign in again";
    /// Only an administrator of the instance may do this.
    AdminRequired => "admin_required", "Administrator required";
    /// A guest of the workspace is given no role on a board above editor.
    GuestRoleLimit => "guest_role_limit", "A guest is no more than an editor";
    /// A guest of the workspace cannot be added to a team.
    GuestCannotJoinTeam => "guest_cannot_join_team", "A guest of the workspace cannot be added to a team";
    /// A temporary organization administrator cannot add themselves to a team.
    TeamSelfJoining => "team_self_joining", "Cannot add yourself to a team during administrative joining";
    /// Public sharing is turned off in the server's configuration.
    SharingDisabled => "sharing_disabled", "Public sharing turned off";
    /// The user has as many uploads in parts open as one may.
    TooManyUploads => "too_many_uploads", "Too many open uploads";
    /// The user's open uploads in parts hold back as many bytes as one user's may.
    UploadsPending => "uploads_pending", "Open uploads hold too much";
    /// The open uploads in parts of the whole server hold back as many bytes as they may.
    ServerUploadsPending => "server_uploads_pending", "The server's open uploads hold too much";
    /// The file is larger than one request takes; it is to be sent in parts.
    UploadInParts => "upload_in_parts", "File to be uploaded in parts";
    /// A version of a file was written from a version that is the file's newest no more.
    FileVersionStale => "file_version_stale", "The file has a newer version";
    /// A lease of a file was asked for while another is held: somebody edits it.
    FileBeingEdited => "file_being_edited", "Somebody else is editing the file";
    /// The user has an export running.
    ExportRunning => "export_running", "An export is running";
    /// The server runs as many exports as it may.
    ServerExportsBusy => "server_exports_busy", "The server is busy exporting";
    /// The user has an import running.
    ImportRunning => "import_running", "An import is running";
    /// The server runs as many imports as it may.
    ServerImportsBusy => "server_imports_busy", "The server is busy importing";
    /// The file is larger than the largest the server stores.
    FileTooLarge => "file_too_large", "File too large";
    /// Storing the file would take the organization past `max_org_storage_bytes`.
    StorageFull => "storage_full", "Organization storage full";
    /// The archive is larger than the largest the server imports.
    ArchiveTooLarge => "archive_too_large", "Archive too large";
    /// An entry of the archive unpacks to more than an entry of its kind may.
    ArchiveEntryTooLarge => "archive_entry_too_large", "Archive entry too large";
    /// The body of the request is larger than its route reads.
    RequestTooLarge => "request_too_large", "Request too large";
    /// The database did not take a statement of the request in time.
    DatabaseBusy => "database_busy", "Database busy";
    /// The body's document does not take the update.
    UpdateRefused => "update_refused", "Update refused";
    /// The update builds on changes the body's document does not have.
    UpdateOutOfStep => "update_out_of_step", "Update out of step";
    /// The update is larger than the server takes.
    UpdateTooLarge => "update_too_large", "Update too large";
    /// With the update the body's document, or its body, would be larger than it may be.
    DocumentTooLarge => "document_too_large", "Document too large";
    /// A table in the body has, or with the update would have, more rows or columns than it may.
    TableTooLarge => "table_too_large", "Table too large";
    /// The server holds as many documents of bodies being edited as it may.
    TooManyDocuments => "too_many_documents", "Too many documents open";
    /// The body's document cannot be worked on for now.
    DocumentUnavailable => "document_unavailable", "Document unavailable";
    /// The connection reads the body's document and does not write it.
    DocumentReadOnly => "document_read_only", "Document read only";
    /// The websocket connection does not read what it is sent of the body's document.
    ConnectionTooSlow => "connection_too_slow", "Connection too slow";
    /// The websocket connection is in as many rooms of bodies as one may be.
    TooManyDocumentsJoined => "too_many_documents_joined", "Connection in too many documents";
    /// As many websocket connections are in the room of the body as may be.
    DocumentFull => "document_full", "Document full";
    /// As many of the user's websocket connections are in the room of the body as may be.
    TooManyConnectionsJoined => "too_many_connections_joined", "Too many connections of one user";
    /// The websocket connection sends more of the rooms in a second than it may.
    TooManyFrames => "too_many_frames", "Too many frames";
    /// The server has no `secret_key` to seal a provider's client secret with.
    SecretKeyRequired => "secret_key_required", "Secret key required";
    /// The issuer of a provider could not be read and checked.
    OidcIssuerUnverified => "oidc_issuer_unverified", "Provider not verified";
    /// The organization has no provider that is on, or it could not be reached.
    OidcUnavailable => "oidc_unavailable", "Sign-in through the provider not available";
    /// A sign-in came back that is not one this browser started, was used or ended.
    OidcStateInvalid => "oidc_state_invalid", "Sign-in not recognized";
    /// The provider did not let the person sign in.
    OidcDenied => "oidc_denied", "Sign-in refused by the provider";
    /// The provider gave no ID token for the sign-in.
    OidcTokenRefused => "oidc_token_refused", "No token from the provider";
    /// The ID token of the provider does not hold.
    OidcTokenInvalid => "oidc_token_invalid", "Token of the provider not valid";
    /// The provider gave no email address for a person who has no account yet.
    OidcEmailMissing => "oidc_email_missing", "No email address from the provider";
    /// The provider says it has not checked the person's email address.
    OidcEmailUnverified => "oidc_email_unverified", "Email address not verified";
    /// Another account has the email address the provider gave.
    OidcEmailTaken => "oidc_email_taken", "Email address of another account";
    /// The organization does not let in who is no member through its provider.
    OidcJoinClosed => "oidc_join_closed", "Organization closed to newcomers";
    /// The identity a provider vouches for is another account's: it is not linked.
    OidcIdentityTaken => "oidc_identity_taken", "Sign-in of another account";
    /// The email address a provider vouches for is not the one of the account it was
    /// to be linked to: it is not linked.
    OidcEmailMismatch => "oidc_email_mismatch", "Email address of the provider not the account's";
    /// A trial of a provider's pending settings ended in another account than the one
    /// of the administrator who started it: nothing is changed.
    OidcTrialAccount => "oidc_trial_account", "Trial in another account";
    /// A trial of pending settings of another issuer vouched for an identity tied to
    /// no account, and was not started from a session made through the provider just
    /// now: nothing is tied or changed.
    OidcTrialStale => "oidc_trial_stale", "Trial from a session not made just now";
    /// Nothing was carried over to the organization's provider: no sign-in is held for
    /// the browser, the former provider is kept no more, or it vouched for nobody with
    /// an account that has a place in the organization.
    OidcCarryOverRefused => "oidc_carry_over_refused", "Account not carried over";
    /// The account has no password to confirm with.
    PasswordNotSet => "password_not_set", "Account without a password";
    /// The identity is the only way in to an account without a password.
    LastSignIn => "last_sign_in", "Last way to sign in";
    /// The organization takes only sessions made through its provider, and this one
    /// was not.
    SsoRequired => "sso_required", "Sign-in through the provider required";
    /// The user's own space takes only sessions that showed the account's password, and
    /// this one was made through a provider and has not.
    PasswordSignInRequired => "password_sign_in_required", "The account's password is required";
    /// Taking no password is switched on from a session made through the provider.
    SsoUnproven => "sso_unproven", "Provider not signed in through";
    /// The organization takes no password, and its provider stays as it is meanwhile.
    SsoEnforced => "sso_enforced", "Provider held while no password is taken";
    /// A personal API token was sent to a route that is not open to tokens: what is of
    /// the account, of who is on what, of what is shared, or of an administrator.
    TokenForbiddenEndpoint => "token_forbidden_endpoint", "Not open to a personal API token";
    /// A personal API token that reads only was sent to a route that writes.
    TokenScopeInsufficient => "token_scope_insufficient", "Personal API token reads only";
    /// The workspace's organization, which takes only sessions made through its
    /// provider, gives its workspaces to no personal API token.
    ApiTokensDisallowed => "api_tokens_disallowed", "Organization takes no personal API tokens";
    /// A personal API token, or the tokens that reach one organization together, sent
    /// more requests than they may in a minute.
    TokenRateLimited => "token_rate_limited", "Too many requests of personal API tokens";
    /// A personal API token reaches a workspace of an organization that takes only
    /// sessions made through its provider more than 90 days after it was made.
    TokenExpiredForOrganization => "token_expired_for_organization", "Personal API token too old for the organization";
    /// The account holds as many personal API tokens in use as one may.
    TooManyApiTokens => "too_many_api_tokens", "Too many personal API tokens";
    /// The session was made through the organization's provider, which has been removed,
    /// switched off or changed since; the session stays good for the rest.
    ProviderChanged => "provider_changed", "Provider changed since the session was made";
    /// A body is written whole from Markdown over one that has blocks its Markdown does
    /// not hold as they are (`complexBlocks`): Markdown is added after it instead.
    ComplexBlocksUseAppend => "complex_blocks_use_append", "Body that Markdown does not hold";
    /// A personal API token writes a body whole while people edit it: what they typed
    /// and did not send yet would be lost. Markdown is added after it instead.
    RoomHasActiveEditors => "room_has_active_editors", "Body edited by people now";
    /// The value of a card relation is neither `null` nor a list of identifiers.
    RelationNotAList => "relation_not_a_list", "Card relation that is no list";
    /// A card relation names the card it is the value of.
    RelationToItself => "relation_to_itself", "Card relation to the card itself";
    /// A card relation names a card twice.
    RelationRepeated => "relation_repeated", "Card named twice in a relation";
    /// A card relation names more cards than one may.
    RelationTooMany => "relation_too_many", "Card relation names too many cards";
    /// A card relation names a block of the board that is no card.
    RelationNotACard => "relation_not_a_card", "Card relation names no card";
    /// A card relation names a card that is not on the board.
    RelationCardNotFound => "relation_card_not_found", "Card relation names a card not on the board";
    /// Ordering card dependencies form a cycle on the board.
    RelationCycle => "relation_cycle", "Card dependency cycle";
    /// Another workspace in use of the organization has the name.
    WorkspaceNameTaken => "workspace_name_taken", "Workspace name taken";
    /// Another team of the workspace has the name, in some letter case.
    TeamNameTaken => "team_name_taken", "Team name taken";
    /// The organization's default workspace is not deleted.
    DefaultWorkspace => "default_workspace", "Default workspace";
    /// The change would leave the organization without an administrator.
    LastOrgAdmin => "last_org_admin", "Last administrator of the organization";
    /// The account that would leave or be taken out of the organization is its only
    /// administrator, and other people are in it.
    SoleOrgAdmin => "sole_org_admin", "Only administrator of the organization";
    /// The account that would leave or be taken out of the organization is the only
    /// person in it.
    AloneInOrganization => "alone_in_organization", "Only person in the organization";
    /// The account that would leave or be taken out of the server's own organization is
    /// the server's only administrator.
    LastInstanceAdmin => "last_instance_admin", "Last administrator of the server";
    /// An administrator tried to remove themselves through `removeOrgMember` instead of
    /// `leaveOrganization`.
    CannotRemoveSelf => "cannot_remove_self", "Cannot remove yourself";
    /// The organization has turned its public links off.
    PublicLinksOff => "public_links_off", "Public links turned off";
    /// The organization has turned off copying containers out to another organization.
    CopyOutDisabled => "copy_out_disabled", "Copying out of the organization turned off";
    /// An invitation mail request names more recipients than one may.
    TooManyRecipients => "too_many_recipients", "Too many recipients";
    /// The workspace queued as many invitation messages within the hour as it may.
    InvitationMailLimit => "invitation_mail_limit", "Hourly invitation mail limit reached";
    /// The server holds as many invitation messages waiting to be sent as it may.
    MailQueueFull => "mail_queue_full", "Mail queue full";
    /// The search server the server is configured with cannot hold its index: bodies
    /// are not searched (nor, on the administration socket, made again where there is no
    /// search server).
    SearchNotConfigured => "search_not_configured", "Search not configured";
    /// A search was not answered in time: by the search server, or by the database
    /// where there is none.
    SearchUnavailable => "search_unavailable", "Search unavailable";
    /// A page would be deeper in its wiki than the organization's limit.
    PageTooDeep => "page_too_deep", "Page too deep";
    /// A card, or a sub-card below it, would be deeper on its board than the limit.
    CardTooDeep => "card_too_deep", "Card too deep";
    /// A card is to repeat on a board without a rule of done or a due date property.
    RecurrenceNeedsBoardRules => "recurrence_needs_board_rules", "The board does not let cards repeat";
    /// The wiki would have more pages than the organization's limit.
    TooManyPages => "too_many_pages", "Too many pages in the wiki";
    /// The day of the diary has an entry: one that would come back from the trash finds
    /// its day taken.
    DayTaken => "day_taken", "The day has an entry";
    /// The diary would have more entries than a diary may.
    TooManyEntries => "too_many_entries", "Too many entries in the diary";
    /// The space would have more diaries than a space may.
    TooManyDiaries => "too_many_diaries", "Too many diaries";
    /// The course would have more units than a course may.
    TooManyUnits => "too_many_units", "Too many units in the course";
    /// The course would have more chapters than a course may.
    TooManyChapters => "too_many_chapters", "Too many chapters in the course";
    /// The account made as many organizations, which are in use, as one account may.
    TooManyOrganizations => "too_many_organizations", "Too many organizations made";
    /// The user has as many stars in the workspace as a user may.
    TooManyFavorites => "too_many_favorites", "Too many favorites in the workspace";
    /// The block is answered with as many different emoji as a block may.
    TooManyReactions => "too_many_reactions", "Too many emoji on the block";
}

impl Reason {
    /// The name of the problem an error with this reason is, the last part of its
    /// `type`: the reason, written with hyphens.
    pub fn name(self) -> String {
        self.as_str().replace('_', "-")
    }
}
