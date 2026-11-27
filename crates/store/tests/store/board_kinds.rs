//! The kind of a board: what a board is made as it stays, what a wiki is not, and the
//! lists of boards, each of the kinds its caller asks for.

use fvoci_model::PublicContainers;
use fvoci_model::{
    Block, BlockType, Container, ContainerError, ContainerKind, ContainerKinds, ContainerPatch,
    ContainerSearchField, ContainerVisibility, MinimumRole, WorkspaceRole, new_id,
};
use fvoci_store::{Store, StoreError};

use crate::boards::new_board;
use crate::common;

/// Stores a private board of `kind` with `user_id` as its administrator.
async fn stored(
    store: &Store,
    workspace_id: &str,
    user_id: &str,
    kind: ContainerKind,
) -> Container {
    let mut board = Container {
        kind,
        title: format!("a {} to find", kind.as_str()),
        ..new_board(&new_id(), workspace_id, ContainerVisibility::Private)
    };
    store
        .insert_container_with_admin(&mut board, user_id)
        .await
        .expect("the board is stored");
    board
}

fn ids(boards: &[Container]) -> Vec<&str> {
    boards.iter().map(|board| board.id.as_str()).collect()
}

fn refusal(result: Result<impl std::fmt::Debug, StoreError>) -> ContainerError {
    match result {
        Err(StoreError::InvalidContainer(refused)) => refused,
        other => panic!("the board is refused: {other:?}"),
    }
}

#[tokio::test]
async fn a_board_is_of_the_kind_it_was_made_as_whatever_stores_it_again() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let board = stored(&test, &workspace_id, &user_id, ContainerKind::Board).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let course = stored(&test, &workspace_id, &user_id, ContainerKind::Course).await;
    assert_eq!(
        Container::default().kind,
        ContainerKind::Board,
        "a board that says no kind is a board"
    );

    for (made, other) in [
        (&board, ContainerKind::Wiki),
        (&wiki, ContainerKind::Board),
        (&course, ContainerKind::Wiki),
    ] {
        let mut as_other = Container {
            kind: other,
            title: "of another kind".to_owned(),
            ..made.clone()
        };
        assert_eq!(
            refusal(test.insert_container(&mut as_other, &user_id).await),
            ContainerError::KindChanges {
                stored: made.kind,
                asked: other,
            }
        );
        // A change that leaves the kind alone is stored, and the kind with it.
        let patch = ContainerPatch {
            description: Some("described".to_owned()),
            ..ContainerPatch::default()
        };
        let patched = test
            .patch_container(&made.id, &patch, &user_id)
            .await
            .expect("the board is patched");
        assert_eq!(patched.kind, made.kind);
        let read = test
            .get_container(&made.id)
            .await
            .expect("the board is read");
        assert_eq!(
            (read.kind, read.title.as_str(), read.description.as_str()),
            (made.kind, made.title.as_str(), "described")
        );
    }
    test.teardown().await;
}

#[tokio::test]
async fn a_wiki_is_no_template_and_has_no_card_properties() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let property = fvoci_model::CardPropertyDefinition::new(
        "status",
        "Status",
        fvoci_model::PropertyType::Text,
    );
    let wiki = || Container {
        kind: ContainerKind::Wiki,
        ..new_board(&new_id(), &workspace_id, ContainerVisibility::Open)
    };

    let mut template = Container {
        is_template: true,
        ..wiki()
    };
    assert_eq!(
        refusal(test.insert_container(&mut template, &user_id).await),
        ContainerError::WikiIsTemplate
    );
    let mut with_properties = Container {
        card_properties: vec![property.clone()],
        ..wiki()
    };
    assert_eq!(
        refusal(test.insert_container(&mut with_properties, &user_id).await),
        ContainerError::WikiHasCardProperties
    );
    for refused in [&template, &with_properties] {
        let found = test.get_container(&refused.id).await;
        assert!(
            matches!(&found, Err(error) if error.is_not_found()),
            "nothing is stored of what is refused: {found:?}"
        );
    }

    // Nor does a wiki that is stored get them afterwards.
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let patch = ContainerPatch {
        updated_card_properties: vec![property],
        ..ContainerPatch::default()
    };
    assert_eq!(
        refusal(test.patch_container(&wiki.id, &patch, &user_id).await),
        ContainerError::WikiHasCardProperties
    );
    let read = test
        .get_container(&wiki.id)
        .await
        .expect("the wiki is read");
    assert!(
        read.card_properties.is_empty(),
        "the wiki has none: {:?}",
        read.card_properties
    );

    // Courses may be templates, but card properties are still refused.
    let course_prop =
        fvoci_model::CardPropertyDefinition::new("level", "Level", fvoci_model::PropertyType::Text);
    let course = || Container {
        kind: ContainerKind::Course,
        ..new_board(&new_id(), &workspace_id, ContainerVisibility::Open)
    };
    let mut course_template = Container {
        is_template: true,
        ..course()
    };
    test.insert_container(&mut course_template, &user_id)
        .await
        .expect("a course template is stored");
    let saved_course_template = test
        .get_container(&course_template.id)
        .await
        .expect("the course template is read");
    assert!(saved_course_template.is_template);
    assert_eq!(saved_course_template.kind, ContainerKind::Course);
    let mut course_with_props = Container {
        card_properties: vec![course_prop.clone()],
        ..course()
    };
    assert_eq!(
        refusal(
            test.insert_container(&mut course_with_props, &user_id)
                .await
        ),
        ContainerError::CourseHasCardProperties
    );
    let stored_course = stored(&test, &workspace_id, &user_id, ContainerKind::Course).await;
    let patch = ContainerPatch {
        updated_card_properties: vec![course_prop],
        ..ContainerPatch::default()
    };
    assert_eq!(
        refusal(
            test.patch_container(&stored_course.id, &patch, &user_id)
                .await
        ),
        ContainerError::CourseHasCardProperties
    );
    test.teardown().await;
}

