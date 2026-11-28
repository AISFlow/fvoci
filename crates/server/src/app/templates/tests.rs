use std::collections::{BTreeMap, BTreeSet};

use super::*;

/// A built-in template as the import stores it, in English, without the mark of
/// the welcome board.
fn unmarked() -> Container {
    let mut board = Container {
        id: "b-built-in".to_owned(),
        workspace_id: "t-root".to_owned(),
        is_template: true,
        is_builtin: true,
        template_version: DEFAULT_TEMPLATE_VERSION,
        visibility: ContainerVisibility::Open,
        ..Container::default()
    };
    board.properties.insert(
        TEMPLATE_LANGUAGE_PROPERTY.to_owned(),
        Value::String("en".to_owned()),
    );
    board
}

/// The built-in welcome board of a language.
fn marked(language: Language) -> Container {
    let mut board = unmarked();
    board
        .properties
        .insert(ONBOARDING_PROPERTY.to_owned(), Value::Bool(true));
    board.properties.insert(
        TEMPLATE_LANGUAGE_PROPERTY.to_owned(),
        Value::String(language.tag().to_owned()),
    );
    board
}

/// The template files as JSON, by key and then by the tag of their language.
fn files_by_key() -> BTreeMap<String, BTreeMap<String, Value>> {
    let mut files: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    for (name, text) in TEMPLATE_FILES {
        let file: Value = serde_json::from_str(text).expect(name);
        let key = file["key"].as_str().expect(name).to_owned();
        let language = file["language"].as_str().expect(name).to_owned();
        assert!(
            files
                .entry(key)
                .or_default()
                .insert(language, file)
                .is_none(),
            "{name} is written twice in one language"
        );
    }
    files
}

/// What a course template has that is the same in every language: its schedule,
/// chapters without titles, and units without titles or body text.
fn course_skeleton(file: &Value) -> Value {
    let chapters: Vec<Value> = file["chapters"]
        .as_array()
        .expect("the chapters")
        .iter()
        .map(|chapter| serde_json::json!([chapter["id"], chapter["fields"]]))
        .collect();
    let units: Vec<Value> = file["units"]
        .as_array()
        .expect("the units")
        .iter()
        .map(|unit| {
            let mut nodes = Vec::new();
            outline(&unit["body"], &mut nodes);
            serde_json::json!([unit["id"], unit["parentId"], unit["fields"], nodes])
        })
        .collect();
    serde_json::json!({
        "onboarding": file["onboarding"],
        "board": [file["board"]["id"], file["board"]["kind"], file["board"]["icon"]],
        "schedule": file["schedule"],
        "chapters": chapters,
        "units": units,
        "files": file["files"].as_array().map_or(0, Vec::len),
    })
}

/// What a template has that is the same in every language: its property
/// definitions without their names, its views without their titles, and its cards
/// without their titles, their texts and the names of their files.
fn skeleton(file: &Value) -> Value {
    if file["board"]["kind"].as_str() == Some("course") {
        return course_skeleton(file);
    }
    let text_kinds = ["text", "email", "phone", "url"];
    let properties = file["board"]["cardProperties"]
        .as_array()
        .expect("the properties");
    let written: BTreeSet<&str> = properties
        .iter()
        .filter(|property| text_kinds.contains(&property["type"].as_str().expect("a type")))
        .map(|property| property["id"].as_str().expect("an identifier"))
        .collect();
    let definitions: Vec<Value> = properties
        .iter()
        .map(|property| {
            let options: Vec<Value> = property["options"]
                .as_array()
                .expect("the options")
                .iter()
                .map(|option| serde_json::json!([option["id"], option["color"]]))
                .collect();
            serde_json::json!([property["id"], property["type"], options])
        })
        .collect();
    let views: Vec<Value> = file["views"]
        .as_array()
        .expect("the views")
        .iter()
        .map(|view| serde_json::json!([view["id"], view["fields"]]))
        .collect();
    let cards: Vec<Value> = file["cards"]
        .as_array()
        .expect("the cards")
        .iter()
        .map(|card| {
            let values: BTreeMap<&str, &Value> = card["fields"]["properties"]
                .as_object()
                .expect("the values")
                .iter()
                .filter(|(id, _)| !written.contains(id.as_str()))
                .map(|(id, value)| (id.as_str(), value))
                .collect();
            let mut nodes = Vec::new();
            outline(&card["body"], &mut nodes);
            serde_json::json!([
                card["id"],
                card["fields"]["icon"],
                card["fields"]["isTemplate"],
                values,
                nodes
            ])
        })
        .collect();
    serde_json::json!({
        "onboarding": file["onboarding"],
        "board": [
            file["board"]["id"],
            file["board"]["icon"],
            file["board"]["dueDatePropertyId"],
            file["board"]["doneWhen"],
            definitions
        ],
        "views": views,
        "cards": cards,
        "files": file["files"].as_array().expect("the files").len(),
    })
}

