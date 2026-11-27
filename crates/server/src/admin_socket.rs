//! The administration socket: `serve_admin_socket`.

use std::fs::{DirBuilder, Permissions};
use std::io;
use std::os::unix::fs::{
    DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, PermissionsExt as _,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, bail};
use axum::Router;
use axum::extract::connect_info::Connected;
use axum::serve::{IncomingStream, Listener};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::auth::secret::new_secret;
use crate::connection::Connection;
use crate::request_log::Peer;

/// The mode of the socket file: only the user the server runs as may connect.
const SOCKET_MODE: u32 = 0o600;

/// The mode of the temporary staging directory in which the socket is bound before it is
/// moved into place (`H-2`).
const STAGING_DIR_MODE: u32 = 0o700;

/// How long stopping waits for requests in flight before it cuts them off.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Wraps [`UnixListener`] to verify `SO_PEERCRED` on every accepted stream (`H-2`),
/// admitting only connections from the server's effective UID or `root` (`uid == 0`).
#[derive(Debug)]
struct VerifiedUnixListener {
    inner: UnixListener,
    allowed_uid: u32,
}

impl Listener for VerifiedUnixListener {
    type Io = UnixStream;
    type Addr = tokio::net::unix::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.inner.accept().await {
                Ok((stream, addr)) => match stream.peer_cred() {
                    Ok(cred) if cred.uid() == self.allowed_uid || cred.uid() == 0 => {
                        return (stream, addr);
                    }
                    Ok(cred) => {
                        tracing::warn!(
                            peer_uid = cred.uid(),
                            expected_uid = self.allowed_uid,
                            "refused connection on administration unix socket from unauthorized uid"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            "refused connection on administration unix socket without peer credentials"
                        );
                    }
                },
                Err(error) => {
                    tracing::warn!(%error, "error accepting connection on administration unix socket");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

impl Connected<IncomingStream<'_, VerifiedUnixListener>> for Connection {
    fn connect_info(_stream: IncomingStream<'_, VerifiedUnixListener>) -> Self {
        Self {
            peer: Peer::Unix,
            breaker: None,
        }
    }
}

/// Guard that removes the temporary `0700` staging directory on drop.
struct StagingDir(PathBuf);

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Binds a `0600` [`UnixListener`] inside a private `0700` sibling directory and renames
/// the socket atomically onto `path` so no world-writable window ever exists (`H-2`).
fn bind_private_socket(path: &Path) -> anyhow::Result<(UnixListener, u32)> {
    remove_stale_socket(path)?;
    let parent = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let short_tag = &new_secret()[..10];
    let staging_path = parent.join(format!(".fvs-{short_tag}"));
    DirBuilder::new()
        .mode(STAGING_DIR_MODE)
        .create(&staging_path)
        .context("creating private staging directory for the administration socket")?;
    let _staging = StagingDir(staging_path.clone());
    std::fs::set_permissions(&staging_path, Permissions::from_mode(STAGING_DIR_MODE))
        .context("restricting the staging directory of the administration socket")?;
    let owner_uid = std::fs::symlink_metadata(&staging_path)
        .context("reading the owner of the staging directory")?
        .uid();

    let staged_socket = staging_path.join("s");
    let listener = UnixListener::bind(&staged_socket).context("binding the socket")?;
    std::fs::set_permissions(&staged_socket, Permissions::from_mode(SOCKET_MODE))
        .context("restricting the socket to the server's user")?;
    remove_stale_socket(path)?;
    std::fs::rename(&staged_socket, path).context("moving the administration socket into place")?;
    Ok((listener, owner_uid))
}

/// The Unix socket that serves the administration API, and the task that serves it.
///
/// The socket file is removed when this is stopped or dropped.
#[derive(Debug)]
pub(crate) struct AdminSocket {
    path: PathBuf,
    stop: CancellationToken,
    serving: JoinHandle<()>,
}

impl AdminSocket {
    /// Creates the socket at `path`, replacing a socket file a previous run left
    /// behind, and starts serving `router` on it.
    pub(crate) fn start(path: &Path, router: Router) -> anyhow::Result<Self> {
        let (listener, allowed_uid) = bind_private_socket(path)?;
        let verified = VerifiedUnixListener {
            inner: listener,
            allowed_uid,
        };

        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let serving = tokio::spawn(async move {
            tracing::info!("starting unix socket server");
            let service = router.into_make_service_with_connect_info::<Connection>();
            let served = axum::serve(verified, service)
                .with_graceful_shutdown(async move { stopped.cancelled().await })
                .await;
            if let Err(error) = served {
                tracing::error!(%error, "error serving the unix socket");
            }
        });
        Ok(Self {
            path: path.to_owned(),
            stop,
            serving,
        })
    }

    /// Stops serving and removes the socket file.
    pub(crate) async fn stop(mut self) {
        self.stop.cancel();
        if tokio::time::timeout(STOP_TIMEOUT, &mut self.serving)
            .await
            .is_err()
        {
            tracing::warn!("requests on the unix socket did not finish before shutdown");
        }
        // Dropping removes the file.
    }
}

impl Drop for AdminSocket {
    fn drop(&mut self) {
        self.stop.cancel();
        self.serving.abort();
        if let Err(error) = std::fs::remove_file(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(%error, path = %self.path.display(), "unable to remove the unix socket");
        }
    }
}

/// Removes the socket file of a server that did not get to remove it, such as one
/// that was killed.
///
/// Only a Unix socket is removed here, so a mistyped path cannot delete a regular file.
fn remove_stale_socket(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            std::fs::remove_file(path).context("removing the socket file of an earlier run")
        }
        Ok(_) => bail!("the path exists and is not a socket; it is left alone"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("looking at the socket path"),
    }
}

#[cfg(test)]
mod tests {
    use fvoci_model::new_id;