/// Closed to the session, the workspace gives it no board: neither one the user is
/// on, nor one a team of theirs is on. `admin` has the organization take its
/// provider's sessions only, and each of `users` asks with a session made with the
/// password.
async fn nothing_is_listed_once_closed(store: &Store, admin: &str, users: [&String; 2]) {
    store
        .put_oidc_provider_as_read(
            &crate::oidc_audit::settings(),
            None,
            admin,
            std::time::Duration::ZERO,
        )
        .await
        .expect("the organization takes its provider's sessions only");
    for who in users {
        let listed = store
            .container_memberships_of_user(
                who,
                fvoci_store::Making::with_password(),
                ContainerKinds::EVERY,
            )
            .await
            .expect("the memberships of a session the workspace is closed to");
        assert!(
            listed.is_empty(),
            "nothing of a closed workspace: {listed:?}"
        );
    }
}

#[tokio::test]
async fn every_list_of_boards_is_of_the_kinds_it_is_asked_for() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let board = stored(&test, &workspace_id, &user_id, ContainerKind::Board).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let course = stored(&test, &workspace_id, &user_id, ContainerKind::Course).await;
    // Somebody who is on all three through a team only.
    let (admin, through_team) = (
        common::user(&test, "admin").await,
        common::user(&test, "teammate").await,
    );
    for (who, role) in [
        (&admin, WorkspaceRole::Admin),
        (&through_team, WorkspaceRole::Member),
    ] {
        test.add_workspace_member(&workspace_id, who, role)
            .await
            .expect("the user is let into the workspace");
    }
    let team = test
        .create_team(&workspace_id, "a team", &admin)
        .await
        .expect("a team is stored");
    test.add_team_member(&workspace_id, &team, &through_team, &admin)
        .await
        .expect("a team member is stored");
    for on in [&board, &wiki, &course] {
        let role = Some(MinimumRole::Editor);
        test.set_container_team(&workspace_id, &on.id, &team, role, &admin)
            .await
            .expect("the team is on the board");
    }
    let cases = [
        (ContainerKinds::BOARDS, vec![board.id.as_str()]),
        (ContainerKinds::WIKIS, vec![wiki.id.as_str()]),
        (ContainerKinds::COURSES, vec![course.id.as_str()]),
        (
            ContainerKinds::LINKED,
            vec![board.id.as_str(), wiki.id.as_str(), course.id.as_str()],
        ),
        (
            ContainerKinds::OPEN_TO_READ,
            vec![board.id.as_str(), wiki.id.as_str(), course.id.as_str()],
        ),
        (
            ContainerKinds::SHARED,
            vec![board.id.as_str(), wiki.id.as_str(), course.id.as_str()],
        ),
        (
            ContainerKinds::EVERY,
            vec![board.id.as_str(), wiki.id.as_str(), course.id.as_str()],
        ),
    ];

    for (kinds, expected) in &cases {
        assert_listed_kinds(
            &test,
            &workspace_id,
            &user_id,
            &through_team,
            *kinds,
            expected,
        )
        .await;
    }

    nothing_is_listed_once_closed(&test, &admin, [&user_id, &through_team]).await;
    test.teardown().await;
}

