//! The tags of the contract: the names operations are grouped under, and what each
//! group is about, in the order the document lists them.

/// The names operations are grouped under.
pub(crate) mod tag {
    pub(crate) const SYSTEM: &str = "system";
    pub(crate) const AUTH: &str = "auth";
    pub(crate) const USERS: &str = "users";
    pub(crate) const WORKSPACES: &str = "workspaces";
    pub(crate) const CONTAINERS: &str = "boards";
    pub(crate) const WIKIS: &str = "wikis";
    pub(crate) const DIARIES: &str = "diaries";
    pub(crate) const COURSES: &str = "courses";
    pub(crate) const MEMBERS: &str = "members";
    pub(crate) const PUBLIC_LINKS: &str = "public-links";
    pub(crate) const CARDS: &str = "cards";
    pub(crate) const REFERENCES: &str = "references";
    pub(crate) const SIDEBAR: &str = "sidebar";
    pub(crate) const FILES: &str = "files";
    pub(crate) const ARCHIVES: &str = "archives";
    pub(crate) const SUBSCRIPTIONS: &str = "subscriptions";
    pub(crate) const NOTIFICATIONS: &str = "notifications";
    pub(crate) const TIME: &str = "time";
    pub(crate) const ORGANIZATION: &str = "organization";
    pub(crate) const API_TOKENS: &str = "api-tokens";
    pub(crate) const MCP: &str = "mcp";
}

/// The tags in the order the document lists them, each with what it is about.
pub(super) const TAGS: [(&str, &str); 21] = [
    (
        tag::SYSTEM,
        "Whether the server is there, and what a client must know before it signs in.",
    ),
    (
        tag::AUTH,
        "Signing in and out, registering, and changing the password.",
    ),
    (
        tag::USERS,
        "The user of the session, their settings, and other users.",
    ),
    (
        tag::WORKSPACES,
        "The workspaces a user is a member of, what is in a workspace, and its users.",
    ),
    (tag::CONTAINERS, "Boards and templates."),
    (
        tag::WIKIS,
        "Wikis and their pages. A wiki is a board of the kind `wiki`: who is on it, its \
         sharing, its files, its place in the sidebar and in the trash are asked as a \
         board's are, under `/containers/{containerId}` with the wiki's identifier.",
    ),
    (
        tag::DIARIES,
        "Diaries and their entries, in the workspace of a user's own space and nowhere \
         else. A diary is a board of the kind `diary` that holds one entry a day and \
         nothing else; its owner alone reads it. What it shares with a board is asked \
         under `/containers/{containerId}` with the diary's identifier: the diary itself \
         (`getContainer`), its title, description and icon (`patchContainer`), the trash \
         (`deleteContainer`, `restoreContainer`, `getDeletedContainers`), its files and \
         their versions and leases, its place in the sidebar, and its archive \
         (`exportContainer`; the archive of the workspace holds its diaries, and an \
         archive that is imported makes each diary anew, in the user's own space only). \
         Its entries are found by `searchWorkspaceContent`, by their writer. What is about \
         the people of a board answers 404 for a diary, as for an identifier of nothing: \
         its members, joining and leaving it, its groups, its link (`/public-link`, \
         `/shared/{containerId}`), invitations to it, and `duplicateContainer`. So do the \
         operations under `/boards/{boardId}` and `/wikis/{wikiId}`: a diary has no \
         blocks but its entries, no cards, no views, no pages, no comments and no \
         templates; and no reference names an entry.",
    ),
    (
        tag::COURSES,
        "Courses and their chapters and units. A course is a container of the kind \
         `course`: who is on it, its groups, invitations, public link, files, place in \
         the sidebar and trash, and export and import work under \
         `/containers/{containerId}` with the course's identifier. Open courses are read \
         and searched without joining, as open wikis are.",
    ),
    (tag::MEMBERS, "Who is on a board, and with which role."),
    (
        tag::PUBLIC_LINKS,
        "Sharing a board with whoever has its public link.",
    ),
    (tag::CARDS, "The blocks of type `card`, as cards."),
    (
        tag::REFERENCES,
        "What the bodies of a workspace name: a body holds the identifier of a card, a \
         page or a unit (the node `reference`), and what that is called is told here, to \
         each user what they read.",
    ),
    (
        tag::SIDEBAR,
        "A user's categories and the boards filed under them.",
    ),
    (tag::FILES, "The files of a board."),
    (tag::ARCHIVES, "Exporting boards and importing them."),
    (tag::SUBSCRIPTIONS, "Subscriptions to blocks."),
    (
        tag::NOTIFICATIONS,
        "What the user was told inside the application: mentions and cards given to them.",
    ),
    (
        tag::TIME,
        "Personal time tracking entries and the single active timer of the user of the \
         session. Every entry is private to its owner across all organizations.",
    ),
    (
        tag::ORGANIZATION,
        "What an organization's administrators manage: the organization, its members' \
         roles, its workspaces and their trash, joining a workspace, and the audit logs.",
    ),
    (
        tag::API_TOKENS,
        "Personal API tokens: what a user gives an agent to read, or read and write, the \
         content of some of their workspaces on their behalf (see \"Personal API tokens\" \
         above).",
    ),
    (
        tag::MCP,
        "The Model Context Protocol server, by which an agent with a personal API token \
         reads and writes what the token reaches through tools, as the operations above \
         do.",
    ),
];
