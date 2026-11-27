//! On-disk multipart upload part streaming, joining, and cleanup helpers.

use futures_util::{Stream, StreamExt as _, TryStreamExt as _};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;
use tokio_util::io::{ReaderStream, StreamReader};

use super::{AppError, Bytes, PART_IDLE_TIMEOUT, PartDigest, Path, UploadPart, fs, io};

/// The parts in `directory` read one after the other, after checking that each is the
/// size it was received with.
pub(super) async fn parts_reader(
    parts: &[UploadPart],
    directory: &Path,
) -> Result<impl tokio::io::AsyncRead + Send + Unpin + use<>, AppError> {
    let mut paths = Vec::with_capacity(parts.len());
    for part in parts {
        let path = directory.join(&part.part_id);
        let on_disk = fs::metadata(&path)
            .await
            .map_err(|error| part_error(&error))?
            .len();
        if Some(on_disk) != u64::try_from(part.size).ok() {
            return Err(AppError::internal(format_args!(
                "part {} of an upload has {on_disk} bytes on the disk, not {}",
                part.number, part.size
            )));
        }
        paths.push(path);
    }
    let bytes = futures_util::stream::iter(paths)
        .then(|path| async move { fs::File::open(path).await })
        .map_ok(ReaderStream::new)
        .try_flatten();
    Ok(StreamReader::new(Box::pin(bytes)))
}

/// Joins the parts in `directory` into the file `to`, in order, removing each once it is
/// in: the disk holds the archive and one part at most, not the archive twice.
pub(super) async fn join_parts(
    parts: &[UploadPart],
    directory: &Path,
    to: &Path,
) -> Result<(), AppError> {
    let mut file = fs::File::create(to)
        .await
        .map_err(|error| part_error(&error))?;
    for part in parts {
        let path = directory.join(&part.part_id);
        let mut reader = fs::File::open(&path)
            .await
            .map_err(|error| part_error(&error))?;
        let copied = tokio::io::copy(&mut reader, &mut file)
            .await
            .map_err(|error| part_error(&error))?;
        if Some(copied) != u64::try_from(part.size).ok() {
            return Err(AppError::internal(format_args!(
                "part {} of an upload has {copied} bytes on the disk, not {}",
                part.number, part.size
            )));
        }
        drop(reader);
        remove_quietly(&path).await;
    }
    file.flush().await.map_err(|error| part_error(&error))?;
    Ok(())
}

/// Writes the body to `path`, checking its length and digest as it goes.
pub(super) async fn receive_part<E>(
    path: &Path,
    size: u64,
    digest: &PartDigest,
    mut body: impl Stream<Item = Result<Bytes, E>> + Unpin,
) -> Result<(), AppError>
where
    E: std::fmt::Display,
{
    let mut file = fs::File::create(path).await.map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            closed()
        } else {
            part_error(&error)
        }
    })?;
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    loop {
        let next = tokio::time::timeout(PART_IDLE_TIMEOUT, body.next())
            .await
            .map_err(|_| AppError::bad_request("the part timed out waiting for data"))?;
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.map_err(|error| {
            AppError::bad_request(format_args!("the part could not be read: {error}"))
        })?;
        received = received.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        if received > size {
            return Err(AppError::bad_request(format_args!(
                "the part is longer than its {size} bytes"
            )));
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| part_error(&error))?;
    }
    if received != size {
        return Err(AppError::bad_request(format_args!(
            "the part has {received} of its {size} bytes"
        )));
    }
    if hasher.finalize().as_slice() != digest {
        return Err(AppError::bad_request(
            "the part's SHA-256 digest is not the one of the Content-Digest header",
        )
        .with_header("Content-Digest"));
    }
    file.flush().await.map_err(|error| part_error(&error))?;
    file.sync_all().await.map_err(|error| part_error(&error))?;
    Ok(())
}

/// The refusal of a part or a completion for a session that is closed.
pub(super) fn closed() -> AppError {
    AppError::conflict("the upload is being completed or thrown away")
}

/// A part that cannot be written: the server's failure.
pub(super) fn part_error(error: &io::Error) -> AppError {
    AppError::internal(format_args!("the part could not be stored: {error}"))
}

/// Removes a directory of parts that is not to be kept, if it is there. Returns false
/// if it is there still, which is logged.
pub(super) async fn remove_dir_quietly(directory: &Path) -> bool {
    match fs::remove_dir_all(directory).await {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(error) => {
            tracing::error!(%error, "the parts of an upload could not be removed");
            false
        }
    }
}

/// Removes a file that is not to be kept, if it is there.
pub(super) async fn remove_quietly(path: &Path) {
    match fs::remove_file(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(%error, "a part that is not kept could not be removed"),
    }
}
