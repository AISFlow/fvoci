//! Image previews (source `packages/jobs/src/thumbnail*.ts`).
//!
//! The source decodes untrusted images with a rebuilt magick-wasm in a child
//! process (`--internal-thumbnail`) under a timeout and an RSS ceiling and
//! stores a WebP no larger than 1600×1600. Here the same boundary is a hidden
//! mode of this binary, `--internal-image-preview`: the child sets its own
//! address-space and CPU rlimits before reading a byte, decodes with the
//! `image` crate under explicit pixel and allocation limits, and writes
//! `width`, `height` (u32 LE) and a lossless WebP to stdout. The parent bounds
//! input, output and wall time and kills the child on timeout or drop.
//!
//! Differences from the source: only the first frame of an animated GIF,
//! APNG or WebP is kept (the `image` WebP encoder is lossless, still-only),
//! so the frame limits reduce to the per-image pixel limit.

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Hidden argv[1] that turns this binary into the preview child.
pub const PREVIEW_HELPER_ARG: &str = "--internal-image-preview";

/// Source `PREVIEW_MIME`: the rebuilt codec set. Other `image/*` uploads
/// (HEIC, AVIF, SVG, …) get no preview.
pub const PREVIEW_MIME: [&str; 7] = [
    "image/png",
    "image/apng",
    "image/jpeg",
    "image/gif",
    "image/tiff",
    "image/bmp",
    "image/webp",
];

/// Source `previewEtag`: `strongEtag(key, bytes, width, height)`, the first
/// 16 hex digits of SHA-256 over the `|`-joined values, quoted.
pub fn preview_etag(key: &str, bytes: i64, width: i64, height: i64) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{key}|{bytes}|{width}|{height}").as_bytes());
    format!("\"{}\"", &hex::encode(digest)[..16])
}

pub fn preview_mime_supported(mime: &str) -> bool {
    PREVIEW_MIME.contains(&mime)
}

/// Longest side of a stored preview (source `MagickGeometry(1600, 1600)`,
/// shrink only).
pub const PREVIEW_MAX_SIDE: u32 = 1600;

#[derive(Debug, Clone, Copy)]
pub struct PreviewLimits {
    /// Source `inputPixels` / `animationTotalPixels` (64 MP).
    pub input_pixels: u64,
    /// Source `inputBytes` (256 MiB); the job also caps at the row's size.
    pub input_bytes: u64,
    /// Decoder allocation ceiling (source `ResourceLimits.memory` 512 MiB).
    pub max_alloc: u64,
    /// Child address-space ceiling (RLIMIT_AS).
    pub child_address_space: u64,
    /// Wall-clock watchdog (source `DEFAULT_WATCHDOG_MS`).
    pub timeout: Duration,
    /// Source `PREVIEW_IPC_BYTE_LIMIT` for the encoded output.
    pub output_bytes: u64,
}

impl Default for PreviewLimits {
    fn default() -> Self {
        Self {
            input_pixels: 64_000_000,
            input_bytes: 256 * 1024 * 1024,
            max_alloc: 512 * 1024 * 1024,
            child_address_space: 2 * 1024 * 1024 * 1024,
            timeout: Duration::from_secs(60),
            output_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewImage {
    pub width: u32,
    pub height: u32,
    pub webp: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewError {
    /// The input is not an acceptable image (format, corrupt data, limits).
    /// Source `UnrecoverableError`: no retry.
    Rejected(String),
    /// The child hit the watchdog or an OS ceiling and was killed.
    ResourceLimit(String),
    /// Spawning or talking to the child failed; retryable.
    Worker(String),
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(msg) => write!(f, "preview rejected: {msg}"),
            Self::ResourceLimit(msg) => write!(f, "preview resource limit: {msg}"),
            Self::Worker(msg) => write!(f, "preview worker failure: {msg}"),
        }
    }
}

fn accepted_format(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::Gif
            | ImageFormat::Tiff
            | ImageFormat::Bmp
            | ImageFormat::WebP
    )
}

/// Decodes `source` and renders the stored preview. Runs inside the child;
/// also used directly by unit tests. Every size check happens before the
/// pixel buffer is allocated.
pub fn render_preview(source: &[u8], limits: &PreviewLimits) -> Result<PreviewImage, String> {
    if source.len() as u64 > limits.input_bytes {
        return Err("preview input exceeds byte limit".into());
    }
    let reader = ImageReader::new(Cursor::new(source))
        .with_guessed_format()
        .map_err(|err| format!("format probe failed: {err}"))?;
    let format = reader
        .format()
        .ok_or_else(|| "preview input is not a supported format".to_string())?;
    if !accepted_format(format) {
        return Err("preview input is not a supported format".into());
    }
    let mut image_limits = image::Limits::default();
    image_limits.max_alloc = Some(limits.max_alloc);
    let side = u32::try_from(limits.input_pixels).unwrap_or(u32::MAX);
    image_limits.max_image_width = Some(side);
    image_limits.max_image_height = Some(side);
    let mut decoder = reader
        .into_decoder()
        .map_err(|err| format!("decoder init failed: {err}"))?;
    decoder
        .set_limits(image_limits)
        .map_err(|err| format!("decoder limits refused: {err}"))?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err("preview input has no pixels".into());
    }
    if u64::from(width) * u64::from(height) > limits.input_pixels {
        return Err("preview input exceeds pixel limit".into());
    }
    let orientation = decoder
        .orientation()
        .map_err(|err| format!("orientation read failed: {err}"))?;
    let mut image =
        DynamicImage::from_decoder(decoder).map_err(|err| format!("decode failed: {err}"))?;
    image.apply_orientation(orientation);
    if image.width() > PREVIEW_MAX_SIDE || image.height() > PREVIEW_MAX_SIDE {
        image = image.resize(
            PREVIEW_MAX_SIDE,
            PREVIEW_MAX_SIDE,
            image::imageops::FilterType::Triangle,
        );
    }
    let (out_width, out_height) = (image.width(), image.height());
    let mut webp = Vec::new();
    let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut webp);
    if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        encoder
            .encode(
                &rgba,
                out_width,
                out_height,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|err| format!("encode failed: {err}"))?;
    } else {
        let rgb = image.to_rgb8();
        encoder
            .encode(&rgb, out_width, out_height, image::ExtendedColorType::Rgb8)
            .map_err(|err| format!("encode failed: {err}"))?;
    }
    if webp.len() as u64 > limits.output_bytes {
        return Err("preview output exceeds byte limit".into());
    }
    Ok(PreviewImage {
        width: out_width,
        height: out_height,
        webp,
    })
}