async fn assert_listed_kinds(
    test: &Store,
    workspace_id: &str,
    user_id: &str,
    through_team: &str,
    kinds: ContainerKinds,
    expected: &[&str],
) {
    let of_user = test
        .get_containers_for_user_and_workspace(
            user_id,
            workspace_id,
            PublicContainers::Include,
            kinds,
        )
        .await
        .expect("the boards of the user");
    let of_workspace = test
        .get_containers_of_workspace(workspace_id, kinds)
        .await
        .expect("the boards of the workspace");
    let found = test
        .search_containers_for_user(
            "find",
            ContainerSearchField::Title,
            user_id,
            PublicContainers::Include,
            kinds,
        )
        .await
        .expect("the boards found");
    let found_there = test
        .search_containers_for_user_in_workspace(workspace_id, "find", user_id, kinds)
        .await
        .expect("the boards found in the workspace");
    let memberships = test
        .container_memberships_of_user(user_id, fvoci_store::Making::with_password(), kinds)
        .await
        .expect("the memberships");
    let member_of: Vec<&str> = memberships
        .iter()
        .map(|member| member.container_id.as_str())
        .collect();
    let memberships = test
        .container_memberships_of_user(through_team, fvoci_store::Making::with_password(), kinds)
        .await
        .expect("the memberships through the team");
    let through: Vec<&str> = memberships
        .iter()
        .map(|member| member.container_id.as_str())
        .collect();
    for (list, listed) in [
        ("of the user", ids(&of_user)),
        ("of the workspace", ids(&of_workspace)),
        ("found", ids(&found)),
        ("found in the workspace", ids(&found_there)),
        ("the user is on", member_of),
        ("a team is on", through),
    ] {
        assert_eq!(&listed, expected, "the boards {list}, of {kinds:?}");
    }
}

#[tokio::test]
async fn a_block_is_found_within_the_kinds_of_its_board_only() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let elsewhere = common::workspace(&test).await;
    let board = stored(&test, &workspace_id, &user_id, ContainerKind::Board).await;
    let mut cards = [Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        ..Block::default()
    }];
    common::insert_blocks(&test, &mut cards, &user_id).await;
    let card = &cards[0].id;

    for (workspace, kinds, found) in [
        (None, ContainerKinds::EVERY, true),
        (None, ContainerKinds::BOARDS, true),
        (None, ContainerKinds::WIKIS, false),
        (Some(workspace_id.as_str()), ContainerKinds::BOARDS, true),
        (Some(workspace_id.as_str()), ContainerKinds::WIKIS, false),
        (Some(elsewhere.as_str()), ContainerKinds::EVERY, false),
    ] {
        let read = test.get_block_within(card, workspace, kinds).await;
        match read {
            Ok(block) => assert!(found && block.id == *card, "{workspace:?}, {kinds:?}"),
            Err(error) => assert!(
                !found && error.is_not_found(),
                "{workspace:?}, {kinds:?}: {error:?}"
            ),
        }
    }
    test.teardown().await;
}

#[tokio::test]
async fn the_trash_is_listed_by_kind_and_each_comes_back_as_what_it_was() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;
    let board = stored(&test, &workspace_id, &user_id, ContainerKind::Board).await;
    let wiki = stored(&test, &workspace_id, &user_id, ContainerKind::Wiki).await;
    let course = stored(&test, &workspace_id, &user_id, ContainerKind::Course).await;
    for deleted in [&board, &wiki, &course] {
        test.delete_container(&deleted.id, &user_id)
            .await
            .expect("the board goes to the trash");
    }
    for (kinds, expected) in [
        (ContainerKinds::BOARDS, vec![board.id.as_str()]),
        (ContainerKinds::WIKIS, vec![wiki.id.as_str()]),
        (ContainerKinds::COURSES, vec![course.id.as_str()]),
        (
            ContainerKinds::LINKED,
            vec![board.id.as_str(), wiki.id.as_str(), course.id.as_str()],
        ),
    ] {
        for admin in [None, Some(user_id.as_str())] {
            let listed = test
                .get_deleted_containers(&workspace_id, admin, None, kinds)
                .await
                .expect("the trash is listed");
            let mut listed: Vec<&str> = listed
                .iter()
                .map(|deleted| deleted.container.id.as_str())
                .collect();
            let mut expected = expected.clone();
            listed.sort_unstable();
            expected.sort_unstable();
            assert_eq!(listed, expected, "{kinds:?}, for {admin:?}");
        }
    }
    let every = test
        .get_deleted_containers(&workspace_id, None, None, ContainerKinds::EVERY)
        .await
        .expect("the trash is listed");
    assert_eq!(every.len(), 3);

    // Each comes back as what it was.
    for (deleted, kind) in [
        (&board, ContainerKind::Board),
        (&wiki, ContainerKind::Wiki),
        (&course, ContainerKind::Course),
    ] {
        let restored = test
            .undelete_container(&deleted.id, &user_id)
            .await
            .expect("the board comes back");
        assert_eq!(restored.kind, kind);
        let read = test.get_container(&deleted.id).await.expect("it is read");
        assert_eq!(read.kind, kind);
    }
    test.teardown().await;
}

