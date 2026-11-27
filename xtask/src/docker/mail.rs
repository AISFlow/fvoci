//! Mail through each provider's settings as `docker/compose.yaml` hands them to fvoci:
//! Cloudflare's (the trial's own), then SES's and OCI's, each with its secret from
//! `.env` as a Docker secret. A fake provider answers in fvoci's own network namespace,
//! at a loopback address, the only kind of plain `http` endpoint the server takes; a
//! reset link for the trial's account must reach it, signed or authorized with the
//! keys given. Before them, fvoci with `email_verification` on and no way of sending
//! must refuse to start, saying why.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail, ensure};
use serde_json::json;

use super::Trial;
use super::scenario::Client;
use crate::steps::{capture, interrupted, say};

/// The port the fake provider listens on, inside fvoci's network namespace.
const FAKE_PORT: u16 = 18025;

/// The account whose reset link is mailed: the scenario's.
const RECIPIENT: &str = "smoke@example.com";

/// How long a reset link may take to reach the fake provider.
const ARRIVE_WITHIN: Duration = Duration::from_secs(60);

/// The test key of the server's OCI tests: a key, but no secret of anyone's.
const OCI_KEY: &str = "crates/server/src/app/mail/provider/testdata/oci-test-key-pkcs8.pem";

/// What fvoci's log says when `email_verification` is on and no mail is sent.
const NO_MAIL: &str = "email_verification is on, but this server sends no mail";

/// The fake provider: answers every request as each provider takes a message, and
/// appends each request, head and body, to `/tmp/requests`. It never prints a request.
const FAKE: &str = r#"
use IO::Socket::INET;
my $server = IO::Socket::INET->new(LocalAddr => "127.0.0.1", LocalPort => $ARGV[0],
    Listen => 8, ReuseAddr => 1) or die "fake provider: $!\n";
$| = 1;
print "listening\n";
my $answer = '{"success":true,"errors":[],"messages":[],"result":{"delivered":[],"permanent_bounces":[],"queued":[]},"MessageId":"fake"}';
while (my $client = $server->accept) {
    my $head = "";
    while (my $line = <$client>) { $head .= $line; last if $line eq "\r\n"; }
    my ($length) = $head =~ /^content-length:\s*(\d+)/mi;
    my $body = "";
    read($client, $body, $length // 0);
    open(my $file, ">>", "/tmp/requests") or die "fake provider: $!\n";
    print $file $head, $body, "\n=====\n";
    close $file;
    print $client "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: "
        . length($answer) . "\r\nConnection: close\r\n\r\n" . $answer;
    close $client;
}
"#;

/// The providers' keys of one trial: made up, apart from OCI's test key.
pub(super) struct Keys {
    cloudflare_account: String,
    cloudflare_token: String,
    ses_key_id: String,
    ses_secret: String,
    oci_fingerprint: String,
    oci_key: String,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Keys").finish_non_exhaustive()
    }
}

/// A way of sending of an API, in the order the trial tries them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Cloudflare,
    Ses,
    Oci,
}

const OCI_TENANCY: &str = "ocid1.tenancy.oc1..smoke";
const OCI_USER: &str = "ocid1.user.oc1..smoke";

impl Keys {
    /// New keys, and OCI's test key from the checkout at `root`.
    pub(super) fn new(root: &Path) -> Result<Self> {
        let oci_key = std::fs::read_to_string(root.join(OCI_KEY))
            .with_context(|| format!("reading {OCI_KEY}"))?;
        let fingerprint: Vec<String> = super::random::<16>()?
            .iter()
            .map(|byte| super::hex(&[*byte]))
            .collect();
        Ok(Self {
            cloudflare_account: super::hex(&super::random::<16>()?),
            cloudflare_token: format!("fake-cloudflare-{}", super::hex(&super::random::<20>()?)),
            ses_key_id: format!("AKIA{}", super::hex(&super::random::<8>()?).to_uppercase()),
            ses_secret: super::hex(&super::random::<20>()?),
            oci_fingerprint: fingerprint.join(":"),
            oci_key,
        })
    }

