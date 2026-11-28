//! A body written in Markdown and read back is the same body, for each kind of node
//! and mark; what Markdown does not hold is named.

use fvoci_model::{Body, ComplexBlockReason};
use serde_json::{Value, json};

use super::write::{Names, body_to_markdown};
use super::{markdown_body, markdown_nodes};

const PERSON: &str = "0190a5b4-0000-7000-8000-000000000001";
const CARD: &str = "0190a5b4-0000-7000-8000-000000000002";

fn doc(content: &Value) -> Body {
    Body::parse(json!({"type": "doc", "content": content}))
        .expect("a body")
        .expect("not empty")
}

fn text(words: &str) -> Value {
    json!({"type": "text", "text": words})
}

fn marked(words: &str, marks: &Value) -> Value {
    json!({"type": "text", "text": words, "marks": marks})
}

fn paragraph(content: &[Value]) -> Value {
    json!({"type": "paragraph", "content": content})
}

/// A document as an editor holds it whichever way its marks were listed: the marks of
/// each text in the order of their names, and texts of the same marks one text.
fn normalized(node: &Value) -> Value {
    let mut node = node.clone();
    if let Some(marks) = node.get_mut("marks").and_then(Value::as_array_mut) {
        marks.sort_by_key(|mark| mark["type"].as_str().unwrap_or_default().to_owned());
    }
    if let Some(content) = node.get("content").and_then(Value::as_array) {
        let mut merged: Vec<Value> = Vec::new();
        for child in content.iter().map(normalized) {
            if let Some(last) = merged.last_mut()
                && last["type"] == "text"
                && child["type"] == "text"
                && last.get("marks") == child.get("marks")
            {
                let joined = format!(
                    "{}{}",
                    last["text"].as_str().unwrap_or_default(),
                    child["text"].as_str().unwrap_or_default()
                );
                last["text"] = Value::String(joined);
                continue;
            }
            merged.push(child);
        }
        node["content"] = Value::Array(merged);
    }
    node
}

/// Writes the body in Markdown, reads it back, and asks that it is the same body and
/// that the Markdown holds all of it; the Markdown, for the test to look at.
#[track_caller]
fn check_round_trip(body: &Body) -> String {
    let written = body_to_markdown(body.document(), &Names::default());
    assert_eq!(written.complex, [], "all is held: {}", written.markdown);
    let read = markdown_body(&written.markdown, "markdown")
        .expect("the Markdown reads")
        .expect("a body");
    assert_eq!(
        normalized(read.document()),
        normalized(body.document()),
        "the Markdown:\n{}",
        written.markdown
    );
    written.markdown
}

#[test]
fn each_block_comes_back_the_same() {
    let item = |words: &str| json!({"type": "listItem", "content": [paragraph(&[text(words)])]});
    let body = doc(&json!([
        {"type": "heading", "attrs": {"level": 1}, "content": [text("Title")]},
        {"type": "heading", "attrs": {"level": 3}, "content": [text("Ends in #")]},
        paragraph(&[text("plain")]),
        {"type": "blockquote", "content": [
            paragraph(&[text("quoted")]),
            {"type": "bulletList", "content": [item("in a quote")]},
        ]},
        {"type": "bulletList", "content": [
            {"type": "listItem", "content": [
                paragraph(&[text("outer")]),
                {"type": "orderedList", "attrs": {"start": 7}, "content": [item("seventh"), item("eighth")]},
            ]},
        ]},
        {"type": "bulletList", "content": [item("a list right after another")]},
        {"type": "taskList", "content": [
            {"type": "taskItem", "attrs": {"checked": true}, "content": [paragraph(&[text("done")])]},
            {"type": "taskItem", "attrs": {"checked": false}, "content": [paragraph(&[text("[ ] not a box")])]},
        ]},
        {"type": "orderedList", "attrs": {"start": 1}, "content": [item("one")]},
        {"type": "orderedList", "attrs": {"start": 1}, "content": [item("another one")]},
        {"type": "codeBlock", "attrs": {"language": "rust"}, "content": [text("let a = \"```\";\n\n*not* bold")]},
        {"type": "codeBlock", "content": [text("ends in a line\n")]},
        {"type": "horizontalRule"},
        {"type": "image", "attrs": {"fileId": "moon.png", "alt": "the [full] moon"}},
        {"type": "file", "attrs": {"fileId": "report.pdf"}},
        {"type": "blockMath", "attrs": {"latex": "\\int_0^1 x\\,dx\n\\quad = \\frac12"}},
        {"type": "table", "content": [
            {"type": "tableRow", "content": [
                {"type": "tableHeader", "content": [paragraph(&[text("a | b")])]},
                {"type": "tableHeader", "content": [{"type": "paragraph"}]},
            ]},
            {"type": "tableRow", "content": [
                {"type": "tableCell", "content": [paragraph(&[marked("x|y", &json!([json!({"type": "code"})]))])]},
                {"type": "tableCell", "content": [paragraph(&[text("2")])]},
            ]},
        ]},
    ]));
    let markdown = check_round_trip(&body);
    assert!(
        markdown.contains("![the \\[full\\] moon](fvoci-file://moon.png)"),
        "{markdown}"
    );
    assert!(
        markdown.contains("[report.pdf](fvoci-file://report.pdf)"),
        "{markdown}"
    );
}