/// Demoting a workspace member to `Guest` removes personal learning rows (`unit_progress`,
/// `exam_scores`, `unit_reviews`, `unit_notes`) on team-shared courses they lose
/// membership on while preserving rows on courses where they hold a direct role.
#[tokio::test]
async fn demoting_workspace_member_to_guest_cleans_unjoined_personal_learning_rows() {
    use fvoci_model::{ContainerVisibility, WorkspaceRole};

    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let learner = common::user(&test, "learner").await;
    common::works_in(&test, &test.workspace_id, &admin).await;
    common::works_in(&test, &test.workspace_id, &learner).await;
    test.execute_unchecked(&format!(
        "UPDATE workspace_members SET role = 'admin' WHERE workspace_id = '{}' AND user_id = '{admin}'",
        test.workspace_id
    ))
    .await
    .expect("admin role");

    let (team_course, team_unit) = seed_course_learning_rows(
        &test,
        &test.workspace_id,
        ContainerVisibility::Open,
        "Team Course",
        &admin,
        &learner,
    )
    .await;
    let (direct_course, direct_unit) = seed_course_learning_rows(
        &test,
        &test.workspace_id,
        ContainerVisibility::Private,
        "Direct Course",
        &admin,
        &learner,
    )
    .await;

    test.change_workspace_role(
        &test.workspace_id,
        &learner,
        (WorkspaceRole::Member, WorkspaceRole::Guest),
        &admin,
    )
    .await
    .expect("demoted to guest");

    assert_course_learning_counts(&test, &team_course, &team_unit, &learner, &admin, 0).await;
    assert_course_learning_counts(&test, &direct_course, &direct_unit, &learner, &learner, 1).await;

    test.teardown().await;
}

async fn seed_course_learning_rows(
    test: &fvoci_store::Store,
    workspace_id: &str,
    ctype: fvoci_model::ContainerVisibility,
    title: &str,
    admin: &str,
    learner: &str,
) -> (String, String) {
    use fvoci_model::{
        Container, ContainerKind, ContainerMember, ContainerRole, ContainerVisibility,
        CourseLimits, MinimumRole, NewCourseExam, NewUnit, UnitKind, new_id,
    };

    let mut c = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ctype,
        kind: ContainerKind::Course,
        title: title.to_owned(),
        ..Container::default()
    };
    test.insert_container_with_admin(&mut c, admin)
        .await
        .expect("course");
    if ctype == ContainerVisibility::Private {
        test.save_member(&ContainerMember::new(
            c.id.clone(),
            learner.to_owned(),
            ContainerRole::Viewer,
        ))
        .await
        .expect("direct course viewer");
    } else {
        let team_id = test
            .create_team(workspace_id, "Course Learners", admin)
            .await
            .expect("team created");
        test.add_team_member(workspace_id, &team_id, learner, admin)
            .await
            .expect("learner in team");
        test.set_container_team(
            workspace_id,
            &c.id,
            &team_id,
            Some(MinimumRole::Viewer),
            admin,
        )
        .await
        .expect("team assigned to course");
    }
    let u = test
        .create_unit(
            &c.id,
            &NewUnit {
                kind: UnitKind::Unit,
                title: "Unit 1".to_owned(),
                ..NewUnit::default()
            },
            CourseLimits::DEFAULT,
            admin,
        )
        .await
        .expect("unit");
    let e = test
        .create_course_exam(
            &c.id,
            &NewCourseExam {
                title: "Quiz".to_owned(),
                exam_date: None,
                max_score: 100.0,
                description: String::new(),
            }
            .check_request()
            .expect("exam"),
        )
        .await
        .expect("exam created");
    populate_learner_activity(test, &c.id, &u.unit.id, &e.id, learner).await;
    (c.id, u.unit.id)
}

