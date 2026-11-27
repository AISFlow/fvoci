//! The administration socket needs Unix sockets, which this platform does not have.

use std::path::Path;

use anyhow::bail;
use axum::Router;

/// Never exists on this platform.
#[derive(Debug)]
pub(crate) struct AdminSocket;

impl AdminSocket {
    pub(crate) fn start(_path: &Path, _router: Router) -> anyhow::Result<Self> {
        bail!("serve_admin_socket needs Unix sockets, which this platform does not have")
    }

    pub(crate) async fn stop(self) {}
}
