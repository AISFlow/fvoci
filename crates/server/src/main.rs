//! The `fvoci-server` executable.

#[cfg(all(not(debug_assertions), feature = "test-util"))]
compile_error!("`fvoci-server` must not be built in release mode with `test-util` enabled");

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser;
use fvoci_server::config::Configuration;
use fvoci_server::{BASE_PATH_ENV, Server, checked_base_path};
use tracing_subscriber::EnvFilter;

/// The fvoci board server.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Location of the JSON config file.
    #[arg(long, default_value = "./config.json")]
    config: PathBuf,
    /// The port number; overrides the config file.
    #[arg(long)]
    port: Option<u16>,
    /// The database kind (sqlite or postgres); overrides `database_kind`.
    #[arg(long)]
    database_kind: Option<String>,
    /// The database connection; overrides `database_connection`.
    #[arg(long)]
    database_connection: Option<String>,
    /// Print the contract of the API, an OpenAPI document, and exit. Nothing is
    /// configured, opened or served.
    #[arg(long)]
    print_openapi: bool,
    /// Ask the server this configuration describes, on this machine, whether it is
    /// ready (`GET /api/v1/ready`), and exit with success only if it is: the health
    /// check of a container, whose image has no HTTP client. Nothing is opened or served.
    #[arg(long)]
    check_ready: bool,
    #[command(flatten)]
    migration: Migration,
}

/// The migration of the database's schema, done instead of serving.
#[derive(Debug, clap::Args)]
struct Migration {
    /// Bring the database's schema up to date and exit, holding the lock of the server
    /// meanwhile: for the role that owns the tables (the configuration's
    /// `database_connection`), when the server connects as one that does not. Nothing
    /// is served.
    #[arg(long, conflicts_with = "check_ready")]
    migrate: bool,
    /// With `--migrate`: say what the migration would do and change nothing, failing if
    /// it would be refused. It takes no lock: it runs while a server serves the database.
    #[arg(long, requires = "migrate")]
    check: bool,
}

/// What `--migrate` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Migrating {
    /// The schema brought up to date.
    Apply,
    /// What the migration would do, said, and nothing changed (`--check`).
    Check,
}

impl Migration {
    /// What is asked for; `None` without `--migrate`.
    fn asked(&self) -> Option<Migrating> {
        match (self.migrate, self.check) {
            (false, _) => None,
            (true, false) => Some(Migrating::Apply),
            (true, true) => Some(Migrating::Check),
        }
    }
}

/// How long the check of readiness waits for the answer: the server answers `ready`
/// within two and a half seconds.
const READY_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Where the server this configuration describes answers whether it is ready, on this
/// machine: the port, plain HTTP or TLS, and the base path (as the server checks it).
fn ready_url(config: &Configuration, base_path: &str) -> String {
    let scheme = if config.serve_https { "https" } else { "http" };
    format!(
        "{scheme}://localhost:{}{base_path}{}/ready",
        config.port,
        fvoci_server::api::PREFIX
    )
}

/// Asks the server on this machine whether it is ready, and succeeds only on 200.
/// Nothing is sent but the request. The certificate of a server that serves TLS itself
/// is for its public name, not `localhost`, so it is not verified here.
async fn check_ready(config: &Configuration, base_path: &str) -> anyhow::Result<()> {
    let url = ready_url(config, base_path);
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .tls_danger_accept_invalid_certs(config.serve_https)
        .timeout(READY_CHECK_TIMEOUT)
        .build()
        .context("unable to set up the check")?;
    let status = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("asking {url}"))?
        .status();
    anyhow::ensure!(status == reqwest::StatusCode::OK, "{url} answered {status}");
    Ok(())
}

/// Writes the contract of the API to the standard output: what `contracts/openapi.json`
/// is to be.
fn print_openapi() -> anyhow::Result<()> {
    let document = fvoci_server::api::contract::document()?;
    std::io::stdout()
        .write_all(document.as_bytes())
        .context("unable to write the contract")
}

fn init_logging() {
    let filter =
        EnvFilter::try_from_env("FVOCI_LOG_LEVEL").unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if std::env::var("FVOCI_LOG_FORMAT").is_ok_and(|format| format == "json") {
        builder.json().init();
    } else {
        builder.init();
    }
}

