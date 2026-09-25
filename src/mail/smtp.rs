//! Thin SMTP transport matching the source mailer: host/port/from, no AUTH env.
//! TLS/auth are not configured in the source; this client speaks plaintext SMTP.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use super::{MailSendError, SmtpConfig};

const SMTP_TIMEOUT: Duration = Duration::from_secs(15);

pub async fn send_mail_op(
    smtp: &SmtpConfig,
    op: &'static str,
    from_header: &str,
    envelope_from: &str,
    to: &str,
    subject: &str,
    text: &str,
) -> Result<(), MailSendError> {
    match send_mail_inner(smtp, from_header, envelope_from, to, subject, text).await {
        Ok(()) => Ok(()),
        Err(err) => Err(MailSendError::from_cause(op, err)),
    }
}

async fn send_mail_inner(
    smtp: &SmtpConfig,
    from_header: &str,
    envelope_from: &str,
    to: &str,
    subject: &str,
    text: &str,
) -> Result<(), SmtpIoError> {
    let addr = format!("{}:{}", smtp.host, smtp.port);
    let stream = tokio::time::timeout(SMTP_TIMEOUT, TcpStream::connect(&addr))
        .await
        .map_err(|_| SmtpIoError {
            code: "timeout".into(),
            message: "connect timed out".into(),
        })?
        .map_err(|err| SmtpIoError {
            code: err.kind().to_string(),
            message: "connect failed".into(),
        })?;
    stream.set_nodelay(true).ok();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    expect_code(&mut reader, 220).await?;
    command(
        &mut writer,
        &mut reader,
        &format!("EHLO {}\r\n", ehlo_name()),
        250,
    )
    .await?;
    command(
        &mut writer,
        &mut reader,
        &format!("MAIL FROM:<{envelope_from}>\r\n"),
        250,
    )
    .await?;
    command(
        &mut writer,
        &mut reader,
        &format!("RCPT TO:<{to}>\r\n"),
        250,
    )
    .await?;
    command(&mut writer, &mut reader, "DATA\r\n", 354).await?;
    let body = format_message(from_header, to, subject, text);
    tokio::time::timeout(SMTP_TIMEOUT, writer.write_all(body.as_bytes()))
        .await
        .map_err(|_| SmtpIoError {
            code: "timeout".into(),
            message: "data timed out".into(),
        })?
        .map_err(|err| SmtpIoError {
            code: err.kind().to_string(),
            message: "data write failed".into(),
        })?;
    expect_code(&mut reader, 250).await?;
    let _ = command(&mut writer, &mut reader, "QUIT\r\n", 221).await;
    Ok(())
}

fn ehlo_name() -> &'static str {
    "fvoci"
}

fn format_message(from: &str, to: &str, subject: &str, text: &str) -> String {
    let mut lines = Vec::new();
    lines.push(format!("From: {from}"));
    lines.push(format!("To: {to}"));
    lines.push(format!("Subject: {subject}"));
    lines.push("MIME-Version: 1.0".to_string());
    lines.push("Content-Type: text/plain; charset=utf-8".to_string());
    lines.push("".to_string());
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with('.') {
            lines.push(format!(".{line}"));
        } else {
            lines.push(line.to_string());
        }
    }
    lines.push(".".to_string());
    let mut out = lines.join("\r\n");
    out.push_str("\r\n");
    out
}

struct SmtpIoError {
    code: String,
    message: String,
}

impl std::fmt::Display for SmtpIoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

async fn command(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    cmd: &str,
    expected: u16,
) -> Result<String, SmtpIoError> {
    tokio::time::timeout(SMTP_TIMEOUT, writer.write_all(cmd.as_bytes()))
        .await
        .map_err(|_| SmtpIoError {
            code: "timeout".into(),
            message: "write timed out".into(),
        })?
        .map_err(|err| SmtpIoError {
            code: err.kind().to_string(),
            message: "write failed".into(),
        })?;
    expect_code(reader, expected).await
}

async fn expect_code(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    expected: u16,
) -> Result<String, SmtpIoError> {
    let (code, text) = read_reply(reader).await?;
    if code != expected {
        return Err(SmtpIoError {
            code: code.to_string(),
            message: "smtp reply".into(),
        });
    }
    Ok(text)
}

async fn read_reply(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
) -> Result<(u16, String), SmtpIoError> {
    loop {
        let mut line = String::new();
        let n = tokio::time::timeout(SMTP_TIMEOUT, reader.read_line(&mut line))
            .await
            .map_err(|_| SmtpIoError {
                code: "timeout".into(),
                message: "read timed out".into(),
            })?
            .map_err(|err| SmtpIoError {
                code: err.kind().to_string(),
                message: "read failed".into(),
            })?;
        if n == 0 {
            return Err(SmtpIoError {
                code: "eof".into(),
                message: "smtp closed".into(),
            });
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.len() < 3 {
            return Err(SmtpIoError {
                code: "protocol".into(),
                message: "short smtp line".into(),
            });
        }
        let code: u16 = line[..3].parse().map_err(|_| SmtpIoError {
            code: "protocol".into(),
            message: "smtp code".into(),
        })?;
        if line.len() == 3 || line.as_bytes().get(3) == Some(&b' ') {
            return Ok((code, line.to_string()));
        }
        if line.as_bytes().get(3) != Some(&b'-') {
            return Err(SmtpIoError {
                code: "protocol".into(),
                message: "smtp separator".into(),
            });
        }
    }
}

impl MailSendError {
    fn from_cause(op: &'static str, err: SmtpIoError) -> Self {
        Self { op, code: err.code }
    }
}

#[cfg(test)]
mod tests {
    use super::format_message;

    #[test]
    fn dots_at_line_start_are_stuffed() {
        let body = format_message("a@b", "c@d", "s", "ok\n.hide\n.");
        assert!(body.contains("\r\n..hide\r\n"));
        assert!(body.ends_with("\r\n..\r\n.\r\n"));
    }
}