#[test]
fn each_mark_and_inline_node_comes_back_the_same() {
    let bold = json!({"type": "bold"});
    let italic = json!({"type": "italic"});
    let strike = json!({"type": "strike"});
    let code = json!({"type": "code"});
    let link = json!({"type": "link", "attrs": {"href": "https://example.com/a_(b)"}});
    let body = doc(&json!([
        paragraph(&[
            text("plain "),
            marked("bold", &json!([bold])),
            marked("both", &json!([bold, italic])),
            text(" "),
            marked("italic", &json!([italic])),
            text(" mid"),
            marked("struck", &json!([strike])),
            text(" and "),
            marked("a `tick`", &json!([code])),
            text(" then "),
            marked("linked ", &json!([link])),
            marked("bold link", &json!([link, bold])),
        ]),
        paragraph(&[
            text("Ask "),
            json!({"type": "mention", "attrs": {"userId": PERSON}}),
            text(" of "),
            json!({"type": "reference", "attrs": {"id": CARD}}),
            text(", where "),
            json!({"type": "inlineMath", "attrs": {"latex": "x^2 \\$"}}),
            text(" is cheap"),
            json!({"type": "hardBreak"}),
            text("# not a heading, - nor a list, 2024. nor a number"),
        ]),
        paragraph(&[text("  spaces kept  ")]),
    ]));
    check_round_trip(&body);
}

#[test]
fn the_spaces_at_the_edge_of_bold_text_lose_only_the_bold() {
    let bold = json!({"type": "bold"});
    let body = doc(&json!([paragraph(&[
        text("a"),
        marked(" bold ", &json!([bold])),
        text("b"),
    ])]));
    let written = body_to_markdown(body.document(), &Names::default());
    assert_eq!(written.markdown, "a **bold** b");
    let read = markdown_body(&written.markdown, "markdown")
        .expect("read")
        .expect("a body");
    assert_eq!(
        read.document()["content"][0]["content"],
        json!([text("a "), marked("bold", &json!([bold])), text(" b")])
    );
}

#[test]
fn text_that_looks_like_markdown_stays_text() {
    let body = doc(&json!([
        paragraph(&[text(
            "[[not a reference]] @[nobody](fvoci-user://x) costs $5 or $6"
        )]),
        paragraph(&[text(
            "*a* _b_ snake_case <b>html</b> &amp; \\ | ~~c~~ `d` ![e](f)"
        )]),
        paragraph(&[text("> not quoted")]),
        paragraph(&[text("1) not a list")]),
        paragraph(&[text("---")]),
        paragraph(&[text("+ plus")]),
        paragraph(&[text("===")]),
    ]));
    check_round_trip(&body);
}

