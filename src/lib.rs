// tests/support/libsql_finish_fixture.rs is compiled into these lib tests
// (src/db/mod.rs) and into integration tests, and names the crate as
// `fvoci_server` in both.
#[cfg(all(test, feature = "db-tests"))]
extern crate self as fvoci_server;

pub mod alloc_guard;
pub mod api;
pub mod attachments;
pub mod auth;
pub mod backup_manifest;
pub mod collab;
pub mod collections;
pub mod config;
pub mod db;
pub mod display_id;
pub mod doctor;
pub mod documents;
pub mod error;
pub mod export_zip;
pub mod gantt;
pub mod healthcheck;
pub mod http;
pub mod http_client;
pub mod ics;
pub mod identity;
pub mod import_job;
pub mod init_env;
pub mod integrations;
pub mod jobs;
pub mod license;
pub mod mail;
pub mod native_archive;
pub mod native_history;
pub mod notifications;
pub mod oidc;
pub mod outbox;
pub mod personal_transfer;
pub mod prepare;
pub mod projects;
pub mod push;
pub mod search;
pub mod secret_box;
pub mod secret_maintenance;
pub mod secret_verify;
pub mod settings;
pub mod share_render;
pub mod streams;
pub mod task_timer;
pub mod tasks;
pub mod validate;
