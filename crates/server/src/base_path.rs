//! The path the server is served under: `FVOCI_BASE_PATH`.

use anyhow::bail;

use crate::BASE_PATH_ENV;

/// The path the web app and the API are served under (`FVOCI_BASE_PATH`),
/// as the server uses it: without a final `/`, empty for none.
///
/// # Errors
///
/// Returns an error for what the router would read as a pattern or could never match,
/// with words, rather than a panic of the router or a server that answers 404 to
/// everything.
pub fn checked_base_path(base_path: &str) -> anyhow::Result<&str> {
    let base_path = base_path.trim_end_matches('/');
    let is_malformed = !base_path.starts_with('/')
        || base_path.contains(['{', '}', '*', '?', '#', '\\'])
        || base_path.contains("//")
        || base_path.contains("/:")
        || base_path.chars().any(char::is_whitespace);
    if !base_path.is_empty() && is_malformed {
        bail!(
            "{BASE_PATH_ENV} must be empty or an absolute path such as /boards, not {base_path:?}"
        );
    }
    Ok(base_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_base_path_loses_its_last_slash() {
        assert_eq!(checked_base_path("").expect("none"), "");
        assert_eq!(checked_base_path("/").expect("the root"), "");
        assert_eq!(checked_base_path("/boards/").expect("a path"), "/boards");
    }

    #[test]
    fn a_base_path_the_router_cannot_take_is_refused() {
        for refused in ["boards", "/a//b", "/a/{id}", "/a b", "/:x"] {
            assert!(checked_base_path(refused).is_err(), "{refused}");
        }
    }
}