#[test]
fn what_markdown_does_not_hold_is_named() {
    let cell = |kind: &str, content: Value| json!({"type": kind, "content": content});
    let body = doc(&json!([
        {"type": "table", "content": [
            {"type": "tableRow", "content": [
                {"type": "tableHeader", "attrs": {"colspan": 2}, "content": [paragraph(&[text("h")])]},
            ]},
            {"type": "tableRow", "content": [
                cell("tableCell", json!([paragraph(&[text("a")]), paragraph(&[text("b")])])),
                cell("tableHeader", json!([paragraph(&[text("c")])])),
            ]},
        ]},
        paragraph(&[json!({"type": "inlineMath", "attrs": {"latex": "a$b"}})]),
    ]));
    let written = body_to_markdown(body.document(), &Names::default());
    let named: Vec<(&str, ComplexBlockReason)> = written
        .complex
        .iter()
        .map(|block| (block.path.as_str(), block.reason))
        .collect();
    assert_eq!(
        named,
        [
            (
                "/content/0/content/0/content/0",
                ComplexBlockReason::MergedCell
            ),
            ("/content/0/content/1", ComplexBlockReason::UnevenTable),
            (
                "/content/0/content/1/content/0",
                ComplexBlockReason::CellWithBlocks
            ),
            ("/content/1/content/0", ComplexBlockReason::UnwrittenFormula),
        ],
        "{}",
        written.markdown
    );
}

#[test]
fn headings_deeper_than_three_are_read_as_three_and_formulas_on_their_own_lines() {
    let nodes = markdown_nodes("#### deep\n\n$$\n\\sum_i i\n$$", "markdown").expect("read");
    assert_eq!(
        nodes,
        [
            json!({"type": "heading", "attrs": {"level": 3}, "content": [text("deep")]}),
            json!({"type": "blockMath", "attrs": {"latex": "\\sum_i i"}}),
        ]
    );
}

#[test]
fn a_mention_is_written_by_the_name_it_is_shown_by() {
    let body = doc(&json!([paragraph(&[
        text("mail@"),
        json!({"type": "mention", "attrs": {"userId": PERSON}}),
    ])]));
    let mut names = Names::default();
    names
        .users
        .insert(PERSON.to_owned(), "Kim [Lab]".to_owned());
    let written = body_to_markdown(body.document(), &names);
    assert_eq!(
        written.markdown,
        format!("mail@@[Kim \\[Lab\\]](fvoci-user://{PERSON})")
    );
    let read = markdown_body(&written.markdown, "markdown")
        .expect("read")
        .expect("a body");
    assert_eq!(read.document(), body.document(), "{}", written.markdown);
}

#[test]
fn a_mention_of_whom_the_reader_does_not_see_is_written_by_no_name() {
    let body = doc(&json!([paragraph(&[
        text("ask "),
        json!({"type": "mention", "attrs": {"userId": PERSON}}),
        text(" and "),
        json!({"type": "mention", "attrs": {"userId": CARD}}),
    ])]));
    // The names hold only whom the reader sees: here, no one (and `CARD` is nobody).
    let written = body_to_markdown(body.document(), &Names::default());
    assert_eq!(
        written.markdown,
        format!("ask @[](fvoci-user://{PERSON}) and @[](fvoci-user://{CARD})")
    );
    assert!(written.complex.is_empty(), "{:?}", written.complex);
    let read = markdown_body(&written.markdown, "markdown")
        .expect("read")
        .expect("a body");
    assert_eq!(read.document(), body.document(), "{}", written.markdown);
}

#[test]
fn a_heading_with_a_line_break_is_written_on_one_line_and_named() {
    let body = doc(&json!([
        {"type": "heading", "attrs": {"level": 2}, "content": [
            text("one"), {"type": "hardBreak"}, text("two"),
        ]},
        paragraph(&[text("after")]),
    ]));
    let written = body_to_markdown(body.document(), &Names::default());
    assert_eq!(written.markdown, "## one two\n\nafter");
    let named: Vec<(&str, ComplexBlockReason)> = written
        .complex
        .iter()
        .map(|block| (block.path.as_str(), block.reason))
        .collect();
    assert_eq!(
        named,
        [("/content/0", ComplexBlockReason::HeadingWithBreak)]
    );
    // Read back, the heading is one, and what follows it is as it was.
    let read = markdown_body(&written.markdown, "markdown")
        .expect("read")
        .expect("a body");
    assert_eq!(
        read.document()["content"],
        json!([
            {"type": "heading", "attrs": {"level": 2}, "content": [text("one two")]},
            paragraph(&[text("after")]),
        ]),
        "{}",
        written.markdown
    );
}

