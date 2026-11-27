use std::collections::BTreeMap;

use serde_json::json;

use super::*;

fn day(text: &str) -> DiaryDate {
    text.parse().expect("valid test date")
}

#[test]
fn sixty_units_with_varied_amounts_distribute_proportionally_across_study_days() {
    // 60 units with diverse amounts: alternating light (10), medium (20, 30), heavy (60)
    // across 20 weekday study days (2026-11-02 Mon .. 2026-11-27 Fri).
    let amounts: Vec<u32> = (0..60)
        .map(|i| match i % 4 {
            0 => 10,
            1 => 20,
            2 => 30,
            _ => 60,
        })
        .collect();
    let ids: Vec<String> = (0..60).map(|i| format!("u-{i:02}")).collect();
    let weights: Vec<UnitWeight<'_>> = ids
        .iter()
        .zip(&amounts)
        .map(|(id, &amount)| UnitWeight {
            unit_id: id.as_str(),
            amount: Some(amount),
        })
        .collect();
    let start = day("2026-11-02");
    let target = day("2026-11-27");
    let weekdays = StudyDays::try_new(STUDY_DAYS_WEEKDAYS).expect("weekdays mask");
    let study_days = study_dates_in_range(start, target, weekdays);
    assert_eq!(study_days.len(), 20);

    let plan = distribute_units(&weights, start, target, weekdays);
    assert_eq!(plan.len(), 60);
    // Starts on start_date and reaches target_date.
    assert_eq!(plan.first().expect("first").1, start);
    assert_eq!(plan.last().expect("last").1, target);
    // Preserves outline order monotonically and only uses selected weekdays.
    for pair in plan.windows(2) {
        assert!(pair[0].1 <= pair[1].1, "non-monotonic: {pair:?}");
    }
    let mut load_by_day: BTreeMap<DiaryDate, u32> = BTreeMap::new();
    for ((_, date), &amount) in plan.iter().zip(&amounts) {
        assert!(weekdays.includes_date(*date), "weekend scheduled: {date}");
        *load_by_day.entry(*date).or_default() += amount;
    }
    // Total weight = 15 * (10 + 20 + 30 + 60) = 1800 -> average 90 per day across 20 days.
    assert_eq!(load_by_day.len(), 20, "every study day gets units");
    let total: u32 = load_by_day.values().sum();
    assert_eq!(total, 1800);
    let min_load = *load_by_day.values().min().expect("min");
    let max_load = *load_by_day.values().max().expect("max");
    assert!(
        min_load >= 60 && max_load <= 120,
        "balanced daily load around 90: min={min_load}, max={max_load}, {load_by_day:?}"
    );
}

#[test]
fn heavy_units_occupy_fewer_units_per_day_than_light_units() {
    // First 10 units weigh 10 each (100 total); last 2 units weigh 50 each (100 total).
    // Across 2 study days, day 0 gets the 10 light units and day 1 gets the 2 heavy units.
    let ids: Vec<String> = (0..12).map(|i| format!("u-{i:02}")).collect();
    let weights: Vec<UnitWeight<'_>> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| UnitWeight {
            unit_id: id.as_str(),
            amount: Some(if i < 10 { 10 } else { 50 }),
        })
        .collect();
    let all = StudyDays::try_new(STUDY_DAYS_ALL).expect("all days");
    let plan = distribute_units(&weights, day("2026-11-24"), day("2026-11-25"), all);
    let day0_count = plan.iter().filter(|(_, d)| *d == day("2026-11-24")).count();
    let day1_count = plan.iter().filter(|(_, d)| *d == day("2026-11-25")).count();
    assert_eq!((day0_count, day1_count), (10, 2));
}

#[test]
fn none_amount_defaults_to_weight_one_and_fewer_units_start_on_start_date() {
    let all = StudyDays::try_new(STUDY_DAYS_ALL).expect("all days");
    let single = [UnitWeight {
        unit_id: "only",
        amount: None,
    }];
    let plan = distribute_units(&single, day("2026-11-24"), day("2026-11-30"), all);
    assert_eq!(plan, vec![("only".to_owned(), day("2026-11-24"))]);

    let five: Vec<UnitWeight<'_>> = ["a", "b", "c", "d", "e"]
        .into_iter()
        .map(|unit_id| UnitWeight {
            unit_id,
            amount: None,
        })
        .collect();
    let spread = distribute_units(&five, day("2026-11-01"), day("2026-11-10"), all);
    let dates: Vec<String> = spread.into_iter().map(|(_, d)| d.to_string()).collect();
    assert_eq!(
        dates,
        [
            "2026-11-01",
            "2026-11-03",
            "2026-11-05",
            "2026-11-07",
            "2026-11-09"
        ]
    );
}