async fn populate_learner_activity(
    test: &fvoci_store::Store,
    course_id: &str,
    unit_id: &str,
    exam_id: &str,
    learner: &str,
) {
    use fvoci_model::{
        CompletionChange, LastPositionChange, NewUnitNote, NewUnitReview, UnitNoteKind,
        UnitProgressChange, ValidatedMyExamScore,
    };

    test.upsert_unit_progress(
        course_id,
        unit_id,
        learner,
        &UnitProgressChange {
            completion: CompletionChange::Complete,
            last_position: LastPositionChange::Keep,
        },
    )
    .await
    .expect("progress");
    test.upsert_my_exam_score(
        course_id,
        exam_id,
        learner,
        &ValidatedMyExamScore {
            score: 92.0,
            note: "personal".to_owned(),
        },
    )
    .await
    .expect("score");
    test.schedule_unit_review(
        course_id,
        unit_id,
        learner,
        &NewUnitReview {
            scheduled_date: "2026-11-25".to_owned(),
            interval_days: 1,
        },
        None,
    )
    .await
    .expect("review");
    test.create_unit_note(
        course_id,
        unit_id,
        learner,
        &NewUnitNote {
            kind: UnitNoteKind::Review,
            content: "memo".to_owned(),
        },
    )
    .await
    .expect("note");
}

async fn assert_course_learning_counts(
    test: &fvoci_store::Store,
    course_id: &str,
    unit_id: &str,
    learner: &str,
    exam_viewer: &str,
    expected: usize,
) {
    use fvoci_model::ReviewStatusFilter;

    assert_eq!(
        test.get_my_course_progress(course_id, learner)
            .await
            .expect("progress")
            .len(),
        expected,
        "unexpected unit_progress count"
    );
    assert_eq!(
        usize::try_from(
            test.list_course_exams(course_id, fvoci_store::ExamViewer::Manager(exam_viewer))
                .await
                .expect("exams")[0]
                .score_count
        )
        .expect("count"),
        expected,
        "unexpected exam_scores count"
    );
    assert_eq!(
        test.list_my_course_reviews(course_id, learner, ReviewStatusFilter::All, None)
            .await
            .expect("reviews")
            .len(),
        expected,
        "unexpected unit_reviews count"
    );
    assert_eq!(
        test.list_my_unit_notes(course_id, unit_id, learner, None)
            .await
            .expect("notes")
            .len(),
        expected,
        "unexpected unit_notes count"
    );
}

#[tokio::test]
async fn team_removal_forgets_trashed_card_watchers() {
    let test = common::setup().await;
    let alice = common::user(&test, "alice-team-trash").await;
    let bob = common::user(&test, "bob-team-trash").await;
    let workspace_id = common::workspace_of(&test, &[&alice, &bob]).await;
    test.execute_unchecked(&format!(
        "UPDATE workspace_members SET role = 'admin' WHERE workspace_id = '{workspace_id}' AND user_id = '{alice}'"
    ))
    .await
    .expect("alice admin role");

    let mut private_board = fvoci_model::Container {
        id: new_id(),
        workspace_id: workspace_id.clone(),
        visibility: fvoci_model::ContainerVisibility::Private,
        title: "Team Trash Board".to_owned(),
        ..fvoci_model::Container::default()
    };
    test.insert_container_with_admin(&mut private_board, &alice)
        .await
        .expect("private board");

    let team = test
        .create_team(&workspace_id, "Reviewers", &alice)
        .await
        .expect("team");
    test.add_team_member(&workspace_id, &team, &bob, &alice)
        .await
        .expect("bob in team");
    test.set_container_team(
        &workspace_id,
        &private_board.id,
        &team,
        Some(fvoci_model::MinimumRole::Editor),
        &alice,
    )
    .await
    .expect("team on board");

    let card_id = new_id();
    let mut cards = [Block {
        id: card_id.clone(),
        container_id: private_board.id.clone(),
        block_type: BlockType::Card,
        ..Block::default()
    }];
    common::insert_blocks(&test, &mut cards, &alice).await;
    test.create_subscription(fvoci_model::Subscription {
        block_type: BlockType::Card,
        block_id: card_id.clone(),
        user_id: bob.clone(),
        notified_at: 0,
        created_at: 0,
        deleted_at: None,
    })
    .await
    .expect("bob watches card");

    test.delete_block(&card_id, &alice)
        .await
        .expect("card trashed");
    test.set_container_team(&workspace_id, &private_board.id, &team, None, &alice)
        .await
        .expect("team removed from board");
    test.undelete_block(&card_id, &alice)
        .await
        .expect("card restored");
    assert!(
        test.get_subscriptions(&bob)
            .await
            .expect("bob subscriptions")
            .is_empty(),
        "former team member must not regain subscription on card restore"
    );
    test.teardown().await;
}