    #[cfg(test)]
    pub(super) fn for_tests() -> Self {
        Self {
            cloudflare_account: "0".repeat(32),
            cloudflare_token: "a-secret-value".to_owned(),
            ses_key_id: "AKIA0".to_owned(),
            ses_secret: "a-secret-value".to_owned(),
            oci_fingerprint: String::new(),
            oci_key: "a-secret-value".to_owned(),
        }
    }

    /// The lines of the trial's `.env`: mail by Cloudflare to the fake provider, the
    /// other providers' lines empty.
    pub(super) fn env_lines(&self) -> String {
        self.env_lines_of(Provider::Cloudflare)
    }

    /// The mail lines of `.env` for `provider`, written as `.env.example` says: OCI's key
    /// in double quotes, its line ends as `\n`.
    fn env_lines_of(&self, provider: Provider) -> String {
        let settings = self.settings(provider).into_iter().map(|(name, value)| {
            if value.contains('\n') {
                format!("{name}=\"{}\"\n", value.replace('\n', "\\n"))
            } else {
                format!("{name}={value}\n")
            }
        });
        [
            "FVOCI_MAIL_FROM=fvoci@example.com\n".to_owned(),
            format!("FVOCI_MAIL_API_ENDPOINT=http://127.0.0.1:{FAKE_PORT}\n"),
        ]
        .into_iter()
        .chain(settings)
        .collect()
    }

    /// What must be found nowhere: the secrets, and a line of OCI's key, which `docker
    /// inspect` would show with its line ends escaped.
    pub(super) fn secrets(&self) -> Vec<String> {
        let key_line = self
            .oci_key
            .lines()
            .nth(1)
            .unwrap_or(&self.oci_key)
            .to_owned();
        vec![
            self.cloudflare_token.clone(),
            self.ses_secret.clone(),
            key_line,
        ]
    }

    /// Every provider's variable, those of `provider` set and the others empty.
    fn settings(&self, provider: Provider) -> Vec<(&'static str, String)> {
        let of = |wanted: Provider, value: &str| {
            if provider == wanted {
                value.to_owned()
            } else {
                String::new()
            }
        };
        let transport = match provider {
            Provider::Cloudflare => "cloudflare",
            Provider::Ses => "ses",
            Provider::Oci => "oci",
        };
        vec![
            ("FVOCI_MAIL_TRANSPORT", transport.to_owned()),
            (
                "FVOCI_CLOUDFLARE_ACCOUNT_ID",
                of(Provider::Cloudflare, &self.cloudflare_account),
            ),
            (
                "FVOCI_CLOUDFLARE_API_TOKEN",
                of(Provider::Cloudflare, &self.cloudflare_token),
            ),
            ("FVOCI_SES_REGION", of(Provider::Ses, "us-east-1")),
            (
                "FVOCI_SES_ACCESS_KEY_ID",
                of(Provider::Ses, &self.ses_key_id),
            ),
            (
                "FVOCI_SES_SECRET_ACCESS_KEY",
                of(Provider::Ses, &self.ses_secret),
            ),
            ("FVOCI_OCI_REGION", of(Provider::Oci, "ap-seoul-1")),
            ("FVOCI_OCI_TENANCY", of(Provider::Oci, OCI_TENANCY)),
            ("FVOCI_OCI_USER", of(Provider::Oci, OCI_USER)),
            (
                "FVOCI_OCI_FINGERPRINT",
                of(Provider::Oci, &self.oci_fingerprint),
            ),
            (
                "FVOCI_OCI_COMPARTMENT",
                of(Provider::Oci, "ocid1.compartment.oc1..smoke"),
            ),
            ("FVOCI_OCI_PRIVATE_KEY", of(Provider::Oci, &self.oci_key)),
        ]
    }

    /// The path a message to `provider` is posted to, and what its `Authorization`
    /// must hold: the keys it was given, as that provider reads them.
    fn expected(&self, provider: Provider) -> (String, String) {
        match provider {
            Provider::Cloudflare => (
                format!(
                    "/client/v4/accounts/{}/email/sending/send",
                    self.cloudflare_account
                ),
                format!("Bearer {}", self.cloudflare_token),
            ),
            Provider::Ses => (
                "/v2/email/outbound-emails".to_owned(),
                format!("Credential={}/", self.ses_key_id),
            ),
            Provider::Oci => (
                "/20220926/actions/submitEmail".to_owned(),
                format!(
                    "keyId=\"{OCI_TENANCY}/{OCI_USER}/{}\"",
                    self.oci_fingerprint
                ),
            ),
        }
    }
}