    use super::*;

    /// A path short enough for a socket address, which the build directory is not.
    fn socket_path() -> PathBuf {
        std::env::temp_dir().join(format!("fvoci-{}.socket", new_id()))
    }

    fn mode(path: &Path) -> u32 {
        let metadata = std::fs::symlink_metadata(path).expect("the path exists");
        metadata.permissions().mode() & 0o777
    }

    fn is_socket(path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket())
    }

    #[tokio::test]
    async fn socket_is_private_and_removed_when_stopped() {
        let path = socket_path();

        let socket = AdminSocket::start(&path, Router::new()).expect("the socket opens");

        assert!(is_socket(&path), "a socket file exists");
        assert_eq!(mode(&path), 0o600, "only the owner may connect");
        socket.stop().await;
        assert!(!path.exists(), "the socket file is removed");
    }

    #[tokio::test]
    async fn socket_is_removed_when_dropped() {
        let path = socket_path();
        let socket = AdminSocket::start(&path, Router::new()).expect("the socket opens");

        drop(socket);

        assert!(!path.exists(), "the socket file is removed");
    }

    #[tokio::test]
    async fn stale_socket_is_replaced() {
        let path = socket_path();
        // A listener that goes away without removing its file, as a killed server's.
        drop(std::os::unix::net::UnixListener::bind(&path).expect("a socket is bound"));
        assert!(is_socket(&path), "the stale socket file exists");

        let socket = AdminSocket::start(&path, Router::new()).expect("the socket opens");

        assert!(is_socket(&path), "a socket file exists");
        assert_eq!(mode(&path), 0o600, "only the owner may connect");
        tokio::net::UnixStream::connect(&path)
            .await
            .expect("the new socket accepts connections");
        socket.stop().await;
    }

    #[tokio::test]
    async fn other_file_at_the_socket_path_is_left_alone() {
        let path = socket_path();
        std::fs::write(&path, "precious").expect("a file is written");

        let error = AdminSocket::start(&path, Router::new())
            .expect_err("a file that is not a socket is not replaced");

        assert!(error.to_string().contains("not a socket"), "{error:#}");
        assert_eq!(
            std::fs::read_to_string(&path).expect("the file is still there"),
            "precious"
        );
        std::fs::remove_file(&path).expect("the file is removed");
    }

    #[tokio::test]
    async fn unauthorized_peer_uid_is_dropped_by_verified_listener() {
        use tokio::io::AsyncReadExt as _;

        let path = socket_path();
        let (listener, my_uid) = bind_private_socket(&path).expect("socket binds");
        // Set `allowed_uid` to a different non-zero UID; unless running as root (`0`),
        // our own connection must be rejected and closed immediately.
        if my_uid == 0 {
            let _ = std::fs::remove_file(&path);
            return;
        }
        let mut verified = VerifiedUnixListener {
            inner: listener,
            allowed_uid: my_uid.wrapping_add(1).max(1),
        };
        let accept_task = tokio::spawn(async move { verified.accept().await });
        let mut client = UnixStream::connect(&path)
            .await
            .expect("connects at kernel level");
        let mut buf = [0_u8; 8];
        let n = tokio::time::timeout(Duration::from_secs(2), client.read(&mut buf))
            .await
            .expect("unauthorized stream is closed promptly")
            .expect("EOF read");
        assert_eq!(n, 0, "unauthorized peer receives immediate EOF");
        accept_task.abort();
        let _ = std::fs::remove_file(&path);
    }
}
