use serde_json::json;

use super::*;

fn members(value: serde_json::Value) -> JsonMap {
    match value {
        serde_json::Value::Object(members) => members,
        other => panic!("the test's body must be an object, not {other}"),
    }
}

fn patched(value: serde_json::Value) -> UserSettings {
    let patch = UserSettingsPatch::from_members(&members(value)).expect("a valid change");
    let (set, _reset) = patch.changes();
    UserSettings::from_stored(set.into_iter().map(|(name, value)| (name.as_str(), value)))
}

#[test]
fn the_defaults_are_what_is_answered_when_nothing_is_stored() {
    assert_eq!(
        serde_json::to_value(UserSettings::from_stored([])).expect("settings are JSON"),
        json!({
            "timeZone": "auto", "weekStart": "auto", "hourCycle": "auto",
            "theme": "auto", "cardOpen": "center", "density": "auto", "haptics": true,
            "language": "auto", "notifyMentions": true, "notifyAssignments": true,
            "notifyComments": true, "notifyDue": true,
        })
    );
}

#[test]
fn each_kind_of_notification_is_switched_off_and_on_by_its_own_setting() {
    use crate::NotificationKind;
    let kinds = [
        ("notifyMentions", NotificationKind::Mention),
        ("notifyAssignments", NotificationKind::Assignment),
        ("notifyComments", NotificationKind::Comment),
        ("notifyDue", NotificationKind::DueSoon),
    ];
    for (name, kind) in kinds {
        let off = patched(json!({name: false}));
        for (_, other) in kinds {
            assert_eq!(off.notifies(other), other != kind, "{name} off, {other}");
        }
        assert!(patched(json!({name: true})).notifies(kind), "{name} on");
        let refused = UserSettingsPatch::from_members(&members(json!({name: "off"})));
        assert!(refused.is_err(), "{name} takes true or false alone");
    }
}

#[test]
fn every_value_of_every_setting_is_stored_and_read_back() {
    let cases = [
        ("weekStart", WeekStart::WORDS),
        ("hourCycle", HourCycle::WORDS),
        ("theme", Theme::WORDS),
        ("cardOpen", CardOpen::WORDS),
        ("density", Density::WORDS),
        (
            "timeZone",
            &[
                "auto",
                "Asia/Seoul",
                "UTC",
                "America/Argentina/Buenos_Aires",
            ][..],
        ),
    ];
    for (name, words) in cases {
        for word in words {
            let settings = patched(json!({name: word}));
            let answered = serde_json::to_value(&settings).expect("settings are JSON");
            assert_eq!(answered[name], json!(word), "{name} = {word}");
        }
    }
    for flag in [true, false] {
        assert_eq!(patched(json!({"haptics": flag})).haptics, flag, "haptics");
    }
    for tag in ["auto", "ko", "en-US", "zh-Hant-TW"] {
        assert_eq!(
            patched(json!({"language": tag})).language.as_str(),
            tag,
            "language"
        );
    }
}

#[test]
fn a_name_the_database_keeps_for_compatibility_is_kept_as_given() {
    let settings = patched(json!({"timeZone": "Asia/Calcutta"}));
    assert_eq!(settings.time_zone.as_str(), "Asia/Calcutta");
}

#[test]
fn a_time_zone_is_a_name_of_the_database_as_it_spells_it() {
    for name in [
        "Asia/Seoul",
        "UTC",
        "Etc/UTC",
        "Etc/GMT+5",
        "Etc/GMT-14",
        "America/Argentina/Buenos_Aires",
        "America/Ciudad_Juarez",
        "Europe/Kyiv",
        "Asia/Calcutta",
        "Asia/Saigon",
        "US/Pacific",
        "GMT0",
        "EST5EDT",
    ] {
        let zone: TimeZone = name.parse().expect("a name of the database");
        assert_eq!(zone.as_str(), name, "kept as given");
    }
    for refused in [
        "",
        "asia/seoul",
        "ASIA/SEOUL",
        "utc",
        "Factory",
        "Etc/Unknown",
        "Etc/GMT+13",
        "UTC+09",
        "+09:00",
        "Z",
        " Asia/Seoul",
        "Asia/Seoul ",
        "/Asia/Seoul",
        "Asia//Seoul",
        "../etc/passwd",
        "posix/UTC",
        "right/UTC",
        "localtime",
        "posixrules",
        "zone.tab",
        "Auto",
    ] {
        assert!(
            refused.parse::<TimeZone>().is_err(),
            "{refused:?} is no name"
        );
    }
}

#[test]
fn what_is_no_value_of_a_setting_is_refused_with_its_name() {
    let cases = [
        json!({"timeZone": "Mars/Olympus_Mons"}),
        json!({"timeZone": "asia/seoul"}),
        json!({"timeZone": "Auto"}),
        json!({"timeZone": ""}),
        json!({"timeZone": 9}),
        json!({"weekStart": "tuesday"}),
        json!({"weekStart": "Monday"}),
        json!({"hourCycle": "h24"}),
        json!({"hourCycle": 12}),
        json!({"theme": "sepia"}),
        json!({"theme": true}),
        json!({"cardOpen": "auto"}),
        json!({"cardOpen": ["side"]}),
        json!({"density": "cozy"}),
        json!({"haptics": "true"}),
        json!({"haptics": 1}),
        json!({"haptics": "auto"}),
    ];
    for case in cases {
        let name = case.as_object().expect("an object").keys().next().cloned();
        let refused = UserSettingsPatch::from_members(&members(case.clone()))
            .expect_err("no value of the setting");
        let refused: Vec<_> = refused.into_iter().collect();
        assert_eq!(refused.len(), 1, "{case}: {refused:?}");
        assert_eq!(refused[0].reason, RefusalReason::Invalid, "{case}");
        assert_eq!(
            Some(refused[0].pointer.trim_start_matches('/')),
            name.as_deref(),
            "{case}"
        );
    }
    let message = UserSettingsPatch::from_members(&members(json!({"cardOpen": "auto"})))
        .expect_err("no value")
        .to_string();
    assert_eq!(
        message, "cardOpen must be one of \"center\", \"side\", \"full\"",
        "the refusal says what the setting can be"
    );
}

