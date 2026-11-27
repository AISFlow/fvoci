//! What was taken back, kept so that a subscription checked meanwhile is checked again
//! when, and only when, something that concerns it was: another user's logout, or a
//! change in another organization or workspace, does not overtake it.

use std::collections::HashMap;

/// How many things taken back are kept by name before they are forgotten together.
/// Forgetting has every check in flight read once more: a larger bound forgets less
/// often, for more memory.
const KEPT: usize = 4096;

/// When each thing a revocation concerns was last taken back, numbered in one
/// sequence: a check notes the number before it reads the store ([`Self::now`]), and
/// asks as it registers whether what concerns it was taken back after that. One
/// sequence, so that a check that learns its board or its organization only as it
/// reads can still be measured against it.
///
/// What is kept by name is bounded ([`KEPT`]): past it, it is forgotten, and a check
/// that began before it was is checked again, whatever it concerns.
#[derive(Debug, Default)]
pub(super) struct Revocations {
    /// The number of the latest revocation.
    now: u64,
    /// The number at which what was kept by name was last forgotten.
    forgotten: u64,
    /// How many things are kept by name now.
    kept: usize,
    /// What may end anybody's access in any workspace: an organization whose
    /// workspaces could not be read beginning to take no password.
    everyone: u64,
    /// A user whose connections were closed: a session of theirs ended.
    users: HashMap<String, u64>,
    /// A user whose access to one workspace ended or changed, by user and workspace.
    users_in_workspaces: HashMap<String, HashMap<String, u64>>,
    /// A user whose access was taken back in an organization (an administrator's
    /// joining, who runs the server), by user and organization.
    users_in_orgs: HashMap<String, HashMap<String, u64>>,
    /// A workspace that went to the trash, or whose organization began to take no
    /// password.
    workspaces: HashMap<String, u64>,
    /// A board whose read token stopped opening it.
    containers: HashMap<String, u64>,
}

/// What a workspace subscription is of, to be measured against what was taken back.
#[derive(Debug, Clone, Copy)]
pub(super) struct Followed<'a> {
    pub(super) user: &'a str,
    pub(super) workspace: &'a str,
    pub(super) org: &'a str,
}

impl Revocations {
    /// The number of the latest revocation: what is read from now on is measured
    /// against it.
    pub(super) fn now(&self) -> u64 {
        self.now
    }

    /// Whether what concerns the subscription was taken back after `since`.
    pub(super) fn of_workspace_since(&self, followed: Followed<'_>, since: u64) -> bool {
        self.unknown_since(since)
            || after(&self.users, followed.user, since)
            || after_of(
                &self.users_in_workspaces,
                followed.user,
                followed.workspace,
                since,
            )
            || after_of(&self.users_in_orgs, followed.user, followed.org, since)
            || after(&self.workspaces, followed.workspace, since)
    }

    /// Whether what concerns a connection of `user_id` (empty without a session)
    /// following blocks of `container_id` by its read token was taken back after `since`.
    pub(super) fn of_container_since(&self, user_id: &str, container_id: &str, since: u64) -> bool {
        self.unknown_since(since)
            || after(&self.users, user_id, since)
            || after(&self.containers, container_id, since)
    }

    pub(super) fn everyone(&mut self) {
        self.everyone = self.next();
    }

    pub(super) fn user(&mut self, user_id: &str) {
        if !user_id.is_empty() {
            let now = self.next();
            let earlier = self.users.insert(user_id.to_owned(), now);
            self.count(earlier);
        }
    }

    pub(super) fn user_in_workspace(&mut self, user_id: &str, workspace_id: &str) {
        let now = self.next();
        let earlier = self
            .users_in_workspaces
            .entry(user_id.to_owned())
            .or_default()
            .insert(workspace_id.to_owned(), now);
        self.count(earlier);
    }

    pub(super) fn user_in_org(&mut self, user_id: &str, org_id: &str) {
        let now = self.next();
        let earlier = self
            .users_in_orgs
            .entry(user_id.to_owned())
            .or_default()
            .insert(org_id.to_owned(), now);
        self.count(earlier);
    }

    pub(super) fn workspace(&mut self, workspace_id: &str) {
        let now = self.next();
        let earlier = self.workspaces.insert(workspace_id.to_owned(), now);
        self.count(earlier);
    }

    pub(super) fn container(&mut self, container_id: &str) {
        let now = self.next();
        let earlier = self.containers.insert(container_id.to_owned(), now);
        self.count(earlier);
    }

