//! The configuration a server hands to its web client.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The part of the server configuration that the web client needs.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ClientConfig {
    /// Whether boards and wikis may be shared publicly through a read token: the
    /// setting `allow_public_links`.
    pub allow_public_links: bool,
    /// How other users' names are displayed: the setting `name_display`.
    pub name_display: String,
    /// The server's feature flags.
    pub feature_flags: BTreeMap<String, String>,
    /// The largest upload the server accepts, in bytes; always more than zero (100 MiB
    /// unless the server is configured otherwise): the setting `max_file_bytes`.
    pub max_file_bytes: i64,
    /// The largest archive an import takes, in bytes; always more than zero (256 MiB
    /// unless the server is configured otherwise): the setting `max_import_bytes`.
    pub max_import_bytes: i64,
    /// Whether what cards and pages say can be searched (`searchWorkspaceContent`):
    /// always, on this server. It searches its search server, or its database where none
    /// is configured; a search server that does not answer, or cannot hold the index,
    /// answers each search 503 (`search_unavailable`).
    pub search_enabled: bool,
    /// How many whole days deleted items stay in the trash before housekeeping purges
    /// them (`trash_retention_days`, default 30).
    pub trash_retention_days: u32,
}