#[test]
fn a_name_that_is_no_setting_is_refused() {
    for name in ["timezone", "locale", "", "TimeZone"] {
        let refused = UserSettingsPatch::from_members(&members(json!({name: "auto"})))
            .expect_err("no such setting");
        let wanted = SettingsError::UnknownSetting {
            name: name.to_owned(),
        };
        assert_eq!(wanted.field(), name);
        let refused: Vec<_> = refused.into_iter().collect();
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(refused[0].pointer, format!("/{name}"));
        assert_eq!(refused[0].reason, RefusalReason::Unknown);
        assert_eq!(refused[0].detail, wanted.to_string());
    }
}

#[test]
fn every_member_that_is_wrong_is_refused_and_nothing_is_changed() {
    let refused = UserSettingsPatch::from_members(&members(json!({
        "theme": "sepia", "locale": "ko", "density": "compact", "haptics": 1,
    })))
    .expect_err("three members are wrong");
    let refused: Vec<(String, RefusalReason)> = refused
        .into_iter()
        .map(|refusal| (refusal.pointer, refusal.reason))
        .collect();
    assert_eq!(
        refused,
        [
            ("/haptics".to_owned(), RefusalReason::Invalid),
            ("/locale".to_owned(), RefusalReason::Unknown),
            ("/theme".to_owned(), RefusalReason::Invalid),
        ],
        "in the order of the names; the one that is fine is not among them"
    );
}

#[test]
fn null_puts_a_setting_back_and_what_is_not_named_is_not_touched() {
    let patch = UserSettingsPatch::from_members(&members(
        json!({"theme": null, "density": "compact", "haptics": null}),
    ))
    .expect("a valid change");
    let (set, reset) = patch.changes();
    assert_eq!(set, [(SettingName::Density, "compact")]);
    assert_eq!(reset, [SettingName::Theme, SettingName::Haptics]);
    assert!(!patch.is_empty(), "it changes something");
    assert!(
        UserSettingsPatch::from_members(&JsonMap::new())
            .expect("nothing is a valid change")
            .is_empty(),
        "an empty object changes nothing"
    );
}

#[test]
fn stored_values_this_build_cannot_read_leave_the_defaults() {
    let settings = UserSettings::from_stored([
        ("theme", "sepia"),
        ("locale", "ko"),
        ("haptics", "maybe"),
        ("timeZone", "Mars/Olympus_Mons"),
        ("density", "compact"),
    ]);
    assert_eq!(
        settings,
        UserSettings {
            density: Density::Compact,
            ..UserSettings::default()
        }
    );
}

#[test]
fn names_and_values_of_client_state_are_bounded() {
    for name in ["a", "welcomePageViewed", "tour.step-1_x", &"n".repeat(64)] {
        assert_eq!(check_client_state_name(name), Ok(()), "{name}");
    }
    for name in ["", &"n".repeat(65), "with space", "키", "a/b", "a\n"] {
        assert_eq!(
            check_client_state_name(name),
            Err(ClientStateError::InvalidName),
            "{name:?}"
        );
    }
    assert_eq!(check_client_state_value(&"v".repeat(4096)), Ok(()));
    assert_eq!(
        check_client_state_value(&"v".repeat(4097)),
        Err(ClientStateError::ValueTooLong)
    );
}

#[test]
fn a_language_is_auto_or_has_the_shape_of_a_tag() {
    for tag in [
        "auto",
        "ko",
        "en-US",
        "en-us",
        "zh-Hant-TW",
        "es-419",
        "sr-Latn",
        "de-CH-1901",
        "sl-rozaj-biske",
        "en-US-u-ca-gregory-hc-h23",
        "en-x-private",
        "haw",
    ] {
        let read: Language = tag.parse().expect(tag);
        assert_eq!(read.as_str(), tag, "kept as given");
    }
    for tag in [
        "",
        "k",
        "-",
        "ko-",
        "-ko",
        "ko--KR",
        "ko_KR",
        "한국어",
        "ko KR",
        "toolongtag",
        "en-toolongsub",
        "1a",
        &format!("en-u{}", "-abcdefgh".repeat(7)),
        "Auto",
        "AUTO",
        "abcd",
        "en-a",
        "en-x",
        "en-1",
        "en-US-US",
        "de-1901-1901",
        "en-a-bc-a-de",
        "en-x-ninechars",
        "zh-yue-HK",
        "sgn-BE-FR",
        "i-klingon",
        "x-private",
    ] {
        assert!(tag.parse::<Language>().is_err(), "{tag:?} is no tag");
    }
}
