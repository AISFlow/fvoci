//! Test client for the installed image; this executable is not shipped in it.
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::{header, Client, Method};
use serde_json::{json, Value};
use std::{
    error::Error,
    io::{Cursor, Read, Write},
    path::Path,
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Installed {
    base: String,
    private: Client,
    public: Client,
}

impl Installed {
    async fn request(
        &self,
        path: &str,
        method: Method,
        body: Option<Value>,
        authenticated: bool,
        status: u16,
    ) -> Result<(Vec<u8>, header::HeaderMap)> {
        let client = if authenticated {
            &self.private
        } else {
            &self.public
        };
        let mut request = client.request(method, format!("{}{path}", self.base));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        assert_eq!(response.status().as_u16(), status, "installed API status");
        let headers = response.headers().clone();
        Ok((response.bytes().await?.to_vec(), headers))
    }

    async fn api(
        &self,
        path: &str,
        method: Method,
        body: Option<Value>,
        status: u16,
    ) -> Result<Value> {
        let (bytes, _) = self.request(path, method, body, true, status).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

fn session_from_jar(path: &str) -> Result<String> {
    let jar = std::fs::read_to_string(path)?;
    for line in jar.lines() {
        let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
        if line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() == 7 && fields[5] == "fvoci_session" {
            return Ok(fields[6].to_owned());
        }
    }
    Err("session missing from test cookie jar".into())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [base, workspace, cookie_file, state_file, phase] = args.as_slice() else {
        return Err(
            "usage: install-smoke <base> <workspace> <cookie-file> <state-file> create|restart"
                .into(),
        );
    };
    let mut headers = header::HeaderMap::new();
    headers.insert(header::ORIGIN, base.parse()?);
    let public = Client::builder()
        .default_headers(headers.clone())
        .timeout(Duration::from_secs(40))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    headers.insert(
        header::COOKIE,
        format!("fvoci_session={}", session_from_jar(cookie_file)?).parse()?,
    );
    let private = Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(40))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let installed = Installed {
        base: base.clone(),
        private,
        public,
    };
    let root = format!("/api/v1/workspaces/{workspace}");
    if phase == "restart" {
        let state: Value = serde_json::from_slice(&std::fs::read(state_file)?)?;
        let document = state["document"].as_str().ok_or("missing document state")?;
        let body = installed
            .api(
                &format!("{root}/documents/{document}/body"),
                Method::GET,
                None,
                200,
            )
            .await?;
        assert_eq!(body["contentJson"], state["body"]);
        let job = state["job"].as_str().ok_or("missing import state")?;
        let job = installed
            .api(
                &format!("/api/v1/import/{job}?workspaceId={workspace}"),
                Method::GET,
                None,
                200,
            )
            .await?;
        assert_eq!(job["status"], "completed");
        println!("imported and edited document plus completed import job survive restart: ok");
        return Ok(());
    }
    if phase != "create" {
        return Err("phase must be create or restart".into());
    }
    let markdown = "# 설치 검증 🎉\n\n**한글 본문** [링크](https://example.com/)\n\n| 항목 | 값 |\n| --- | --- |\n| 표 | 보존 |\n";
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    archive.start_file(
        "설치 검증.md",
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated),
    )?;
    archive.write_all(markdown.as_bytes())?;
    let archive = archive.finish()?.into_inner();
    let job = installed.api("/api/v1/import", Method::POST, Some(json!({"workspaceId": workspace, "source": "markdown-zip", "zipBase64": STANDARD.encode(archive)})), 201).await?;
    assert_eq!(job["status"], "completed");
    let documents = job["createdDocumentIds"]
        .as_array()
        .ok_or("missing imported documents")?;
    assert_eq!(documents.len(), 1);
    let document = documents[0].as_str().ok_or("missing document id")?;
    let path = format!("{root}/documents/{document}");
    let body = installed
        .api(&format!("{path}/body"), Method::GET, None, 200)
        .await?;
    let encoded = body["contentJson"].to_string();
    for token in ["한글 본문", "🎉", "\"table\"", "\"bold\"", "\"link\""] {
        assert!(encoded.contains(token), "{token}");
    }
    installed
        .api(
            &format!("{path}/body"),
            Method::PUT,
            Some(json!({"contentMd": format!("{markdown}\n후속 편집 저장\n")})),
            200,
        )
        .await?;
    let body = installed
        .api(&format!("{path}/body"), Method::GET, None, 200)
        .await?;
    assert!(body["contentJson"].to_string().contains("후속 편집 저장"));
    for (extension, mime) in [
        ("md", "text/markdown"),
        ("pdf", "application/pdf"),
        (
            "docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
        (
            "pptx",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        ),
    ] {
        let (bytes, headers) = installed
            .request(&format!("{path}/{extension}"), Method::GET, None, true, 200)
            .await?;
        assert!(headers[header::CONTENT_TYPE].to_str()?.starts_with(mime));
        assert_eq!(headers[header::CACHE_CONTROL], "private, no-store");
        assert!(!bytes.is_empty());
        match extension {
            "md" => assert!(std::str::from_utf8(&bytes)?.contains("후속 편집 저장")),
            "pdf" => {
                assert!(bytes.starts_with(b"%PDF-"));
                assert!(bytes.windows(5).any(|w| w == b"%%EOF"));
            }
            _ => {
                let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
                let prefix = if extension == "docx" {
                    "word/"
                } else {
                    "ppt/slides/"
                };
                let mut found = false;
                for i in 0..zip.len() {
                    let mut part = zip.by_index(i)?;
                    if part.name().starts_with(prefix) && part.name().ends_with(".xml") {
                        let mut xml = String::new();
                        part.read_to_string(&mut xml)?;
                        found |= xml.contains("후속 편집 저장");
                    }
                }
                assert!(found, "export must contain edited text");
            }
        }
        installed
            .request(
                &format!("{path}/{extension}"),
                Method::GET,
                None,
                false,
                401,
            )
            .await?;
    }
    let share = installed
        .api(
            &format!("{path}/share-links"),
            Method::POST,
            Some(json!({})),
            201,
        )
        .await?;
    let token = share["url"]
        .as_str()
        .ok_or("missing share URL")?
        .rsplit('/')
        .next()
        .ok_or("missing token")?;
    let shared = format!("/api/v1/share/{token}/pdf");
    let (bytes, _) = installed
        .request(&shared, Method::GET, None, false, 200)
        .await?;
    assert!(bytes.starts_with(b"%PDF-"));
    let share_id = share["id"].as_str().ok_or("missing share id")?;
    installed
        .api(
            &format!("{root}/share-links/{share_id}"),
            Method::DELETE,
            None,
            200,
        )
        .await?;
    installed
        .request(&shared, Method::GET, None, false, 404)
        .await?;
    std::fs::write(
        Path::new(state_file),
        serde_json::to_vec(
            &json!({"document": document, "body": body["contentJson"], "job": job["id"]}),
        )?,
    )?;
    println!(
        "installed Rust import, edit, MD/PDF/DOCX/PPTX exports, public PDF and revocation: ok"
    );
    Ok(())
}
