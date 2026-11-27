use super::{all, find};
use crate::error::{ErrorKind, Reason};

#[test]
fn every_kind_and_every_reason_is_a_problem_type_with_a_name_of_its_own() {
    let problems = all();
    assert_eq!(
        problems.len(),
        ErrorKind::ALL.len() + Reason::ALL.len(),
        "one for each"
    );
    for problem in &problems {
        assert_eq!(
            find(&problem.name).as_ref(),
            Some(problem),
            "found by its name, and the name is no other's"
        );
        assert!(
            problem
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '-'),
            "a name is lower-case words with hyphens: {}",
            problem.name
        );
        assert!(!problem.title.is_empty() && !problem.description.is_empty());
    }
    assert_eq!(find("nothing"), None);
}

#[test]
fn the_page_of_a_kind_says_its_type_title_and_what_it_means() {
    let page = find("last-admin").expect("a kind").page("");
    assert!(
        page.starts_with("Last administrator\n\ntype: /problems/last-admin\n"),
        "{page}"
    );
    assert!(page.contains("reason: last_admin\n"), "{page}");
    assert!(
        page.ends_with("\n\nStatus 409. The change would leave the board, the workspace or the server without an administrator. Make somebody else one first.\n"),
        "{page}"
    );
    let page = find("not-found").expect("a kind").page("/base");
    assert!(
        page.contains("type: /base/problems/not-found\nstatus: 404\n"),
        "{page}"
    );
    assert!(!page.contains("reason:"), "{page}");
}