    /// Whether everything may have been taken back after `since`, or what was may
    /// have been forgotten.
    fn unknown_since(&self, since: u64) -> bool {
        self.everyone > since || self.forgotten > since
    }

    /// The number of a new revocation, after forgetting what is kept by name if it
    /// is as much as may be kept.
    fn next(&mut self) -> u64 {
        if self.kept >= KEPT {
            self.users.clear();
            self.users_in_workspaces.clear();
            self.users_in_orgs.clear();
            self.workspaces.clear();
            self.containers.clear();
            self.kept = 0;
            self.forgotten = self.now;
        }
        self.now += 1;
        self.now
    }

    /// Counts what was just kept by name, unless it was kept already.
    fn count(&mut self, earlier: Option<u64>) {
        if earlier.is_none() {
            self.kept += 1;
        }
    }
}

fn after(last: &HashMap<String, u64>, key: &str, since: u64) -> bool {
    last.get(key).is_some_and(|at| *at > since)
}

fn after_of(
    last: &HashMap<String, HashMap<String, u64>>,
    user: &str,
    of: &str,
    since: u64,
) -> bool {
    last.get(user)
        .is_some_and(|of_user| after(of_user, of, since))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn followed<'a>(user_id: &'a str, workspace_id: &'a str, org_id: &'a str) -> Followed<'a> {
        Followed {
            user: user_id,
            workspace: workspace_id,
            org: org_id,
        }
    }

    #[test]
    fn a_revocation_concerns_what_it_names_alone() {
        let mut revoked = Revocations::default();
        let before = revoked.now();
        revoked.user_in_workspace("one", "w1");
        revoked.user_in_org("two", "o2");
        revoked.workspace("w3");
        revoked.container("b1");
        assert!(revoked.of_workspace_since(followed("one", "w1", "o1"), before));
        assert!(
            !revoked.of_workspace_since(followed("one", "w2", "o1"), before),
            "another workspace of the same user"
        );
        assert!(revoked.of_workspace_since(followed("two", "w2", "o2"), before));
        assert!(
            !revoked.of_workspace_since(followed("two", "w1", "o1"), before),
            "the same user in another organization"
        );
        assert!(revoked.of_workspace_since(followed("three", "w3", "o1"), before));
        assert!(revoked.of_container_since("", "b1", before));
        assert!(!revoked.of_container_since("", "b2", before));
        let after = revoked.now();
        assert!(!revoked.of_workspace_since(followed("one", "w1", "o1"), after));
        revoked.user("one");
        assert!(revoked.of_workspace_since(followed("one", "w2", "o2"), after));
        assert!(revoked.of_container_since("one", "b2", after));
        revoked.everyone();
        assert!(revoked.of_workspace_since(followed("four", "w4", "o4"), after));
        assert!(revoked.of_container_since("", "b2", after));
    }

    /// What is kept by name is bounded; a check that began before it was forgotten is
    /// checked again, and one that began after is measured as before.
    #[test]
    fn what_is_forgotten_checks_older_subscriptions_again() {
        let mut revoked = Revocations::default();
        let before = revoked.now();
        for container in 0..=KEPT {
            revoked.container(&format!("b{container}"));
        }
        assert!(revoked.kept <= KEPT, "{} kept by name", revoked.kept);
        assert!(
            revoked.of_workspace_since(followed("one", "w1", "o1"), before),
            "begun before it was forgotten"
        );
        let after = revoked.now();
        assert!(!revoked.of_workspace_since(followed("one", "w1", "o1"), after));
        revoked.container("b-late");
        assert!(revoked.of_container_since("", "b-late", after));
        assert!(!revoked.of_container_since("", "b0", after));
    }

    /// A check begun as much is kept as may be is not checked again by the forgetting
    /// that the next revocation does, only by what that revocation concerns.
    #[test]
    fn a_check_begun_at_the_bound_is_measured_as_before() {
        let mut revoked = Revocations::default();
        for container in 0..KEPT {
            revoked.container(&format!("b{container}"));
        }
        assert_eq!(revoked.kept, KEPT, "as much as may be kept");
        let edge = revoked.now();
        revoked.container("x");
        assert_eq!(revoked.kept, 1, "the rest forgotten");
        assert!(
            !revoked.of_workspace_since(followed("one", "w1", "o1"), edge),
            "nothing it concerns was forgotten after it began"
        );
        assert!(revoked.of_container_since("", "x", edge));
    }
}