/// Refuses fvoci without mail and `email_verification` on, then mails a reset link by
/// each provider.
///
/// # Errors
///
/// Fails naming the provider whose message did not arrive as it should.
pub(super) fn providers(trial: &Trial, directory: &Path) -> Result<()> {
    refuses_verification_without_mail(trial)?;
    let client = Client::new(&trial.base, directory)?;
    for provider in [Provider::Cloudflare, Provider::Ses, Provider::Oci] {
        if provider != Provider::Cloudflare {
            recreate_fvoci(trial, provider)?;
        }
        let fake = Fake::start(trial)?;
        let answer = client.send(
            "POST",
            "/api/v1/password-reset/request",
            Some(&json!({"email": RECIPIENT, "language": "en"})),
        )?;
        ensure!(
            answer.status == 204,
            "{provider:?}: the reset was answered {}: {}",
            answer.status,
            answer.body
        );
        let request = fake.request()?;
        check_request(&trial.mail, provider, &request)?;
        drop(fake);
        holds_its_secret_alone(trial, provider)?;
        say(&format!(
            "    {provider:?}: the reset link reached the fake provider with its keys"
        ));
    }
    Ok(())
}

/// Runs fvoci once with `email_verification` on and nothing to send mail by.
fn refuses_verification_without_mail(trial: &Trial) -> Result<()> {
    let mut command = trial.command(&["run", "--rm", "--no-deps", "fvoci"]);
    for (name, _) in trial.mail.settings(Provider::Cloudflare) {
        command.env(name, "");
    }
    let output = command
        .envs([
            ("FVOCI_EMAIL_VERIFICATION", "true"),
            ("FVOCI_MAIL_FROM", ""),
            ("FVOCI_MAIL_API_ENDPOINT", ""),
        ])
        .output()
        .context("starting docker compose run")?;
    let said = String::from_utf8_lossy(&output.stderr) + String::from_utf8_lossy(&output.stdout);
    ensure!(
        !output.status.success() && said.contains(NO_MAIL),
        "fvoci with email_verification and no mail did not refuse to start ({}): {}",
        output.status,
        said.trim()
    );
    say("    with email_verification and no way of sending, fvoci does not start, saying so");
    Ok(())
}

/// Makes fvoci anew with `provider`'s settings in the trial's `.env`, and waits for it
/// to be healthy.
fn recreate_fvoci(trial: &Trial, provider: Provider) -> Result<()> {
    let lines = trial.mail.env_lines_of(provider);
    let names: Vec<&str> = lines
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name))
        .collect();
    let text = std::fs::read_to_string(&trial.env_file).context("reading the trial's .env")?;
    let mut kept: Vec<&str> = text
        .lines()
        .filter(|line| {
            line.split_once('=')
                .is_none_or(|(name, _)| !names.contains(&name))
        })
        .collect();
    kept.push(&lines);
    let kept = kept.join("\n");
    super::write_private(&trial.env_file, &kept)?;
    let output = trial
        .command(&[
            "up",
            "--detach",
            "--no-build",
            "--no-deps",
            "--force-recreate",
            "--wait",
            "--wait-timeout",
            super::WAIT_SECONDS,
            "fvoci",
        ])
        .output()
        .context("starting docker compose up")?;
    if !output.status.success() {
        let logs = trial.compose(&["logs", "--no-color", "--tail", "30", "fvoci"])?;
        bail!(
            "fvoci did not become healthy with {provider:?}'s settings: {}\n{}",
            String::from_utf8_lossy(&output.stderr).trim(),
            logs.output
        );
    }
    Ok(())
}

/// Where each provider's secret reaches fvoci: the variable naming its file.
fn secret_file_variable(provider: Provider) -> &'static str {
    match provider {
        Provider::Cloudflare => "FVOCI_CLOUDFLARE_API_TOKEN_FILE",
        Provider::Ses => "FVOCI_SES_SECRET_ACCESS_KEY_FILE",
        Provider::Oci => "FVOCI_OCI_PRIVATE_KEY_FILE",
    }
}

