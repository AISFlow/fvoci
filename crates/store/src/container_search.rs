//! Boards found by a search term, among those a user can reach.

use fvoci_model::{Container, ContainerKinds, ContainerSearchField, PublicContainers};

use crate::Store;
use crate::containers;
use crate::db::{Dialect, Scope};
use crate::error::StoreError;

impl Store {
    /// Returns the boards of `kinds`, of any workspace, that match a search term and that
    /// a user is a member of or, with [`PublicContainers::Include`], that are open, oldest
    /// first. Templates are left out.
    ///
    /// An empty `term` matches every board. Searching by title matches boards whose
    /// title contains every space-separated word of the term, ignoring case; searching
    /// by property name matches boards that have a property of that name.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn search_containers_for_user(
        &self,
        term: &str,
        search_field: ContainerSearchField,
        user_id: &str,
        public_containers: PublicContainers,
        kinds: ContainerKinds,
    ) -> Result<Vec<Container>, StoreError> {
        self.with_conn(async |scope| {
            search_containers_for_user(scope, term, search_field, user_id, public_containers, kinds)
                .await
        })
        .await
    }
}

/// Whether the title contains every space-separated word of `term`, ignoring case.
///
/// Compared here rather than in SQL, so that both databases fold every letter the same
/// way (SQLite's `lower()` folds ASCII letters only) and `%` and `_` are characters
/// like any other. Words are split on single spaces, so consecutive spaces produce empty
/// tokens that match every title.
fn title_has_words(title: &str, term: &str) -> bool {
    let title = title.to_lowercase();
    term.trim()
        .split(' ')
        .all(|word| title.contains(&word.to_lowercase()))
}

/// Keeps the boards whose title has the words of `term`; an empty term keeps all.
pub(crate) fn with_title_words(mut containers: Vec<Container>, term: &str) -> Vec<Container> {
    if !term.is_empty() {
        containers.retain(|container| title_has_words(&container.title, term));
    }
    containers
}

async fn search_containers_for_user(
    scope: &mut Scope<'_>,
    term: &str,
    search_field: ContainerSearchField,
    user_id: &str,
    public_containers: PublicContainers,
    kinds: ContainerKinds,
) -> Result<Vec<Container>, StoreError> {
    // No board's property has U+0000 in its name: it is refused when a board is stored,
    // and PostgreSQL refuses it as a parameter.
    if search_field == ContainerSearchField::PropertyName && term.contains('\0') {
        return Ok(Vec::new());
    }
    let mut sql = containers::select_containers_of_user(scope, user_id, public_containers, kinds);
    if !term.is_empty() && search_field == ContainerSearchField::PropertyName {
        // A board property of that name, whatever its value, `null` too; a name is no
        // path, so `a.b` is a name with a dot.
        match scope.dialect() {
            Dialect::Postgres => {
                sql.push_bind(" AND b.properties -> ", term)
                    .push(" IS NOT NULL");
            }
            Dialect::Sqlite => {
                sql.push_bind(
                    " AND EXISTS (SELECT 1 FROM json_each(b.properties) AS p WHERE p.key = ",
                    term,
                )
                .push(")");
            }
        }
    }
    sql.push(containers::OLDEST_CONTAINER_FIRST);
    let containers = containers::fetch_containers(scope, sql).await?;
    Ok(match search_field {
        ContainerSearchField::Title => with_title_words(containers, term),
        ContainerSearchField::PropertyName => containers,
    })
}