/// The nodes of a body in the order they are written in, each with what it has
/// that no language changes: its level, whether it is checked, what it names.
fn outline(node: &Value, nodes: &mut Vec<Value>) {
    let Some(kind) = node["type"].as_str() else {
        return;
    };
    if kind != "text" {
        let attrs = &node["attrs"];
        nodes.push(serde_json::json!([
            kind,
            attrs["level"],
            attrs["checked"],
            attrs["id"],
            attrs["latex"]
        ]));
    }
    for child in node["content"].as_array().into_iter().flatten() {
        outline(child, nodes);
    }
}

/// The type of every node of the bodies of the template files.
fn node_types() -> BTreeSet<String> {
    fn gather(node: &Value, types: &mut BTreeSet<String>) {
        if let Some(kind) = node["type"].as_str() {
            types.insert(kind.to_owned());
        }
        for child in node["content"].as_array().into_iter().flatten() {
            gather(child, types);
        }
    }
    let mut types = BTreeSet::new();
    for (name, text) in TEMPLATE_FILES {
        let file: Value = serde_json::from_str(text).expect(name);
        for block in file["cards"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(file["units"].as_array().into_iter().flatten())
        {
            gather(&block["body"], &mut types);
        }
    }
    types
}

#[test]
fn every_template_file_is_read_with_a_welcome_board_in_each_language() {
    let templates = builtin_templates("t-root").expect("the embedded template files are read");
    let boards = &templates.boards_and_blocks.containers;
    assert_eq!(boards.len(), TEMPLATE_FILES.len(), "a board per file");
    for language in Language::ALL {
        let marked = boards
            .iter()
            .filter(|board| {
                written_in(board) == Some(language)
                    && board.properties.get(ONBOARDING_PROPERTY) == Some(&Value::Bool(true))
            })
            .count();
        assert_eq!(marked, 1, "one welcome board in {}", language.tag());
        let courses = boards
            .iter()
            .filter(|board| {
                written_in(board) == Some(language) && board.kind == ContainerKind::Course
            })
            .collect::<Vec<_>>();
        assert_eq!(
            courses.len(),
            1,
            "one built-in course template in {}",
            language.tag()
        );
        assert!(
            courses[0]
                .properties
                .contains_key(TEMPLATE_SCHEDULE_PROPERTY),
            "course template carries example schedule in {}",
            language.tag()
        );
    }
    assert_eq!(
        initialization_reason(
            &boards
                .iter()
                .cloned()
                .map(|board| Container {
                    is_builtin: true,
                    ..board
                })
                .collect::<Vec<_>>()
        ),
        None,
        "once stored they are current"
    );
}

#[test]
fn every_template_is_written_in_every_language_and_differs_in_its_words_only() {
    for (key, written) in files_by_key() {
        let languages: Vec<&str> = written.keys().map(String::as_str).collect();
        let mut expected: Vec<&str> = Language::ALL.iter().map(|each| each.tag()).collect();
        expected.sort_unstable();
        assert_eq!(languages, expected, "the languages of {key}");
        let mut skeletons = written.values().map(skeleton);
        let first = skeletons.next().expect("a language");
        for other in skeletons {
            assert_eq!(other, first, "{key} is the same template in each language");
        }
    }
}

#[test]
fn the_bodies_point_at_the_places_of_their_files() {
    let templates = builtin_templates("t-root").expect("the embedded template files are read");
    assert!(!templates.files.is_empty(), "the templates show files");
    let mut shown: BTreeSet<(String, String)> = BTreeSet::new();
    for block in &templates.boards_and_blocks.blocks {
        for file in block.shown_files() {
            shown.insert((block.container_id.clone(), file));
        }
    }
    let stored: BTreeSet<(String, String)> = templates
        .files
        .iter()
        .map(|file| {
            (
                file.record.container_id.clone(),
                file.record.stored_name.clone(),
            )
        })
        .collect();
    assert_eq!(
        shown, stored,
        "each file shown is one of its board, and stored"
    );
    for file in &templates.files {
        assert_eq!(
            usize::try_from(file.record.size).ok(),
            Some(file.bytes.len()),
            "{} is recorded at its size",
            file.record.name
        );
        assert!(
            fvoci_model::is_storage_path(&file.record.path),
            "{} is a place of the storage",
            file.record.path
        );
    }
    let used: BTreeSet<&[u8]> = templates.files.iter().map(|file| file.bytes).collect();
    for (name, bytes) in TEMPLATE_ASSETS {
        assert!(used.contains(bytes), "{name} is embedded and shown nowhere");
    }
}

/// The node types a body may hold that no template shows, each with why. Anything
/// else the allowlist takes has to be in a template: a node that is added to the
/// allowlist fails this test until a template shows it or it is listed here.
const NODES_NOT_SHOWN: [(&str, &str); 2] = [
    (
        "mention",
        "names a user, and a template is copied to people it knows nothing of",
    ),
    (
        "boardView",
        "shows a view by its identifier, and a template is written before any view is made",
    ),
];

#[test]
fn the_templates_show_every_node_a_body_may_hold() {
    let shown = node_types();
    for node in fvoci_model::body_node_types() {
        let left_out = NODES_NOT_SHOWN.iter().any(|(name, _)| *name == node);
        assert_ne!(
            shown.contains(node),
            left_out,
            "{node} is shown by a template, or listed as not shown, and not both"
        );
    }
}

#[test]
fn only_a_marked_template_of_the_system_is_the_onboarding_template() {
    assert!(
        is_onboarding_template(&marked(Language::English)),
        "the built-in one"
    );
    assert!(!is_onboarding_template(&unmarked()), "without the mark");

    let by_a_user = Container {
        created_by: "u-somebody".to_owned(),
        is_builtin: false,
        ..marked(Language::English)
    };
    assert!(
        !is_onboarding_template(&by_a_user),
        "a user's board with the mark and the title"
    );
    let no_template = Container {
        is_template: false,
        ..marked(Language::English)
    };
    assert!(!is_onboarding_template(&no_template), "not a template");
    let mut marked_otherwise = unmarked();
    marked_otherwise.properties.insert(
        ONBOARDING_PROPERTY.to_owned(),
        Value::String("true".to_owned()),
    );
    assert!(
        !is_onboarding_template(&marked_otherwise),
        "the mark is the boolean true"
    );
}

#[test]
fn templates_are_imported_when_missing_old_or_without_a_welcome_board() {
    assert_eq!(
        initialization_reason(&[]),
        Some("no default templates found"),
        "none"
    );
    let old = Container {
        template_version: DEFAULT_TEMPLATE_VERSION - 1,
        ..marked(Language::English)
    };
    assert_eq!(
        initialization_reason(&[old]),
        Some("template_version too old"),
        "an old one"
    );
    assert_eq!(
        initialization_reason(&[unmarked()]),
        Some("a language has no welcome board"),
        "current ones without the mark"
    );
    assert_eq!(
        initialization_reason(&[unmarked(), marked(Language::English)]),
        Some("a language has no welcome board"),
        "a welcome board in one language only"
    );
    assert_eq!(
        initialization_reason(&[
            unmarked(),
            marked(Language::English),
            marked(Language::Korean)
        ]),
        None,
        "current ones with a welcome board in each language"
    );
}