/// Entry point for the preview child. `main` calls this before anything else
/// and exits with the returned code when argv[1] is [`PREVIEW_HELPER_ARG`].
pub fn maybe_run_helper() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(PREVIEW_HELPER_ARG) {
        return None;
    }
    let mut limits = PreviewLimits::default();
    let rest: Vec<String> = args.collect();
    for pair in rest.chunks(2) {
        let [name, value] = pair else {
            return Some(child_fail("malformed arguments"));
        };
        let Ok(value) = value.parse::<u64>() else {
            return Some(child_fail("malformed arguments"));
        };
        match name.as_str() {
            "--max-input" => limits.input_bytes = value,
            "--max-pixels" => limits.input_pixels = value,
            "--max-alloc" => limits.max_alloc = value,
            "--max-output" => limits.output_bytes = value,
            "--max-as" => limits.child_address_space = value,
            "--cpu-secs" => limits.timeout = Duration::from_secs(value),
            _ => return Some(child_fail("unknown argument")),
        }
    }
    // Before any input is read: an image that makes the decoder allocate past
    // the ceiling fails the allocation instead of exhausting the host.
    if let Err(err) = document_extract_client::process::apply_rlimits_now(
        limits.child_address_space,
        limits.timeout.as_secs().max(1),
    ) {
        return Some(child_fail(&format!("rlimit: {err}")));
    }
    let mut source = Vec::new();
    let read = std::io::stdin()
        .take(limits.input_bytes.saturating_add(1))
        .read_to_end(&mut source);
    if let Err(err) = read {
        return Some(child_fail(&format!("stdin: {err}")));
    }
    match render_preview(&source, &limits) {
        Ok(preview) => {
            let mut out = std::io::stdout().lock();
            let written = out
                .write_all(&preview.width.to_le_bytes())
                .and_then(|()| out.write_all(&preview.height.to_le_bytes()))
                .and_then(|()| out.write_all(&preview.webp))
                .and_then(|()| out.flush());
            match written {
                Ok(()) => Some(0),
                Err(err) => Some(child_fail(&format!("stdout: {err}"))),
            }
        }
        Err(msg) => Some(child_fail(&msg)),
    }
}

fn child_fail(msg: &str) -> i32 {
    let _ = writeln!(std::io::stderr(), "{msg}");
    2
}

/// Where the preview child lives: this very binary.
pub fn default_helper_path() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