fn toggle(summary: &[Value], hidden: &Value) -> Value {
    let summary = if summary.is_empty() {
        json!({"type": "detailsSummary"})
    } else {
        json!({"type": "detailsSummary", "content": summary})
    };
    json!({"type": "details", "content": [summary, {"type": "detailsContent", "content": hidden}]})
}

fn callout(tone: &str, content: &Value) -> Value {
    json!({"type": "callout", "attrs": {"tone": tone}, "content": content})
}

#[test]
fn toggles_and_callouts_come_back_the_same() {
    let bold = json!([{"type": "bold"}]);
    let item = json!({"type": "listItem", "content": [paragraph(&[text("listed")])]});
    let cell =
        |kind: &str, words: &str| json!({"type": kind, "content": [paragraph(&[text(words)])]});
    let body = doc(&json!([
        toggle(&[text("Why "), marked("this", &bold), text(" <now>?")], &json!([
            paragraph(&[text("Because.")]),
            {"type": "table", "content": [
                {"type": "tableRow", "content": [cell("tableHeader", "a"), cell("tableHeader", "b")]},
                {"type": "tableRow", "content": [cell("tableCell", "1"), cell("tableCell", "2")]},
            ]},
            callout("tip", &json!([paragraph(&[text("inside")]), {"type": "bulletList", "content": [item]}])),
            toggle(&[], &json!([paragraph(&[text("deeper")])])),
        ])),
        toggle(&[text("nothing inside")], &json!([{"type": "paragraph"}])),
        toggle(&[text("ends at </summary> \\")], &json!([paragraph(&[text("not yet")])])),
        callout("info", &json!([paragraph(&[text("a note")])])),
        callout("warn", &json!([{"type": "codeBlock", "content": [text("rm -rf")]}])),
        callout("danger", &json!([{"type": "paragraph"}])),
        {"type": "blockquote", "content": [paragraph(&[text("[!NOTE]")]), paragraph(&[text("a quote")])]},
    ]));
    let markdown = check_round_trip(&body);
    assert!(
        markdown.starts_with("<details>\n<summary>Why **this** &lt;now&gt;?</summary>\n\nBecause."),
        "{markdown}"
    );
    for alert in [
        "> [!NOTE]\n> a note",
        "> [!WARNING]\n> ```",
        "> [!CAUTION]",
        "> [!TIP]\n> inside",
    ] {
        assert!(markdown.contains(alert), "{alert} in {markdown}");
    }
    assert!(
        markdown.contains("<summary>nothing inside</summary>\n\n</details>"),
        "{markdown}"
    );
}

#[test]
fn markdown_from_elsewhere_reads_alerts_and_toggles_where_they_may_stand() {
    let nodes = markdown_nodes(
        "> [!IMPORTANT]\n> heads up\n\n> [!tip]\n\n\
         <details open><summary>Q</summary>\n\nA\n</details>\n\n\
         - item\n\n  <details>\n  <summary>S</summary>\n\n  hidden\n\n  </details>\n\n\
         > [!NOTE]\n> > [!TIP]\n> > inner\n\n\
         </details>\n\n\
         <details>\n<summary>left open</summary>\n\n| a |\n| - |\n| 1 |",
        "markdown",
    )
    .expect("read");
    let cell =
        |kind: &str, words: &str| json!({"type": kind, "content": [paragraph(&[text(words)])]});
    assert_eq!(
        nodes,
        [
            callout("info", &json!([paragraph(&[text("heads up")])])),
            callout("tip", &json!([{"type": "paragraph"}])),
            toggle(&[text("Q")], &json!([paragraph(&[text("A")])])),
            json!({"type": "bulletList", "content": [{"type": "listItem", "content": [
                paragraph(&[text("item")]),
                paragraph(&[text("S")]),
                paragraph(&[text("hidden")]),
            ]}]}),
            callout(
                "info",
                &json!([{"type": "blockquote", "content": [paragraph(&[text("inner")])]}])
            ),
            // The end of a toggle that is not open is HTML, shown as text.
            paragraph(&[text("</details>")]),
            toggle(
                &[text("left open")],
                &json!([{"type": "table", "content": [
                    {"type": "tableRow", "content": [cell("tableHeader", "a")]},
                    {"type": "tableRow", "content": [cell("tableCell", "1")]},
                ]}])
            ),
        ]
    );
}

