//! Who has a file open in the editor: one at a time, by a lease that the editor holds
//! on to while it is open.
//!
//! A lease is the server's to keep, in this process's memory only: one process serves a
//! database (`server_lock`), and a lease is short. It is taken by one editing session,
//! not by a person: the same person's second window is refused as well. It lasts
//! [`LEASE_TERM`] from when it was taken or held on to last, and goes when its holder
//! lets it go or when the term ends, whichever is first; a window that died does not
//! keep it.
//!
//! A lease keeps nobody from writing a version: a write is held to the version it was
//! made from (`file_versions`), and that is all that keeps one from going over another.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use fvoci_model::{FileLease, HeldFileLease, WorkspaceRole, new_id};
use fvoci_store::Tenant;
use tokio::time::Instant;

use super::App;
use super::files::FILE;
use crate::error::{AppError, Reason};

/// How long a lease lasts from when it was taken or held on to last. Three times the
/// web app's 30 seconds between holding on, as with presence: a browser wakes the
/// timers of a hidden tab about once a minute.
pub(crate) const LEASE_TERM: Duration = Duration::from_secs(90);

/// The file a lease is of: an organization's board's (or wiki's) file. The same
/// identifiers in another organization are another file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LeasedFile {
    tenant: Tenant,
    container_id: String,
    file_id: String,
}

/// A lease held.
#[derive(Debug)]
struct Lease {
    id: String,
    user_id: String,
    /// When it ends unless held on to.
    until: Instant,
}

/// The leases held on the server's files.
#[derive(Debug, Default)]
pub(crate) struct FileLeases {
    held: Mutex<HashMap<LeasedFile, Lease>>,
}

/// Why a lease was not given: somebody else holds the file's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Taken;

impl FileLeases {
    /// Gives `user_id` the lease of `file` for [`LEASE_TERM`] from `now`: holds on to
    /// `lease_id` if that is the user's and has not ended, and takes a new one if nobody
    /// holds the file's. The lease held.
    fn hold(
        &self,
        file: LeasedFile,
        user_id: &str,
        lease_id: &str,
        now: Instant,
    ) -> Result<String, Taken> {
        // The map has nothing a panic could leave half done.
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        // Ended leases go here, so that none stays past its holder.
        held.retain(|_, lease| lease.until > now);
        let until = now + LEASE_TERM;
        if let Some(lease) = held.get_mut(&file) {
            if lease.id != lease_id || lease.user_id != user_id {
                return Err(Taken);
            }
            lease.until = until;
            return Ok(lease.id.clone());
        }
        let id = new_id();
        let lease = Lease {
            id: id.clone(),
            user_id: user_id.to_owned(),
            until,
        };
        held.insert(file, lease);
        Ok(id)
    }

    /// Lets `user_id`'s lease `lease_id` of `file` go; one that is another's, or no
    /// more, is left as it is.
    fn release(&self, file: &LeasedFile, user_id: &str, lease_id: &str) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if held
            .get(file)
            .is_some_and(|lease| lease.id == lease_id && lease.user_id == user_id)
        {
            held.remove(file);
        }
    }

    /// Who holds the lease of `file` at `now`, if anybody.
    fn holder(&self, file: &LeasedFile, now: Instant) -> Option<String> {
        let held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.get(file)
            .filter(|lease| lease.until > now)
            .map(|lease| lease.user_id.clone())
    }
}

impl App {
    /// Gives the user `user_id` the lease of the file `file_id` of the board or wiki
    /// `container_id`: holds on to `lease_id` (the user's, as this gave it before), or
    /// takes a new one if nobody holds the file's. Whoever calls this may edit the board.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the board has no such file, a conflict with the
    /// reason `file_being_edited` if another lease of the file is held, and the store's
    /// error.
    pub async fn hold_file_lease(
        &self,
        container_id: &str,
        file_id: &str,
        user_id: &str,
        lease_id: &str,
    ) -> Result<HeldFileLease, AppError> {
        let file = self.leased_file(container_id, file_id).await?;
        let leases = self.file_leases();
        match leases.hold(file, user_id, lease_id, Instant::now()) {
            Ok(lease_id) => Ok(HeldFileLease {
                lease_id,
                seconds: u32::try_from(LEASE_TERM.as_secs()).unwrap_or(u32::MAX),
            }),
            Err(Taken) => Err(AppError::conflict("somebody else is editing the file")
                .with_reason(Reason::FileBeingEdited)),
        }
    }

    /// Lets the user's lease `lease_id` of the file go. Nothing is said of a lease that
    /// is the user's no more, or never was.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the board has no such file, and the store's error.
    pub async fn release_file_lease(
        &self,
        container_id: &str,
        file_id: &str,
        user_id: &str,
        lease_id: &str,
    ) -> Result<(), AppError> {
        let file = self.leased_file(container_id, file_id).await?;
        self.file_leases().release(&file, user_id, lease_id);
        Ok(())
    }