#[test]
fn today_in_time_zone_differs_between_seoul_and_los_angeles_at_utc_16_00() {
    // 2026-11-24T16:00:00Z = 2026-11-25 01:00 KST = 2026-11-24 08:00 PST.
    let utc_16_00 = jiff::Timestamp::from_str("2026-11-24T16:00:00Z")
        .expect("valid timestamp")
        .as_millisecond();
    let seoul: TimeZone = "Asia/Seoul".parse().expect("Seoul TZ");
    let la: TimeZone = "America/Los_Angeles".parse().expect("LA TZ");
    let ny: TimeZone = "America/New_York".parse().expect("NY TZ");
    let auto = TimeZone::default();

    assert_eq!(
        today_in_time_zone(utc_16_00, &seoul).to_string(),
        "2026-11-25"
    );
    assert_eq!(today_in_time_zone(utc_16_00, &la).to_string(), "2026-11-24");
    assert_eq!(today_in_time_zone(utc_16_00, &ny).to_string(), "2026-11-24");
    assert_eq!(
        today_in_time_zone(utc_16_00, &auto).to_string(),
        "2026-11-24"
    );
}

#[test]
fn outline_units_follows_depth_first_tree_order_and_skips_chapters() {
    let node = |id: &str, parent: &str, kind: UnitKind, order: &str| UnitNode {
        id: id.to_owned(),
        parent_id: parent.to_owned(),
        kind,
        order: order.to_owned(),
        ..UnitNode::default()
    };
    let nodes = vec![
        node("u-ch2", "ch2", UnitKind::Unit, "1"),
        node("u-ch1", "ch1", UnitKind::Unit, "9"),
        node("ch1", "", UnitKind::Chapter, "V"),
        node("top", "", UnitKind::Unit, "W"),
        node("ch2", "", UnitKind::Chapter, "X"),
    ];
    let ids: Vec<&str> = outline_units(&nodes)
        .iter()
        .map(|u| u.id.as_str())
        .collect();
    assert_eq!(ids, ["u-ch1", "top", "u-ch2"]);
}

#[test]
fn overdue_units_do_not_auto_shift_and_personal_planned_date_overrides_shared() {
    let nodes = vec![
        UnitNode {
            id: "u1".to_owned(),
            kind: UnitKind::Unit,
            order: "1".to_owned(),
            scheduled_date: Some("2026-11-20".to_owned()),
            ..UnitNode::default()
        },
        UnitNode {
            id: "u2".to_owned(),
            kind: UnitKind::Unit,
            order: "2".to_owned(),
            scheduled_date: Some("2026-11-20".to_owned()),
            ..UnitNode::default()
        },
        UnitNode {
            id: "u3".to_owned(),
            kind: UnitKind::Unit,
            order: "3".to_owned(),
            scheduled_date: Some("2026-11-24".to_owned()),
            ..UnitNode::default()
        },
    ];
    let progress = vec![
        UnitProgress {
            unit_id: "u1".to_owned(),
            completed_at: Some(1_000),
            planned_date: None,
            last_position: None,
            updated_at: 1_000,
            suggested_next_interval_days: None,
        },
        UnitProgress {
            unit_id: "u3".to_owned(),
            completed_at: None,
            planned_date: Some("2026-11-26".to_owned()),
            last_position: None,
            updated_at: 1_000,
            suggested_next_interval_days: None,
        },
    ];
    let view = build_course_schedule_view(None, &nodes, &progress, day("2026-11-24"));
    assert_eq!(view.today, "2026-11-24");
    assert_eq!(view.overdue_count, 1);
    assert_eq!(view.today_count, 0);
    assert!(!view.units[0].overdue && view.units[0].completed);
    assert!(view.units[1].overdue);
    assert_eq!(view.units[1].effective_date.as_deref(), Some("2026-11-20"));
    assert_eq!(view.units[2].scheduled_date.as_deref(), Some("2026-11-24"));
    assert_eq!(view.units[2].my_planned_date.as_deref(), Some("2026-11-26"));
    assert_eq!(view.units[2].effective_date.as_deref(), Some("2026-11-26"));
}

