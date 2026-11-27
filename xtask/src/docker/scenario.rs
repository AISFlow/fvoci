//! What a person does first, from outside the installation, through `curl`: the web
//! app's first page, sign up, a board, a card, words in its body, and a search that
//! finds the card by those words. The words are Korean written together and the search
//! writes them apart, which only nori's reading finds: a search server without the
//! plugin fails here.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail, ensure};
use serde_json::{Value, json};

use super::Trial;
use crate::steps::{capture, interrupted, say};

/// What the card's body says.
const BODY: &str = "결제 후 7일이 지나면 환불규정을 따라 처리합니다.";

/// What is searched for: the compound of the body, written apart.
const QUESTION: &str = "환불 규정";

/// How long the card may take to be found: the server sends what changed to the
/// search server every second, and the index shows it after its refresh.
const FIND_WITHIN: Duration = Duration::from_secs(90);

/// How long one request may take, in seconds.
const REQUEST_SECONDS: &str = "15";

/// A client of the installation, signed in through its cookie jar.
pub(super) struct Client<'a> {
    base: &'a str,
    jar: PathBuf,
}

/// An answer: the status and the body.
pub(super) struct Answer {
    pub(super) status: u16,
    pub(super) body: String,
}

impl Answer {
    fn json(&self) -> Result<Value> {
        serde_json::from_str(&self.body).with_context(|| format!("not JSON: {}", self.body))
    }
}

impl<'a> Client<'a> {
    /// A client of the installation at `base`, with an empty cookie jar in `directory`
    /// that only its owner can read.
    pub(super) fn new(base: &'a str, directory: &Path) -> Result<Self> {
        let client = Client {
            base,
            jar: directory.join("cookies.txt"),
        };
        super::write_private(&client.jar, "")?;
        Ok(client)
    }

    /// Sends a request as the web app does: with the `Origin` of the server's address
    /// and `X-Requested-With`, which a request that changes something needs.
    pub(super) fn send(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Answer> {
        // The body goes through a file: it may hold a password, which a command line
        // shows to every process of the machine.
        let request = self.jar.with_file_name("request.json");
        let from_file = format!("@{}", request.to_string_lossy());
        let mut extra = Vec::new();
        if let Some(body) = body {
            super::write_private(&request, &body.to_string())?;
            extra.extend([
                "--header",
                "Content-Type: application/json",
                "--data-binary",
                &from_file,
            ]);
        }
        let answer = self.curl(method, path, &extra);
        let _ = std::fs::remove_file(&request);
        answer
    }

    /// Posts the file at `file` to `path` as a form's field `file`, as the web app
    /// uploads one.
    pub(super) fn upload(&self, path: &str, file: &Path) -> Result<Answer> {
        let form = format!("file=@{}", file.to_string_lossy());
        self.curl("POST", path, &["--form", &form])
    }

    /// Runs `curl` for one request, with the session's cookies and the headers of the
    /// web app, `extra` before the address.
    fn curl(&self, method: &str, path: &str, extra: &[&str]) -> Result<Answer> {
        let url = format!("{}{path}", self.base);
        let jar = self.jar.to_string_lossy();
        let origin = format!("Origin: {}", self.base);
        let mut arguments = vec![
            "--silent",
            "--show-error",
            "--max-time",
            REQUEST_SECONDS,
            "--cookie",
            &jar,
            "--cookie-jar",
            &jar,
            "--request",
            method,
            "--header",
            &origin,
            "--header",
            "X-Requested-With: XMLHttpRequest",
            "--write-out",
            "\n%{http_code}",
        ];
        arguments.extend_from_slice(extra);
        arguments.push(&url);
        let answer = capture("curl", &arguments)?;
        if !answer.succeeded {
            bail!("{method} {path}: curl failed: {}", answer.error);
        }
        let (body, status) = answer
            .output
            .rsplit_once('\n')
            .unwrap_or(("", &answer.output));
        let status = status
            .parse()
            .with_context(|| format!("{method} {path}: no status in {status:?}"))?;
        Ok(Answer {
            status,
            body: body.to_owned(),
        })
    }

    /// Sends a request that must succeed, and reads its JSON.
    pub(super) fn expect(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        let answer = self.send(method, path, body)?;
        ensure!(
            (200..300).contains(&answer.status),
            "{method} {path} answered {}: {}",
            answer.status,
            answer.body
        );
        if answer.body.trim().is_empty() {
            return Ok(Value::Null);
        }
        answer.json()
    }
}

/// The text of `value`'s member `key`.
pub(super) fn text(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("no {key} in {value}"))
}