/// Checks that fvoci, made with `provider`'s settings, holds that provider's secret in
/// the file its variable names, and shows no secret of the trial in its description or
/// its log: each provider's own container, before the next one replaces it.
fn holds_its_secret_alone(trial: &Trial, provider: Provider) -> Result<()> {
    let id = trial.compose(&["ps", "--quiet", "fvoci"])?;
    ensure!(id.succeeded && !id.output.is_empty(), "no running fvoci");
    let inspect = capture("docker", &["inspect", &id.output])?;
    let logs = trial.compose(&["logs", "--no-color", "fvoci"])?;
    ensure!(
        inspect.succeeded && logs.succeeded,
        "fvoci cannot be inspected"
    );
    for (place, text) in [
        ("description", &inspect.output),
        ("log", &logs.output),
        ("log", &logs.error),
    ] {
        ensure!(
            !trial
                .secrets
                .iter()
                .any(|secret| text.contains(secret.as_str())),
            "{provider:?}: a secret is in fvoci's {place}"
        );
    }
    let description: serde_json::Value =
        serde_json::from_str(&inspect.output).context("docker inspect gave no JSON")?;
    let variable = format!("{}=", secret_file_variable(provider));
    let file = description[0]["Config"]["Env"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .find_map(|entry| entry.strip_prefix(&variable))
        .with_context(|| format!("{provider:?}: fvoci has no {variable}"))?;
    let held = trial.compose(&["exec", "-T", "fvoci", "cat", file])?;
    let expected = match provider {
        Provider::Cloudflare => &trial.mail.cloudflare_token,
        Provider::Ses => &trial.mail.ses_secret,
        Provider::Oci => &trial.mail.oci_key,
    };
    ensure!(
        held.succeeded && held.output.trim_end() == expected.trim_end(),
        "{provider:?}: {variable} does not name a file holding the secret given"
    );
    Ok(())
}

/// The label of the fake provider's container, valued with the trial's project.
const FAKE_LABEL: &str = "fvoci-smoke-fake";

/// The fake provider's container, removed when this is dropped.
struct Fake<'a> {
    trial: &'a Trial,
    name: String,
}