#[test]
fn a_toggle_or_a_table_in_a_callout_is_read_as_its_parts() {
    let nodes = markdown_nodes(
        "> [!WARNING]\n> <details>\n> <summary>S</summary>\n>\n> x\n>\n> </details>\n>\n> | a |\n> | - |\n> | 1 |",
        "markdown",
    )
    .expect("read");
    assert_eq!(
        nodes,
        [callout(
            "warn",
            &json!([
                paragraph(&[text("S")]),
                paragraph(&[text("x")]),
                paragraph(&[text("a")]),
                paragraph(&[text("1")]),
            ])
        )]
    );
}

#[test]
fn a_link_of_an_export_goes_from_the_folder_of_its_file() {
    use super::relative;
    let path = |parts: &[&str]| {
        parts
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>()
    };
    let to = path(&["Wiki", "files", "a b.png"]);
    assert_eq!(relative(&path(&["Wiki"]), &to), "files/a b.png");
    assert_eq!(
        relative(&path(&["Wiki", "One", "Two"]), &to),
        "../../files/a b.png"
    );
    assert_eq!(
        relative(&path(&["Wiki"]), &path(&["Wiki", "One", "Two.md"])),
        "One/Two.md"
    );
    assert_eq!(
        relative(&path(&["Wiki", "One"]), &path(&["Wiki", "One.md"])),
        "../One.md"
    );
}

#[test]
fn an_export_links_what_it_holds_and_names_the_rest_in_plain_text() {
    use std::sync::Arc;

    use super::{ExportPlaces, Exported};
    const PAGE: &str = "0190a5b4-0000-7000-8000-000000000003";
    const UNSEEN: &str = "0190a5b4-0000-7000-8000-000000000004";
    let folder = |parts: &[&str]| {
        parts
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>()
    };
    let mut places = ExportPlaces::default();
    places
        .files
        .insert("f1.png".to_owned(), folder(&["Wiki", "files", "moon.png"]));
    places.blocks.insert(
        PAGE.to_owned(),
        (
            folder(&["Wiki", "Guide", "Setup.md"]),
            "Setup [one]".to_owned(),
        ),
    );
    places
        .titles
        .insert(CARD.to_owned(), "A card *elsewhere*".to_owned());
    let mut names = Names::default();
    names.users.insert(PERSON.to_owned(), "Kim".to_owned());
    names.export = Some(Exported {
        from: folder(&["Wiki"]),
        places: Arc::new(places),
    });
    let body = doc(&json!([
        {"type": "paragraph", "content": [
            {"type": "reference", "attrs": {"id": PAGE}},
            text(" "),
            {"type": "reference", "attrs": {"id": CARD}},
            text(" "),
            {"type": "reference", "attrs": {"id": UNSEEN}},
            text(" "),
            {"type": "mention", "attrs": {"userId": PERSON}},
        ]},
        {"type": "image", "attrs": {"fileId": "f1.png", "alt": "moon"}},
    ]));
    let written = body_to_markdown(body.document(), &names).markdown;
    assert_eq!(
        written,
        format!(
            "[Setup \\[one\\]](<Guide/Setup.md>) A card \\*elsewhere\\* [[{UNSEEN}]] @Kim\n\n\
             ![moon](<files/moon.png>)"
        )
    );
}

/// The line between `<summary>` and `</summary>` of the first toggle of `markdown`.
fn summary_line(markdown: &str) -> &str {
    let start = markdown.find("<summary>").expect("a summary") + "<summary>".len();
    let end = markdown.rfind("</summary>").expect("the end of a summary");
    &markdown[start..end]
}

