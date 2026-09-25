#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiTokenScope {
    DocumentsRead,
    DocumentsWrite,
    TasksRead,
    TasksWrite,
    ProjectsRead,
    ProjectsManage,
    ShareManage,
    WorkspaceManage,
}

impl ApiTokenScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DocumentsRead => "documents.read",
            Self::DocumentsWrite => "documents.write",
            Self::TasksRead => "tasks.read",
            Self::TasksWrite => "tasks.write",
            Self::ProjectsRead => "projects.read",
            Self::ProjectsManage => "projects.manage",
            Self::ShareManage => "share.manage",
            Self::WorkspaceManage => "workspace.manage",
        }
    }

    fn domain(self) -> &'static str {
        match self {
            Self::DocumentsRead | Self::DocumentsWrite => "documents",
            Self::TasksRead | Self::TasksWrite => "tasks",
            Self::ProjectsRead | Self::ProjectsManage => "projects",
            Self::ShareManage => "share",
            Self::WorkspaceManage => "workspace",
        }
    }
}

pub fn parse_api_token_scope(value: &str) -> Option<ApiTokenScope> {
    match value {
        "documents.read" => Some(ApiTokenScope::DocumentsRead),
        "documents.write" => Some(ApiTokenScope::DocumentsWrite),
        "tasks.read" => Some(ApiTokenScope::TasksRead),
        "tasks.write" => Some(ApiTokenScope::TasksWrite),
        "projects.read" => Some(ApiTokenScope::ProjectsRead),
        "projects.manage" => Some(ApiTokenScope::ProjectsManage),
        "share.manage" => Some(ApiTokenScope::ShareManage),
        "workspace.manage" => Some(ApiTokenScope::WorkspaceManage),
        _ => None,
    }
}

/// write·manage include read of the same domain; workspace·share only have manage.
pub fn grants_api_token_scope(held: &[ApiTokenScope], required: ApiTokenScope) -> bool {
    if held.contains(&required) {
        return true;
    }
    let action_read = matches!(
        required,
        ApiTokenScope::DocumentsRead | ApiTokenScope::TasksRead | ApiTokenScope::ProjectsRead
    );
    action_read && held.iter().any(|scope| scope.domain() == required.domain())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_includes_same_domain_read_only() {
        assert!(grants_api_token_scope(
            &[ApiTokenScope::DocumentsWrite],
            ApiTokenScope::DocumentsRead
        ));
        assert!(grants_api_token_scope(
            &[ApiTokenScope::DocumentsWrite],
            ApiTokenScope::DocumentsWrite
        ));
        assert!(!grants_api_token_scope(
            &[ApiTokenScope::DocumentsWrite],
            ApiTokenScope::TasksRead
        ));
        assert!(!grants_api_token_scope(
            &[ApiTokenScope::WorkspaceManage],
            ApiTokenScope::DocumentsRead
        ));
        assert!(grants_api_token_scope(
            &[ApiTokenScope::ProjectsManage],
            ApiTokenScope::ProjectsRead
        ));
    }
}