impl<'a> Fake<'a> {
    /// Starts the fake provider in fvoci's network namespace, as nobody, and waits
    /// until it listens.
    fn start(trial: &'a Trial) -> Result<Self> {
        let fvoci = trial.compose(&["ps", "--quiet", "fvoci"])?;
        ensure!(
            fvoci.succeeded && !fvoci.output.is_empty(),
            "no running fvoci"
        );
        let fake = Fake {
            trial,
            name: fake_name(trial),
        };
        let network = format!("container:{}", fvoci.output);
        let port = FAKE_PORT.to_string();
        let label = format!("{FAKE_LABEL}={}", trial.project);
        let started = capture(
            "docker",
            &[
                "run",
                "--detach",
                "--rm",
                "--name",
                &fake.name,
                "--network",
                &network,
                "--label",
                &label,
                "--user",
                "65534:65534",
                // Over the image's volume, which `docker rm` would otherwise leave as an
                // anonymous volume of no project.
                "--mount",
                "type=tmpfs,destination=/var/lib/postgresql",
                "--entrypoint",
                "perl",
                "postgres:18",
                "-e",
                FAKE,
                &port,
            ],
        )?;
        ensure!(
            started.succeeded,
            "the fake provider did not start: {}",
            started.error
        );
        let mounts = capture(
            "docker",
            &[
                "inspect",
                "--format",
                "{{range .Mounts}}{{.Type}} {{end}}",
                &fake.name,
            ],
        )?;
        ensure!(
            mounts.succeeded,
            "the fake provider's mounts could not be read: {}",
            mounts.error
        );
        ensure!(
            !mounts.output.contains("volume"),
            "the fake provider has a volume, which its removal would leave: {}",
            mounts.output
        );
        let since = Instant::now();
        loop {
            let logs = capture("docker", &["logs", &fake.name])?;
            if logs.output.contains("listening") {
                return Ok(fake);
            }
            if since.elapsed() > Duration::from_secs(30) || interrupted() {
                bail!("the fake provider does not listen: {}", logs.error);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// The first request the fake provider was sent, once there is one.
    fn request(&self) -> Result<String> {
        let since = Instant::now();
        loop {
            let requests = capture("docker", &["exec", &self.name, "cat", "/tmp/requests"])?;
            if requests.succeeded
                && let Some((first, _)) = requests.output.split_once("\n=====")
            {
                return Ok(first.to_owned());
            }
            if since.elapsed() > ARRIVE_WITHIN || interrupted() {
                bail!("no message reached the fake provider within {ARRIVE_WITHIN:?}");
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

impl Drop for Fake<'_> {
    fn drop(&mut self) {
        // A container that stays is found by the take-down's look for leftovers.
        let _ = remove_fake(self.trial);
    }
}

fn fake_name(trial: &Trial) -> String {
    format!("{}-fake-mail", trial.project)
}

/// Removes the fake provider's container, if there is one: before fvoci's own goes.
///
/// # Errors
///
/// Fails if a container of the trial's fake provider is left.
pub(super) fn remove_fake(trial: &Trial) -> Result<()> {
    let _ = capture("docker", &["rm", "--force", "--volumes", &fake_name(trial)]);
    let label = format!("label={FAKE_LABEL}={}", trial.project);
    let left = capture(
        "docker",
        &["container", "ls", "--all", "--quiet", "--filter", &label],
    )?;
    ensure!(
        left.succeeded && left.output.is_empty(),
        "the fake mail provider's container is left: {} {}",
        left.output,
        left.error
    );
    Ok(())
}

/// Checks that `request` is a message to the recipient for `provider`, with its keys.
/// What fails is said without the request, which holds a secret and a reset link.
fn check_request(keys: &Keys, provider: Provider, request: &str) -> Result<()> {
    let (head, body) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
    let (path, authorization) = keys.expected(provider);
    let line = head.lines().next().unwrap_or_default();
    ensure!(
        line == format!("POST {path} HTTP/1.1"),
        "{provider:?}: the message was not posted to {path}"
    );
    let given = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.trim())
        .unwrap_or_default();
    ensure!(
        given.contains(&authorization),
        "{provider:?}: the message's Authorization does not carry the keys given"
    );
    ensure!(
        body.contains(RECIPIENT),
        "{provider:?}: the message is not to {RECIPIENT}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_is_given_its_keys_and_the_others_nothing() {
        let keys = Keys::for_tests();
        let ses = keys.settings(Provider::Ses);
        let value = |name: &str| {
            ses.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(value("FVOCI_MAIL_TRANSPORT"), Some("ses"));
        assert_eq!(value("FVOCI_SES_SECRET_ACCESS_KEY"), Some("a-secret-value"));
        assert_eq!(value("FVOCI_CLOUDFLARE_API_TOKEN"), Some(""));
        assert_eq!(value("FVOCI_OCI_PRIVATE_KEY"), Some(""));
    }

    #[test]
    fn a_key_of_several_lines_is_one_quoted_line_of_the_env_file() {
        let keys = Keys {
            oci_key: "-----BEGIN-----\nAB\n-----END-----\n".to_owned(),
            ..Keys::for_tests()
        };
        let lines = keys.env_lines_of(Provider::Oci);
        assert!(
            lines
                .lines()
                .any(|line| line
                    == r#"FVOCI_OCI_PRIVATE_KEY="-----BEGIN-----\nAB\n-----END-----\n""#),
            "{lines}"
        );
        assert!(lines.contains("FVOCI_CLOUDFLARE_API_TOKEN=\n"), "{lines}");
    }

    #[test]
    fn a_request_is_checked_by_its_path_authorization_and_recipient() {
        let keys = Keys::for_tests();
        let (path, _) = keys.expected(Provider::Cloudflare);
        let good = format!(
            "POST {path} HTTP/1.1\r\nauthorization: Bearer a-secret-value\r\n\r\n\
             {{\"to\":\"{RECIPIENT}\"}}"
        );
        assert!(check_request(&keys, Provider::Cloudflare, &good).is_ok());
        let forged = good.replace("a-secret-value", "another");
        let error = check_request(&keys, Provider::Cloudflare, &forged)
            .expect_err("another token")
            .to_string();
        assert!(!error.contains("a-secret-value"), "{error}");
        assert!(
            check_request(&keys, Provider::Ses, &good).is_err(),
            "another path"
        );
    }
}