/// A summary is in a block of HTML, which a renderer passes on as it is: none of its
/// letters is markup there, and the summary still comes back the same.
#[test]
fn the_markup_of_a_summary_is_written_as_character_references() {
    let link = json!([{"type": "link", "attrs": {"href": "https://example.com/?a=1&b=<2>"}}]);
    let plain_link = json!([{"type": "link", "attrs": {"href": "https://example.com/?a=1&b=2"}}]);
    let injected = "<script>alert(1)</script><img src=x onerror=alert(1)> a & b > c &amp;";
    let body = doc(&json!([toggle(
        &[
            text(injected),
            marked(" here", &plain_link),
            json!({"type": "mention", "attrs": {"userId": PERSON}}),
        ],
        &json!([paragraph(&[text("hidden")])]),
    )]));
    let mut names = Names::default();
    names
        .users
        .insert(PERSON.to_owned(), "<b>Kim</b> & co".to_owned());
    let written = body_to_markdown(body.document(), &names);
    assert_eq!(written.complex, [], "all is held: {}", written.markdown);
    let summary = summary_line(&written.markdown);
    assert!(
        !summary.contains(['<', '>']),
        "no tag in the summary: {summary}"
    );
    assert!(
        summary.starts_with("&lt;script&gt;alert(1)&lt;/script&gt;&lt;img"),
        "{summary}"
    );
    check_round_trip(&body);

    // What a character reference cannot be read back in is named, and written without
    // markup all the same.
    for (summary, reason) in [
        (marked("</summary>", &json!([{"type": "code"}])), "code"),
        (
            json!({"type": "inlineMath", "attrs": {"latex": "a<b"}}),
            "formula",
        ),
        (marked("there", &link), "link"),
    ] {
        let body = doc(&json!([toggle(
            &[text("a "), summary, text(" b")],
            &json!([paragraph(&[text("hidden")])]),
        )]));
        let written = body_to_markdown(body.document(), &Names::default());
        assert_eq!(
            written.complex,
            [fvoci_model::ComplexBlock {
                path: "/content/0/content/0".to_owned(),
                reason: ComplexBlockReason::MarkupInSummary,
            }],
            "{reason}: {}",
            written.markdown
        );
        let summary = summary_line(&written.markdown);
        assert!(!summary.contains(['<', '>']), "{reason}: {summary}");
        // What it hides stays hidden: the summary ends where it was written to.
        let read = markdown_nodes(&written.markdown, "markdown").expect("read");
        assert_eq!(read.len(), 1, "{reason}: {read:?}");
        assert_eq!(
            read[0]["content"][1],
            json!({"type": "detailsContent", "content": [paragraph(&[text("hidden")])]}),
            "{reason}"
        );
    }
}

/// In an export, a reference and a person in a toggle's summary are written as the
/// summary writes its letters: the link without `<…>`, nothing HTML reads as markup.
#[test]
fn an_export_writes_a_summary_without_markup() {
    use std::sync::Arc;

    use super::{ExportPlaces, Exported};
    const PAGE: &str = "0190a5b4-0000-7000-8000-000000000003";
    let folder = |parts: &[&str]| {
        parts
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>()
    };
    let mut places = ExportPlaces::default();
    places.blocks.insert(
        PAGE.to_owned(),
        (folder(&["Wiki", "Q & A.md"]), "Q & A".to_owned()),
    );
    let mut names = Names::default();
    names
        .users
        .insert(PERSON.to_owned(), "<b>Kim</b>".to_owned());
    names.export = Some(Exported {
        from: folder(&["Wiki"]),
        places: Arc::new(places),
    });
    let body = doc(&json!([toggle(
        &[
            json!({"type": "reference", "attrs": {"id": PAGE}}),
            text(" "),
            json!({"type": "mention", "attrs": {"userId": PERSON}}),
        ],
        &json!([paragraph(&[text("hidden")])]),
    )]));
    let written = body_to_markdown(body.document(), &names).markdown;
    let summary = summary_line(&written);
    assert_eq!(
        summary, "[Q &amp; A](Q%20&amp;%20A.md) @&lt;b&gt;Kim&lt;/b&gt;",
        "{written}"
    );
}
