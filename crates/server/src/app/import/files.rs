//! The files of an import, written to storage before the one transaction that records
//! them with the boards, and what the import does once that transaction is committed.
//!
//! Each file is unpacked from the archive into a temporary file of a staging directory
//! of the import's own, then written to a place marked as being written: what nothing
//! records is taken back, whether the import gives up or its client goes away.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fvoci_model::{Container, ContainerKind, ContainerMember};
use tokio::fs;

use crate::app::App;
use crate::app::container_copies::counted_as;
use crate::app::doomed::Doomed;
use crate::app::loose_objects::WrittenPlace;
use crate::auth::secret::new_secret;
use crate::error::AppError;
use crate::metrics::{ContainerWork, CourseSource};

/// The files an import has written so far, none of them recorded yet: each is given up
/// ([`StagedFiles::give_up`]) unless the transaction that records them commits
/// ([`StagedFiles::settle`]). Dropped, its staging directory goes, and the places it
/// wrote are left to the housekeeping of what is marked as being written.
#[must_use = "the files written must be settled or given up"]
pub(super) struct StagedFiles {
    app: App,
    /// Made with the first file to unpack.
    dir: Option<Doomed>,
    written: Vec<WrittenPlace>,
    bytes: u64,
}

impl StagedFiles {
    /// Nothing written yet, by `app`.
    pub(super) fn begin(app: &App) -> Self {
        Self {
            app: app.clone(),
            dir: None,
            written: Vec::new(),
            bytes: 0,
        }
    }

    /// Where the next file is unpacked before it is stored: one temporary file of the
    /// staging directory, which is made the first time.
    ///
    /// # Errors
    ///
    /// Returns an internal error if the staging directory cannot be made.
    pub(super) async fn unpacked(&mut self) -> Result<PathBuf, AppError> {
        if let Some(dir) = &self.dir {
            return Ok(dir.path().join("entry"));
        }
        let path = self
            .app
            .config()
            .tmp_dir()
            .join(format!("import-stage-{}", new_secret()));
        fs::create_dir_all(&path).await.map_err(|error| {
            AppError::internal(format_args!("cannot create staging directory: {error}"))
        })?;
        let dir = self.dir.insert(Doomed::directory(path));
        Ok(dir.path().join("entry"))
    }

    /// Writes the file unpacked where [`StagedFiles::unpacked`] says to the place
    /// `path`, marked as being written, and takes the unpacked file away. Returns its
    /// size. What is written so far is held against the organization's room.
    ///
    /// # Errors
    ///
    /// Returns what marking the place and writing to it return, an internal error if
    /// the unpacked file cannot be read, and what [`App::check_org_storage_room`]
    /// returns for what is written so far, the file among it. A file written stays
    /// to be given up with the others.
    pub(super) async fn store(&mut self, path: &str) -> Result<u64, AppError> {
        let unpacked = self.unpacked().await?;
        let marked = match self.app.mark_place(path).await {
            Ok(marked) => marked,
            Err(error) => {
                remove_unpacked(&unpacked).await;
                return Err(error);
            }
        };
        let mut file = match fs::File::open(&unpacked).await {
            Ok(file) => file,
            Err(error) => {
                remove_unpacked(&unpacked).await;
                marked.give_up_unused().await;
                return Err(AppError::internal(format_args!(
                    "cannot open an unpacked archive entry: {error}"
                )));
            }
        };
        let written = marked.write(&mut file).await;
        remove_unpacked(&unpacked).await;
        let written = written?;
        let size = written.size();
        self.written.push(written);
        self.bytes = self.bytes.saturating_add(size);
        self.app.check_org_storage_room(self.bytes).await?;
        Ok(size)
    }

    /// Takes back every file written: the import is given up.
    pub(super) async fn give_up(self) {
        for place in self.written {
            place.give_up().await;
        }
        if let Some(dir) = self.dir {
            dir.remove().await;
        }
    }

    /// Lets every file written be: the transaction that recorded them, which took their
    /// marks away, is committed.
    pub(super) async fn settle(self) {
        for place in self.written {
            place.settled_in_transaction();
        }
        if let Some(dir) = self.dir {
            dir.remove().await;
        }
    }
}

async fn remove_unpacked(unpacked: &Path) {
    if let Err(error) = fs::remove_file(unpacked).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, path = %unpacked.display(), "could not remove staged archive entry");
    }
}

impl App {
    /// What an import does once its boards are committed and told of: files each board
    /// that is no template under the default category of each of its new members, and
    /// counts it. A sidebar that cannot file a board is no failure of the import, which
    /// is committed and would be made twice if its client were told to try again.
    pub(super) async fn file_and_count_imported(
        &self,
        containers: &[Container],
        members: &[ContainerMember],
    ) {
        let by_id: HashMap<&str, &Container> = containers
            .iter()
            .map(|container| (container.id.as_str(), container))
            .collect();
        for member in members {
            let Some(container) = by_id.get(member.container_id.as_str()) else {
                continue;
            };
            if container.is_template {
                continue;
            }
            if let Err(error) = self
                .add_containers_to_default_category(
                    &member.user_id,
                    &container.workspace_id,
                    std::slice::from_ref(&member.container_id),
                )
                .await
            {
                tracing::warn!(
                    container_id = %member.container_id,
                    user_id = %member.user_id,
                    %error,
                    "failed to add imported container to default sidebar category after commit"
                );
            }
        }
        for container in containers {
            self.metrics()
                .container_copied(ContainerWork::Import, counted_as(container.kind));
            if container.kind == ContainerKind::Course && !container.is_template {
                self.metrics().course_created(CourseSource::Import);
            }
        }
    }
}
