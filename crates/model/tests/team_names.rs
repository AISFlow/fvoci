//! The name of a team: its rule, shared with an organization's, and its key.

/// A team name's key folds letter case alike for both databases, beyond ASCII too, and
/// keeps apart what only looks alike in some letter case.
#[test]
fn a_team_name_key_folds_letter_case_and_nothing_else() {
    let key = fvoci_model::team_name_key;
    for (one, other) in [
        ("Design", "DESIGN"),
        ("Straße", "STRASSE"),
        ("ΟΔΟΣ", "οδοσ"),
        ("ſun", "SUN"),
        ("İstanbul", "i\u{307}stanbul"),
    ] {
        assert_eq!(key(one), key(other), "{one:?} and {other:?} are one name");
    }
    for (one, other) in [("Kırmızı", "Kirmizi"), ("ıi", "ii"), ("Café", "Cafe")] {
        assert_ne!(key(one), key(other), "{one:?} and {other:?} are two names");
    }
    assert_eq!(key("KIRMIZI"), "kirmizi");
    assert_eq!(key("ıı"), "ıı");
}

/// A team name is read by the organization's rule, about the member `name`.
#[test]
fn a_team_name_is_read_by_the_rule_of_an_organization_name() {
    use fvoci_model::{Named, RefusalReason, required_name, team_name};
    assert_eq!(team_name("  디자인  "), Ok("디자인".to_owned()));
    assert_eq!(
        team_name(&"가".repeat(64)).map(|name| name.len()),
        Ok(64 * 3)
    );
    for (raw, reason) in [
        ("   ".to_owned(), RefusalReason::Required),
        ("가".repeat(65), RefusalReason::TooLong),
        ("a\u{202E}b".to_owned(), RefusalReason::InvalidCharacters),
        ("a\tb".to_owned(), RefusalReason::InvalidCharacters),
    ] {
        let team = team_name(&raw).expect_err("refused");
        let organization = required_name(&raw, Named::Organization).expect_err("refused");
        assert_eq!(team.reason(), reason, "{raw:?}");
        assert_eq!(organization.reason(), reason, "{raw:?}: one rule");
        assert_eq!(team.field(), "name");
        assert_eq!(organization.field(), "organizationName");
    }
}