#[test]
fn validation_refuses_bad_dates_empty_study_days_and_missing_saved_schedule() {
    let bad: CourseScheduleUpdate = serde_json::from_value(json!({
        "startDate": "2026-11-28",
        "targetDate": "2026-11-29",
        "studyDays": STUDY_DAYS_WEEKDAYS
    }))
    .expect("decodes");
    let refused = bad.check_request().expect_err("weekend has no weekdays");
    assert_eq!(
        refused
            .iter()
            .map(|r| r.pointer.as_str())
            .collect::<Vec<_>>(),
        ["/studyDays"]
    );

    let reverse: CourseScheduleUpdate = serde_json::from_value(json!({
        "startDate": "2026-11-30",
        "targetDate": "2026-11-01",
        "studyDays": 0
    }))
    .expect("decodes");
    let refused = reverse.check_request().expect_err("refused");
    let pointers: Vec<&str> = refused.iter().map(|r| r.pointer.as_str()).collect();
    assert_eq!(pointers, ["/studyDays", "/targetDate"]);

    let empty_req = RedistributeScheduleRequest::default();
    let missing = empty_req
        .resolve(day("2026-11-24"), None)
        .expect_err("needs saved schedule");
    let pointers: Vec<&str> = missing.iter().map(|r| r.pointer.as_str()).collect();
    assert_eq!(pointers, ["/targetDate", "/studyDays"]);
}

#[test]
fn unit_schedule_update_requires_planned_date_member() {
    assert!(
        serde_json::from_value::<UnitScheduleUpdate>(json!({})).is_err(),
        "empty object must not deserialize as clearing plannedDate"
    );
    let cleared: UnitScheduleUpdate =
        serde_json::from_value(json!({"plannedDate": null})).expect("explicit null decodes");
    assert_eq!(cleared.check_request().expect("valid null"), None);

    let set: UnitScheduleUpdate =
        serde_json::from_value(json!({"plannedDate": "2026-11-25"})).expect("date decodes");
    assert_eq!(
        set.check_request().expect("valid date"),
        Some(day("2026-11-25"))
    );
}

#[test]
fn distribute_units_edge_cases_and_span_limits() {
    let weekdays = StudyDays::try_new(STUDY_DAYS_WEEKDAYS).expect("weekdays");
    let all = StudyDays::try_new(STUDY_DAYS_ALL).expect("all");

    // Empty unit slice, reverse range, and range with zero matching study days return empty.
    assert!(
        distribute_units(&[], day("2026-11-02"), day("2026-11-10"), all).is_empty(),
        "0 units produces empty plan"
    );
    let one = [UnitWeight {
        unit_id: "u1",
        amount: Some(0),
    }];
    assert!(
        distribute_units(&one, day("2026-11-10"), day("2026-11-02"), all).is_empty(),
        "start > target produces empty plan"
    );
    assert!(
        distribute_units(&one, day("2026-11-28"), day("2026-11-29"), weekdays).is_empty(),
        "0 matching study days produces empty plan"
    );
    // amount = Some(0) clamps to weight 1 cleanly.
    assert_eq!(
        distribute_units(&one, day("2026-11-02"), day("2026-11-02"), weekdays),
        vec![("u1".to_owned(), day("2026-11-02"))]
    );

    // Span boundary: exactly MAX_SCHEDULE_SPAN_DAYS (3660) passes; 3661 is refused.
    let start = day("2026-01-01");
    let max_span = i32::try_from(MAX_SCHEDULE_SPAN_DAYS).expect("span fits i32");
    let ok_target = start.plus_days(max_span).expect("within span");
    let too_far = start.plus_days(max_span + 1).expect("past span");
    let ok_req: CourseScheduleUpdate = serde_json::from_value(json!({
        "startDate": start.to_string(),
        "targetDate": ok_target.to_string(),
        "studyDays": STUDY_DAYS_ALL
    }))
    .expect("decodes");
    assert!(ok_req.check_request().is_ok(), "3660-day span is valid");
    let bad_req: CourseScheduleUpdate = serde_json::from_value(json!({
        "startDate": start.to_string(),
        "targetDate": too_far.to_string(),
        "studyDays": STUDY_DAYS_ALL
    }))
    .expect("decodes");
    let err = bad_req.check_request().expect_err("3661-day span refused");
    assert_eq!(
        err.iter().map(|r| r.pointer.as_str()).collect::<Vec<_>>(),
        ["/targetDate"]
    );

    // Maximum scale: 2,000 units * 100,000 weight does not overflow and stays monotonic.
    let ids: Vec<String> = (0..2_000).map(|i| format!("u-{i:04}")).collect();
    let max_weights: Vec<UnitWeight<'_>> = ids
        .iter()
        .map(|id| UnitWeight {
            unit_id: id.as_str(),
            amount: Some(UNIT_AMOUNT_MAX),
        })
        .collect();
    let large = distribute_units(&max_weights, start, ok_target, all);
    assert_eq!(large.len(), 2_000);
    assert_eq!(large.first().expect("first").1, start);
    assert!(
        large.windows(2).all(|w| w[0].1 <= w[1].1),
        "2,000 max-weight units remain monotonic"
    );
}
