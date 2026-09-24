use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

use crate::db::workspace::WorkspaceRole;

static PROJECT_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9-]{1,31}$").expect("project key regex"));
static PROJECT_KEY_TRAILING_NUM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-\d+$").expect("project key trailing num regex"));

const RESERVED_KEYS: &[&str] = &[
    "WIKI",
    "PROJECTS",
    "SEARCH",
    "MY-TASKS",
    "TRASH",
    "NOTIFICATIONS",
    "SETTINGS",
    "A",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectKeyError {
    InvalidPattern,
    Reserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProjectPermission {
    None,
    View,
    Edit,
    Manage,
}

impl ProjectPermission {
    pub fn at_least(self, min: Self) -> bool {
        self >= min
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectMemberRole {
    Lead,
    Member,
    Viewer,
}

impl ProjectMemberRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lead => "lead",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "lead" => Some(Self::Lead),
            "member" => Some(Self::Member),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    pub fn permission(self) -> ProjectPermission {
        match self {
            Self::Lead => ProjectPermission::Manage,
            Self::Member => ProjectPermission::Edit,
            Self::Viewer => ProjectPermission::View,
        }
    }
}

pub fn normalize_project_key(key: &str) -> Result<String, ProjectKeyError> {
    let canonical: String = key.nfkc().collect();
    if !PROJECT_KEY_RE.is_match(&canonical) || PROJECT_KEY_TRAILING_NUM_RE.is_match(&canonical) {
        return Err(ProjectKeyError::InvalidPattern);
    }
    if RESERVED_KEYS.contains(&canonical.as_str()) {
        return Err(ProjectKeyError::Reserved);
    }
    Ok(canonical)
}

pub fn workspace_base_permission(role: WorkspaceRole) -> ProjectPermission {
    match role {
        WorkspaceRole::Guest => ProjectPermission::None,
        WorkspaceRole::Member => ProjectPermission::Edit,
        WorkspaceRole::Admin | WorkspaceRole::Owner => ProjectPermission::Manage,
    }
}

pub fn effective_permission(
    workspace_role: WorkspaceRole,
    visibility: &str,
    project_member_role: Option<ProjectMemberRole>,
) -> ProjectPermission {
    if visibility == "workspace" && workspace_role != WorkspaceRole::Guest {
        return workspace_base_permission(workspace_role);
    }
    project_member_role
        .map(ProjectMemberRole::permission)
        .unwrap_or(ProjectPermission::None)
}

pub fn name_is_valid(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= 200
}

pub fn description_is_valid(value: Option<&str>) -> bool {
    match value {
        None => true,
        Some(raw) => raw.chars().count() <= 2000,
    }
}

pub fn icon_is_valid(value: Option<&str>) -> bool {
    match value {
        None => true,
        Some(raw) => raw.chars().count() <= 50,
    }
}

pub fn optional_text_to_db(value: Option<&str>) -> Option<String> {
    value.map(str::trim).and_then(|v| {
        if v.is_empty() {
            None
        } else {
            Some(v.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_key_nfkc_and_pattern() {
        assert!(normalize_project_key("lab").is_err());
        assert_eq!(normalize_project_key("ＡＢ").unwrap(), "AB");
        assert!(normalize_project_key("OPS-5").is_err());
        assert!(normalize_project_key("WIKI").is_err());
        assert_eq!(normalize_project_key("OPS").unwrap(), "OPS");
    }

    #[test]
    fn private_project_requires_membership_even_for_owner() {
        let perm = effective_permission(WorkspaceRole::Owner, "private", None);
        assert_eq!(perm, ProjectPermission::None);
    }

    #[test]
    fn workspace_visible_member_gets_edit() {
        let perm = effective_permission(WorkspaceRole::Member, "workspace", None);
        assert_eq!(perm, ProjectPermission::Edit);
    }
}