/// Runs `helper --internal-image-preview` on `source`. The child is killed on
/// timeout and when this future is dropped (`kill_on_drop`).
pub async fn run_preview_helper(
    helper: &Path,
    source: Vec<u8>,
    limits: &PreviewLimits,
) -> Result<PreviewImage, PreviewError> {
    if source.len() as u64 > limits.input_bytes {
        return Err(PreviewError::Rejected(
            "preview input exceeds byte limit".into(),
        ));
    }
    let mut command = tokio::process::Command::new(helper);
    // The child dies with the server even on SIGKILL (it is otherwise bounded
    // only by RLIMIT_CPU), as the document extract helper does.
    #[cfg(target_os = "linux")]
    {
        let parent_pid = std::process::id() as i32;
        // SAFETY: the closure runs between fork and exec and only calls the
        // async-signal-safe prctl/getppid/raise/_exit wrapper.
        unsafe {
            command.pre_exec(move || {
                document_extract_client::process::apply_parent_death_signal(parent_pid)
            });
        }
    }
    let mut child = command
        .arg(PREVIEW_HELPER_ARG)
        .arg("--max-input")
        .arg(limits.input_bytes.to_string())
        .arg("--max-pixels")
        .arg(limits.input_pixels.to_string())
        .arg("--max-alloc")
        .arg(limits.max_alloc.to_string())
        .arg("--max-output")
        .arg(limits.output_bytes.to_string())
        .arg("--max-as")
        .arg(limits.child_address_space.to_string())
        .arg("--cpu-secs")
        .arg(limits.timeout.as_secs().max(1).to_string())
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| PreviewError::Worker(format!("spawn failed: {err}")))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let output_cap = limits.output_bytes.saturating_add(8);
    let io = async move {
        let feed = async move {
            // A child that exits early closes the pipe; that is its answer.
            let _ = stdin.write_all(&source).await;
            drop(stdin);
        };
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut out_pipe = (&mut stdout).take(output_cap + 1);
        let mut err_pipe = (&mut stderr).take(4096);
        let read_out = out_pipe.read_to_end(&mut out);
        let read_err = err_pipe.read_to_end(&mut err);
        let ((), out_res, err_res) = tokio::join!(feed, read_out, read_err);
        out_res.map_err(|e| PreviewError::Worker(format!("stdout: {e}")))?;
        err_res.map_err(|e| PreviewError::Worker(format!("stderr: {e}")))?;
        let status = child
            .wait()
            .await
            .map_err(|e| PreviewError::Worker(format!("wait: {e}")))?;
        Ok::<_, PreviewError>((status, out, err))
    };
    let (status, out, err) = match tokio::time::timeout(limits.timeout, io).await {
        Ok(result) => result?,
        // Dropping the future drops the child handle: kill_on_drop reaps it.
        Err(_) => {
            return Err(PreviewError::ResourceLimit(format!(
                "preview watchdog {}s",
                limits.timeout.as_secs()
            )))
        }
    };
    let message = String::from_utf8_lossy(&err).trim().to_string();
    if !status.success() {
        return Err(if status.code() == Some(2) {
            PreviewError::Rejected(message)
        } else {
            // Killed by a signal (RLIMIT_CPU, allocation abort) or crashed.
            PreviewError::ResourceLimit(format!("child exited {status}: {message}"))
        });
    }
    if out.len() as u64 > output_cap || out.len() < 8 {
        return Err(PreviewError::Rejected("preview output malformed".into()));
    }
    let width = u32::from_le_bytes(out[0..4].try_into().expect("4 bytes"));
    let height = u32::from_le_bytes(out[4..8].try_into().expect("4 bytes"));
    if width == 0 || height == 0 || width > PREVIEW_MAX_SIDE || height > PREVIEW_MAX_SIDE {
        return Err(PreviewError::Rejected(
            "preview dimensions malformed".into(),
        ));
    }
    Ok(PreviewImage {
        width,
        height,
        webp: out[8..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn shrinks_to_1600_and_keeps_small_images() {
        let limits = PreviewLimits::default();
        let big = render_preview(&png(3200, 800), &limits).unwrap();
        assert_eq!((big.width, big.height), (1600, 400));
        assert_eq!(&big.webp[0..4], b"RIFF");
        let small = render_preview(&png(40, 30), &limits).unwrap();
        assert_eq!((small.width, small.height), (40, 30));
    }

    #[test]
    fn refuses_pixel_bombs_before_decoding() {
        let limits = PreviewLimits {
            input_pixels: 1000,
            ..PreviewLimits::default()
        };
        let err = render_preview(&png(40, 30), &limits).unwrap_err();
        assert!(err.contains("pixel") || err.contains("limit"), "{err}");
    }

    #[test]
    fn refuses_non_images_and_unsupported_formats() {
        let limits = PreviewLimits::default();
        assert!(render_preview(b"not an image at all", &limits).is_err());
        assert!(render_preview(b"<svg xmlns='http://www.w3.org/2000/svg'/>", &limits).is_err());
        let mut truncated = png(64, 64);
        truncated.truncate(truncated.len() / 2);
        assert!(render_preview(&truncated, &limits).is_err());
        assert!(!preview_mime_supported("image/svg+xml"));
        assert!(preview_mime_supported("image/webp"));
    }
}