    /// Whether somebody edits the file, as `seer_id`, in `role` in the workspace
    /// `workspace_id`, is told it: who, only for one the seer sees among the people of
    /// the workspace (as presence names them), and for the seer.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the board has no such file, and the store's error.
    pub async fn file_lease(
        &self,
        container_id: &str,
        file_id: &str,
        seer_id: &str,
        workspace_id: &str,
        role: WorkspaceRole,
    ) -> Result<FileLease, AppError> {
        let file = self.leased_file(container_id, file_id).await?;
        let Some(holder) = self.file_leases().holder(&file, Instant::now()) else {
            return Ok(FileLease::default());
        };
        let seen = holder == seer_id
            || !self
                .users_in_workspace(seer_id, workspace_id, role, std::slice::from_ref(&holder))
                .await?
                .is_empty();
        Ok(FileLease {
            editing: true,
            editor_id: if seen { holder } else { String::new() },
        })
    }

    /// The file of a lease: the board's file `file_id`, in this handle's organization.
    async fn leased_file(&self, container_id: &str, file_id: &str) -> Result<LeasedFile, AppError> {
        if self.file_record(container_id, file_id).await?.is_none() {
            return Err(AppError::not_found(FILE));
        }
        Ok(LeasedFile {
            tenant: self.store().tenant(),
            container_id: container_id.to_owned(),
            file_id: file_id.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(file_id: &str) -> LeasedFile {
        LeasedFile {
            tenant: Tenant::Unset,
            container_id: "board".to_owned(),
            file_id: file_id.to_owned(),
        }
    }

    #[test]
    fn a_lease_keeps_the_file_from_another_until_it_is_let_go() {
        let leases = FileLeases::default();
        let now = Instant::now();
        let ann = leases
            .hold(file("a"), "ann", "", now)
            .expect("nobody holds it");
        assert_eq!(leases.hold(file("a"), "bob", "", now), Err(Taken));
        assert_eq!(leases.holder(&file("a"), now).as_deref(), Some("ann"));
        // Ann's other window takes none either: a lease is one editor's.
        assert!(leases.hold(file("a"), "ann", "", now).is_err());
        // Another file is another lease.
        assert!(leases.hold(file("b"), "bob", "", now).is_ok());
        // Somebody else's lease is not let go by naming it.
        leases.release(&file("a"), "bob", &ann);
        assert_eq!(leases.holder(&file("a"), now).as_deref(), Some("ann"));
        leases.release(&file("a"), "ann", &ann);
        assert_eq!(leases.holder(&file("a"), now), None);
        assert!(leases.hold(file("a"), "bob", "", now).is_ok());
    }

    #[test]
    fn a_lease_held_on_to_lasts_its_term_from_then_and_one_not_held_on_to_ends() {
        let leases = FileLeases::default();
        let start = Instant::now();
        let lease = leases
            .hold(file("a"), "ann", "", start)
            .expect("nobody holds it");
        let later = start + LEASE_TERM - Duration::from_secs(1);
        assert_eq!(
            leases.hold(file("a"), "ann", &lease, later).as_deref(),
            Ok(lease.as_str()),
            "held on to, it is the same lease"
        );
        // Its term is counted from when it was held on to.
        let past_first_term = start + LEASE_TERM + Duration::from_secs(1);
        assert!(leases.hold(file("a"), "bob", "", past_first_term).is_err());
        assert_eq!(
            leases.holder(&file("a"), past_first_term).as_deref(),
            Some("ann")
        );
        // Not held on to again, it ends with its term: then nobody holds it, and
        // whoever asks next has it.
        let ended = later + LEASE_TERM;
        assert_eq!(leases.holder(&file("a"), ended), None);
        let bob = leases
            .hold(file("a"), "bob", "", ended)
            .expect("the lease ended");
        assert_ne!(bob, lease);
        // Ann's ended lease is no more hers to hold on to.
        assert!(leases.hold(file("a"), "ann", &lease, ended).is_err());
    }

    #[test]
    fn an_ended_lease_held_on_to_is_taken_anew_when_nobody_else_holds_the_file() {
        let leases = FileLeases::default();
        let start = Instant::now();
        let lease = leases
            .hold(file("a"), "ann", "", start)
            .expect("nobody holds it");
        // A tab that slept past the term asks again with the lease it had.
        let woken = start + LEASE_TERM * 2;
        let again = leases
            .hold(file("a"), "ann", &lease, woken)
            .expect("nobody took it");
        assert_eq!(leases.holder(&file("a"), woken).as_deref(), Some("ann"));
        assert!(leases.hold(file("a"), "ann", &again, woken).is_ok());
    }

    #[test]
    fn ended_leases_are_not_kept() {
        let leases = FileLeases::default();
        let start = Instant::now();
        for each in ["a", "b", "c"] {
            leases
                .hold(file(each), "ann", "", start)
                .expect("nobody holds it");
        }
        leases
            .hold(file("d"), "bob", "", start + LEASE_TERM)
            .expect("nobody holds it");
        let held = leases.held.lock().expect("lock poisoned");
        assert_eq!(held.len(), 1, "only the lease taken last is held");
    }
}