fn main() -> anyhow::Result<()> {
    // The worker of editing together is this executable too, started by the server
    // with one argument: it reads no configuration, starts no runtime and logs nothing.
    if let Some(ended) = fvoci_server::collab::run_as_worker() {
        return ended;
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("unable to start the runtime")?
        .block_on(serve())
}

async fn serve() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if cli.print_openapi {
        return print_openapi();
    }
    init_logging();

    let mut config = Configuration::read(&cli.config).context("unable to read the config file")?;
    if let Some(kind) = cli.database_kind.filter(|kind| !kind.is_empty()) {
        tracing::info!(kind, "the database kind is given on the command line");
        config.database_kind = kind;
    }
    if let Some(connection) = cli
        .database_connection
        .filter(|connection| !connection.is_empty())
    {
        // Not echoed: the connection may hold a password.
        tracing::info!("the database connection is given on the command line");
        config.database_connection = connection;
    }
    if let Some(port) = cli.port.filter(|port| *port > 0) {
        tracing::info!(port, "the port is given on the command line");
        config.port = port;
    }

    let base_path = std::env::var(BASE_PATH_ENV).unwrap_or_default();
    // A server that starts says it with the other warnings of its configuration.
    let migrating = cli.migration.asked();
    if (migrating.is_some() || cli.check_ready)
        && let Some(warning) = config.unread_keys_warning()
    {
        tracing::warn!("{warning}");
    }
    match migrating {
        Some(Migrating::Check) => return fvoci_server::app::check_database(&config).await,
        Some(Migrating::Apply) => return fvoci_server::app::migrate_database(&config).await,
        None => {}
    }
    if cli.check_ready {
        return check_ready(&config, checked_base_path(&base_path)?).await;
    }

    let port = config.port;
    let loopback_only = config.listen_loopback_only;
    let server = Server::new(config, &base_path).await?;

    let listener = if loopback_only {
        tokio::net::TcpListener::bind(("localhost", port))
            .await
            .with_context(|| format!("listening on localhost:{port}"))?
    } else {
        // All interfaces, IPv6 and IPv4 where the system maps one onto the other.
        match tokio::net::TcpListener::bind(("::", port)).await {
            Ok(listener) => listener,
            Err(on_ipv6) => tokio::net::TcpListener::bind(("0.0.0.0", port))
                .await
                .with_context(|| {
                    format!("listening on [::]:{port} ({on_ipv6}) and then on 0.0.0.0:{port}")
                })?,
        }
    };

    server.serve(listener, shutdown_signal()).await
}

/// Completes when the process is asked to stop: on Ctrl-C (SIGINT) or on SIGTERM
/// (sent by service managers and container runtimes).
async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => tokio::select! {
                () = interrupt => {}
                _ = terminate.recv() => {}
            },
            Err(error) => {
                tracing::warn!(%error, "unable to listen for the termination signal");
                interrupt.await;
            }
        }
    }
    #[cfg(not(unix))]
    interrupt.await;
    tracing::info!("shutting down");

    // The shutdown waits for requests and notifications under way. A second interrupt
    // ends the process at once.
    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::warn!("interrupted again; exiting without waiting");
            std::process::exit(130);
        }
    });
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    /// A server on this machine that answers every request with `status`.
    async fn answering(status: &'static str) -> u16 {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("a local port");
        let port = listener.local_addr().expect("its address").port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = [0; 1024];
                let _ = stream.read(&mut request).await;
                let answer =
                    format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
                let _ = stream.write_all(answer.as_bytes()).await;
            }
        });
        port
    }

    #[tokio::test]
    async fn the_check_succeeds_on_200_only() {
        for (status, ready) in [
            ("200 OK", true),
            ("503 Service Unavailable", false),
            ("204 No Content", false),
        ] {
            let config = Configuration {
                port: answering(status).await,
                ..Configuration::default()
            };
            assert_eq!(check_ready(&config, "").await.is_ok(), ready, "{status}");
        }
    }

    #[test]
    fn the_check_asks_where_the_server_listens() {
        let plain = Configuration {
            port: 8000,
            ..Configuration::default()
        };
        let tls = Configuration {
            port: 8443,
            serve_https: true,
            ..Configuration::default()
        };
        let base = |raw| checked_base_path(raw).expect("a base path the server takes");
        assert_eq!(
            ready_url(&plain, base("")),
            "http://localhost:8000/api/v1/ready"
        );
        assert_eq!(
            ready_url(&plain, base("/boards/")),
            "http://localhost:8000/boards/api/v1/ready"
        );
        assert_eq!(
            ready_url(&tls, base("/")),
            "https://localhost:8443/api/v1/ready"
        );
    }
}