/// A body of one paragraph, as the editor writes it.
fn paragraph(words: &str) -> Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": words}]},
    ]})
}

/// Percent-encodes everything but ASCII letters and digits, for a query.
fn encoded(words: &str) -> String {
    words
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// Goes from the first page to a search that finds the card.
///
/// # Errors
///
/// Fails at the first step that does not answer as it should.
pub(super) fn run(trial: &Trial, directory: &Path) -> Result<()> {
    // The session's cookie is the owner's only.
    let client = Client::new(&trial.base, directory)?;

    let page = client.send("GET", "/", None)?;
    ensure!(
        page.status == 200 && page.body.contains("<script") && page.body.contains("static/"),
        "the first page is not the web app ({}): {}",
        page.status,
        page.body.chars().take(300).collect::<String>()
    );
    let ping = client.expect("GET", "/api/v1/ping", None)?;
    say(&format!("    the web app answers, and ping: {ping}"));
    ensure!(
        ping["commit"] == trial.images.revision() && ping["buildDate"] == trial.images.created(),
        "ping does not name the commit {} of {}: {ping}",
        trial.images.revision(),
        trial.images.created()
    );

    let password = super::hex(&super::random::<16>()?);
    client.expect(
        "POST",
        "/api/v1/register",
        Some(&json!({
            "username": "smoke",
            "email": "smoke@example.com",
            "password": password,
            "organizationName": "Smoke",
        })),
    )?;
    // Signing up does not sign in.
    client.expect(
        "POST",
        "/api/v1/login",
        Some(&json!({"username": "smoke", "password": password})),
    )?;
    let workspaces = client.expect("GET", "/api/v1/users/me/workspaces", None)?;
    let workspace = workspaces
        .get(0)
        .map(|workspace| text(workspace, "id"))
        .context("the new account has no workspace")??;
    let w = format!("/api/v1/w/{workspace}");
    let board = client.expect(
        "POST",
        &format!("{w}/boards"),
        Some(&json!({"visibility": "open", "title": "Smoke"})),
    )?;
    let board = text(&board, "id")?;
    let card_id = write_card(&client, &w, &board)?;
    say("    signed up, made a board and a card, and wrote its body");
    find_card(&client, &w, &card_id)
}

/// Makes a card on the board at `w` (a workspace's path) with [`BODY`] for its body,
/// and returns its identifier.
///
/// # Errors
///
/// Fails if the card is not made or its body not written.
pub(super) fn write_card(client: &Client<'_>, w: &str, board: &str) -> Result<String> {
    let card = client.expect(
        "POST",
        &format!("{w}/boards/{board}/cards"),
        Some(&json!({"title": "고객 응대 안내"})),
    )?;
    let card_id = text(&card, "id")?;
    let read_at = card
        .get("updatedAt")
        .and_then(Value::as_i64)
        .with_context(|| format!("no updatedAt in {card}"))?;
    client.expect(
        "PATCH",
        &format!("{w}/cards/{card_id}"),
        Some(&json!({"body": paragraph(BODY), "readAt": read_at})),
    )?;
    Ok(card_id)
}

/// Searches the workspace at `w` for [`QUESTION`] until the card `card_id` is found.
///
/// # Errors
///
/// Fails if the search fails or does not find the card within [`FIND_WITHIN`].
pub(super) fn find_card(client: &Client<'_>, w: &str, card_id: &str) -> Result<()> {
    let path = format!("{w}/search?q={}", encoded(QUESTION));
    let started = Instant::now();
    loop {
        let answer = client.send("GET", &path, None)?;
        if answer.status == 200 {
            let hits = answer.json()?;
            let found = hits.as_array().is_some_and(|hits| {
                hits.iter()
                    .any(|hit| hit.get("id").and_then(Value::as_str) == Some(card_id))
            });
            if found {
                say(&format!(
                    "    \"{QUESTION}\" found the card in {:.1}s",
                    started.elapsed().as_secs_f64()
                ));
                return Ok(());
            }
        } else if answer.status != 429 && answer.status != 503 {
            bail!("the search answered {}: {}", answer.status, answer.body);
        }
        if interrupted() || started.elapsed() > FIND_WITHIN {
            bail!(
                "\"{QUESTION}\" did not find the card within {FIND_WITHIN:?}: {}",
                answer.body
            );
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_keeps_letters_and_digits_and_encodes_the_rest() {
        assert_eq!(encoded("a1 b"), "a1%20b");
        assert_eq!(encoded("환"), "%ED%99%98");
    }
}
