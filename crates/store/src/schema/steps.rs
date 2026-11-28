//! Ordered SQLite and PostgreSQL schema migration steps.

/// One migration: its version, its name, and its SQL for each database kind.
pub(crate) struct Step {
    pub(super) version: i64,
    pub(super) description: &'static str,
    pub(super) sqlite: &'static str,
    pub(super) postgres: &'static str,
}

/// Every migration, oldest first. Applied ones are never edited: add a new one.
pub(crate) const STEPS: [Step; 5] = [
    Step {
        version: 1,
        description: "baseline",
        sqlite: include_str!("../../schema/sqlite/0001_baseline.sql"),
        postgres: include_str!("../../schema/postgres/0001_baseline.sql"),
    },
    Step {
        version: 2,
        description: "card events",
        sqlite: include_str!("../../schema/sqlite/0002_card_events.sql"),
        postgres: include_str!("../../schema/postgres/0002_card_events.sql"),
    },
    Step {
        version: 3,
        description: "reactions",
        sqlite: include_str!("../../schema/sqlite/0003_reactions.sql"),
        postgres: include_str!("../../schema/postgres/0003_reactions.sql"),
    },
    Step {
        version: 4,
        description: "favorites",
        sqlite: include_str!("../../schema/sqlite/0004_favorites.sql"),
        postgres: include_str!("../../schema/postgres/0004_favorites.sql"),
    },
    Step {
        version: 5,
        description: "notification archive",
        sqlite: include_str!("../../schema/sqlite/0005_notification_archive.sql"),
        postgres: include_str!("../../schema/postgres/0005_notification_archive.sql"),
    },
];

/// What takes each migration after the baseline away again, by its version, newest
/// first, for the tests alone: a database brought up to date is made the one the last
/// server before the baseline left, whose schema is the baseline's
/// (`testing::write_the_record_before_the_baseline`). Statements for both kinds of
/// database, run in their order on a database the migrations brought up to date; a
/// table or an index goes `IF EXISTS`. A column cannot: SQLite has no `IF EXISTS` for
/// one, and drops none an index names, so the index goes first. A migration added to
/// [`STEPS`] adds its line here: a test asks that every one after the baseline has one.
#[cfg(any(test, feature = "test-util"))]
pub(crate) const UNDONE_FOR_TESTS: [(i64, &[&str]); 4] = [
    (
        5,
        &[
            "DROP INDEX IF EXISTS notifications_inbox_idx",
            "ALTER TABLE notifications DROP COLUMN archived_at",
        ],
    ),
    (4, &["DROP TABLE IF EXISTS favorites"]),
    (3, &["DROP TABLE IF EXISTS reactions"]),
    (2, &["DROP TABLE IF EXISTS card_events"]),
];
