//! Thumbnails for the file list and grid view.
//!
//! Common raster formats are decoded in pure Rust with the `image` crate.
//! HEIC/HEIF (what iPhones shoot) has no Rust decoder, and video frames
//! would need a whole codec stack, so both go through an external tool
//! found on `PATH` at start-up, if any. Either way the result is a small
//! JPEG, cached in memory and keyed by the file's path, size and mtime, so
//! a refreshed listing costs nothing to re-render.
//!
//! The tool approach keeps the binary standalone: without a tool, HEIC and
//! video files simply get a placeholder, and the terminal says what to
//! install.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use bytes::Bytes;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageDecoder, ImageReader, Limits};
use tokio::sync::Semaphore;
use tracing::debug;

/// Longest edge of a thumbnail, in pixels. 320 covers a 3-column grid on a
/// 2x phone screen without looking soft.
pub const SMALL_EDGE: u32 = 320;
/// A preview-sized rendition, used by the page when the browser cannot
/// display the original (HEIC in Chrome/Firefox, TIFF, ...).
pub const LARGE_EDGE: u32 = 1600;
const JPEG_QUALITY: u8 = 82;
/// Total bytes of cached thumbnails before least-recently-used eviction.
const CACHE_LIMIT: usize = 64 * 1024 * 1024;
/// Images above this size are not thumbnailed: decoding them would cost
/// more memory and time than a placeholder is worth.
const MAX_SOURCE_BYTES: u64 = 256 * 1024 * 1024;
/// Decoder allocation cap, independent of file size (a tiny PNG can claim
/// enormous dimensions).
const MAX_DECODE_ALLOC: u64 = 512 * 1024 * 1024;
const MAX_DIMENSION: u32 = 24_000;
/// External tools get this long per file before being killed.
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);

/// Formats the `image` crate handles for us.
const NATIVE_MIMES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "image/bmp",
    "image/tiff",
];
/// Formats that need the external tool.
const TOOL_MIMES: &[&str] = &[
    "image/heic",
    "image/heif",
    "image/heic-sequence",
    "image/avif",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Size {
    Small,
    Large,
}

impl Size {
    fn edge(self) -> u32 {
        match self {
            Size::Small => SMALL_EDGE,
            Size::Large => LARGE_EDGE,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ThumbError {
    #[error("no thumbnail for this type")]
    Unsupported,
    #[error("file too large to thumbnail")]
    TooLarge,
    #[error("cannot read file: {0}")]
    Io(String),
    #[error("cannot decode image: {0}")]
    Decode(String),
    #[error("external tool failed: {0}")]
    Tool(String),
}

#[derive(Debug, Clone)]
pub struct Thumb {
    pub jpeg: Bytes,
    pub etag: String,
}

// --------------------------------------------------------------------------
// External HEIC tool
// --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolKind {
    /// ImageMagick 7 (`magick`) or 6 (`convert`).
    Magick,
    /// `heif-thumbnail` from libheif-examples: fast, uses the embedded thumbnail.
    HeifThumbnail,
    /// `heif-convert` from libheif-examples: full-size conversion.
    HeifConvert,
    /// `vipsthumbnail` from libvips-tools.
    Vips,
    /// macOS built-in.
    Sips,
}

/// A HEIC decoder found on this machine.
#[derive(Debug, Clone)]
pub struct HeicTool {
    program: PathBuf,
    kind: ToolKind,
}

impl HeicTool {
    /// Pick a tool. `LANSHARE_HEIC_TOOL` forces a specific program (the
    /// kind is inferred from its file name); otherwise `PATH` is searched
    /// in order of preference.
    pub fn detect() -> Option<Self> {
        if let Some(forced) = std::env::var_os("LANSHARE_HEIC_TOOL") {
            return Self::from_program(PathBuf::from(forced));
        }
        // `convert` last and never on Windows, where it is a disk utility.
        let candidates: &[&str] = if cfg!(windows) {
            &["magick", "heif-thumbnail", "heif-convert", "vipsthumbnail"]
        } else {
            &[
                "magick",
                "heif-thumbnail",
                "heif-convert",
                "vipsthumbnail",
                "sips",
                "convert",
            ]
        };
        candidates.iter().find_map(|name| {
            let program = find_in_path(name)?;
            Some(Self {
                program,
                kind: Self::kind_for(Path::new(name))?,
            })
        })
    }

    /// Use a specific program. Returns `None` if its name is not one of the
    /// supported tools. Public so tests can inject a stand-in.
    pub fn from_program(program: impl Into<PathBuf>) -> Option<Self> {
        let program = program.into();
        let kind = Self::kind_for(&program)?;
        Some(Self { program, kind })
    }

    fn kind_for(program: &Path) -> Option<ToolKind> {
        Some(match program_name(program)?.as_str() {
            "magick" | "convert" => ToolKind::Magick,
            "heif-thumbnail" => ToolKind::HeifThumbnail,
            "heif-convert" => ToolKind::HeifConvert,
            "vipsthumbnail" => ToolKind::Vips,
            "sips" => ToolKind::Sips,
            _ => return None,
        })
    }

    pub fn describe(&self) -> String {
        self.program.display().to_string()
    }

    /// Command that converts `input` to a JPEG at `output`. Tools that can
    /// downscale are asked to, so the Rust pass afterwards has less to do;
    /// the others produce full size and get resized here.
    fn command(&self, input: &Path, output: &Path, edge: u32) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&self.program);
        let geometry = format!("{edge}x{edge}>");
        match self.kind {
            ToolKind::Magick => {
                // `[0]` selects the primary image, not an embedded thumbnail
                // or an HDR gain map.
                let mut input_spec = input.as_os_str().to_owned();
                input_spec.push("[0]");
                cmd.arg(input_spec)
                    .arg("-auto-orient")
                    .arg("-thumbnail")
                    .arg(&geometry)
                    .arg("-quality")
                    .arg("90")
                    .arg(output);
            }
            ToolKind::HeifThumbnail => {
                cmd.arg("-s").arg(edge.to_string()).arg(input).arg(output);
            }
            ToolKind::HeifConvert => {
                cmd.arg("-q").arg("90").arg(input).arg(output);
            }
            ToolKind::Vips => {
                cmd.arg(input)
                    .arg("-s")
                    .arg(format!("{edge}x{edge}"))
                    .arg("-o")
                    .arg(output);
            }
            ToolKind::Sips => {
                cmd.args(["-s", "format", "jpeg", "-Z"])
                    .arg(edge.to_string())
                    .arg(input)
                    .arg("--out")
                    .arg(output);
            }
        }
        cmd
    }

    /// Run the tool and return the JPEG it produced.
    async fn convert(&self, source: &Path, edge: u32) -> Result<Vec<u8>, ThumbError> {
        run_in_tempdir(source, &self.describe(), |i, o| self.command(i, o, edge)).await
    }
}

/// Link `source` into a private temp directory under a fixed name, run the
/// command the closure builds for that input and a `thumb.jpg` output, and
/// return the output bytes.
///
/// The fixed name sidesteps every tool's own path syntax: ImageMagick
/// treats `foo:bar.heic` as a coder prefix and `x.heic[2]` as a frame
/// index, GStreamer parses `location=` values, and none of them accept
/// `--` consistently.
async fn run_in_tempdir(
    source: &Path,
    describe: &str,
    build: impl FnOnce(&Path, &Path) -> tokio::process::Command,
) -> Result<Vec<u8>, ThumbError> {
    let dir = tempfile::Builder::new()
        .prefix("lanshare-thumb-")
        .tempdir()
        .map_err(|e| ThumbError::Io(e.to_string()))?;
    let ext = source
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "bin".into());
    let input = dir.path().join(format!("input.{ext}"));
    let output = dir.path().join("thumb.jpg");
    link_or_copy(source, &input).await?;

    let mut cmd = build(&input, &output);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let run = async {
        let child = cmd
            .spawn()
            .map_err(|e| ThumbError::Tool(format!("cannot start {describe}: {e}")))?;
        child
            .wait_with_output()
            .await
            .map_err(|e| ThumbError::Tool(e.to_string()))
    };
    let out = match tokio::time::timeout(TOOL_TIMEOUT, run).await {
        Ok(result) => result?,
        Err(_) => return Err(ThumbError::Tool("timed out".into())),
    };
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(ThumbError::Tool(
            stderr.lines().last().unwrap_or("non-zero exit").to_string(),
        ));
    }
    tokio::fs::read(&output)
        .await
        .map_err(|e| ThumbError::Tool(format!("no output produced: {e}")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VideoToolKind {
    Ffmpeg,
    /// `gst-launch-1.0`: present on most Linux desktops even without ffmpeg.
    GStreamer,
}

/// A program that can grab a frame from a video.
#[derive(Debug, Clone)]
pub struct VideoTool {
    program: PathBuf,
    kind: VideoToolKind,
}

impl VideoTool {
    /// `LANSHARE_VIDEO_TOOL` forces a program; otherwise ffmpeg is preferred
    /// and GStreamer's launcher is the fallback.
    pub fn detect() -> Option<Self> {
        if let Some(forced) = std::env::var_os("LANSHARE_VIDEO_TOOL") {
            return Self::from_program(PathBuf::from(forced));
        }
        ["ffmpeg", "gst-launch-1.0"].iter().find_map(|name| {
            let program = find_in_path(name)?;
            Self::from_program(program)
        })
    }

    pub fn from_program(program: impl Into<PathBuf>) -> Option<Self> {
        let program = program.into();
        let kind = match program_name(&program)?.as_str() {
            "ffmpeg" => VideoToolKind::Ffmpeg,
            "gst-launch-1.0" | "gst-launch" => VideoToolKind::GStreamer,
            _ => return None,
        };
        Some(Self { program, kind })
    }

    pub fn describe(&self) -> String {
        self.program.display().to_string()
    }

    /// `attempt` 0 seeks one second in (skips black lead-ins); attempt 1
    /// takes the very first frame, for clips shorter than that.
    fn command(
        &self,
        input: &Path,
        output: &Path,
        edge: u32,
        attempt: u32,
    ) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&self.program);
        match self.kind {
            VideoToolKind::Ffmpeg => {
                cmd.args(["-y", "-loglevel", "error", "-nostdin"]);
                if attempt == 0 {
                    cmd.args(["-ss", "1"]);
                }
                // ffmpeg applies the rotation tag from phone videos itself.
                cmd.arg("-i")
                    .arg(input)
                    .args(["-frames:v", "1", "-vf"])
                    .arg(format!("scale='min({edge},iw)':-2"))
                    .args(["-q:v", "3"])
                    .arg(output);
            }
            VideoToolKind::GStreamer => {
                // `snapshot=true` makes jpegenc emit one frame and end the
                // pipeline; `videoflip video-direction=auto` honours the
                // rotation tag that iPhone recordings carry.
                let mut location_in = std::ffi::OsString::from("location=");
                location_in.push(input);
                let mut location_out = std::ffi::OsString::from("location=");
                location_out.push(output);
                cmd.arg("-q")
                    .arg("filesrc")
                    .arg(location_in)
                    .args([
                        "!",
                        "decodebin",
                        "!",
                        "videoconvert",
                        "!",
                        "videoflip",
                        "video-direction=auto",
                    ])
                    .args(["!", "jpegenc", "snapshot=true", "!", "filesink"])
                    .arg(location_out);
            }
        }
        cmd
    }

    async fn convert(&self, source: &Path, edge: u32) -> Result<Vec<u8>, ThumbError> {
        let describe = self.describe();
        let first = run_in_tempdir(source, &describe, |i, o| self.command(i, o, edge, 0)).await;
        match (first, self.kind) {
            (Ok(jpeg), _) => Ok(jpeg),
            // Seeking past the end of a very short clip yields nothing;
            // the first frame will do.
            (Err(_), VideoToolKind::Ffmpeg) => {
                run_in_tempdir(source, &describe, |i, o| self.command(i, o, edge, 1)).await
            }
            (Err(e), _) => Err(e),
        }
    }
}

/// Lower-case file name without a Windows `.exe`. Not `file_stem`, which
/// would turn `gst-launch-1.0` into `gst-launch-1`.
fn program_name(program: &Path) -> Option<String> {
    let name = program.file_name()?.to_string_lossy().to_ascii_lowercase();
    Some(name.strip_suffix(".exe").unwrap_or(&name).to_string())
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

async fn link_or_copy(source: &Path, target: &Path) -> Result<(), ThumbError> {
    #[cfg(unix)]
    {
        tokio::fs::symlink(source, target)
            .await
            .map_err(|e| ThumbError::Io(e.to_string()))
    }
    #[cfg(not(unix))]
    {
        // Symlinks need privileges on Windows; a copy is the portable fallback.
        tokio::fs::copy(source, target)
            .await
            .map(|_| ())
            .map_err(|e| ThumbError::Io(e.to_string()))
    }
}

// --------------------------------------------------------------------------
// Cache
// --------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    size: u64,
    mtime: u128,
    thumb: Size,
}

#[derive(Debug, Clone)]
enum Cached {
    Ok(Bytes),
    /// Remember failures too, so a corrupt file is not re-decoded on every
    /// listing refresh. Costs a nominal amount of cache budget.
    Failed(String),
}

impl Cached {
    fn cost(&self) -> usize {
        match self {
            Cached::Ok(b) => b.len(),
            Cached::Failed(_) => 1024,
        }
    }
}

/// Least-recently-used cache bounded by total bytes. Eviction is a linear
/// scan, which is fine for the few thousand entries a share can hold.
#[derive(Debug, Default)]
struct Cache {
    entries: HashMap<Key, (Cached, u64)>,
    total: usize,
    clock: u64,
}

impl Cache {
    fn get(&mut self, key: &Key) -> Option<Cached> {
        self.clock += 1;
        let clock = self.clock;
        let (value, last_used) = self.entries.get_mut(key)?;
        *last_used = clock;
        Some(value.clone())
    }

    fn insert(&mut self, key: Key, value: Cached) {
        self.clock += 1;
        if let Some((old, _)) = self.entries.remove(&key) {
            self.total -= old.cost();
        }
        self.total += value.cost();
        self.entries.insert(key, (value, self.clock));
        while self.total > CACHE_LIMIT && self.entries.len() > 1 {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone())
                .expect("non-empty");
            if let Some((old, _)) = self.entries.remove(&oldest) {
                self.total -= old.cost();
            }
        }
    }
}

// --------------------------------------------------------------------------
// Service
// --------------------------------------------------------------------------

pub struct Thumbnailer {
    heic: Option<HeicTool>,
    video: Option<VideoTool>,
    cache: Mutex<Cache>,
    /// Decoding is CPU-bound; limit how many run at once so a grid of 200
    /// photos does not starve the download streams.
    gate: Semaphore,
}

impl Thumbnailer {
    pub fn new(heic: Option<HeicTool>, video: Option<VideoTool>) -> Self {
        let parallel = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(1, 4);
        Self {
            heic,
            video,
            cache: Mutex::new(Cache::default()),
            gate: Semaphore::new(parallel),
        }
    }

    pub fn heic_tool(&self) -> Option<&HeicTool> {
        self.heic.as_ref()
    }

    pub fn video_tool(&self) -> Option<&VideoTool> {
        self.video.as_ref()
    }

    /// Whether a thumbnail can be produced for this content type here.
    pub fn supports(&self, mime: &str) -> bool {
        NATIVE_MIMES.contains(&mime)
            || (self.heic.is_some() && TOOL_MIMES.contains(&mime))
            || (self.video.is_some() && mime.starts_with("video/"))
    }

    /// Thumbnail for `path`, from cache when possible.
    pub async fn get(&self, path: &Path, mime: &str, size: Size) -> Result<Thumb, ThumbError> {
        if !self.supports(mime) {
            return Err(ThumbError::Unsupported);
        }
        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| ThumbError::Io(e.to_string()))?;
        if meta.len() > MAX_SOURCE_BYTES {
            return Err(ThumbError::TooLarge);
        }
        let key = Key {
            path: path.to_path_buf(),
            size: meta.len(),
            mtime: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            thumb: size,
        };
        let etag = format!("\"{:x}-{:x}-{}\"", key.size, key.mtime, size.edge());

        if let Some(cached) = self.cache.lock().unwrap().get(&key) {
            return match cached {
                Cached::Ok(jpeg) => Ok(Thumb { jpeg, etag }),
                Cached::Failed(why) => Err(ThumbError::Decode(why)),
            };
        }

        let _permit = self.gate.acquire().await.expect("semaphore never closed");
        // Another request may have filled the cache while we waited.
        if let Some(Cached::Ok(jpeg)) = self.cache.lock().unwrap().get(&key) {
            return Ok(Thumb { jpeg, etag });
        }

        let result = self.generate(path, mime, size).await;
        let cached = match &result {
            Ok(jpeg) => Cached::Ok(jpeg.clone()),
            Err(e) => Cached::Failed(e.to_string()),
        };
        self.cache.lock().unwrap().insert(key, cached);
        result.map(|jpeg| Thumb { jpeg, etag })
    }

    async fn generate(&self, path: &Path, mime: &str, size: Size) -> Result<Bytes, ThumbError> {
        let edge = size.edge();
        let source: Vec<u8> = if TOOL_MIMES.contains(&mime) {
            let tool = self.heic.as_ref().ok_or(ThumbError::Unsupported)?;
            debug!(path = %path.display(), tool = %tool.describe(), "heic via tool");
            tool.convert(path, edge).await?
        } else if mime.starts_with("video/") {
            let tool = self.video.as_ref().ok_or(ThumbError::Unsupported)?;
            debug!(path = %path.display(), tool = %tool.describe(), "video frame via tool");
            tool.convert(path, edge).await?
        } else {
            tokio::fs::read(path)
                .await
                .map_err(|e| ThumbError::Io(e.to_string()))?
        };
        // Everything funnels through the same resize/orient/encode step so
        // the output is uniform regardless of which decoder produced it.
        let jpeg = tokio::task::spawn_blocking(move || shrink_to_jpeg(&source, edge))
            .await
            .map_err(|e| ThumbError::Decode(e.to_string()))??;
        Ok(Bytes::from(jpeg))
    }
}

/// Decode, honour EXIF orientation (phones store landscape pixels plus a
/// rotation flag), fit within `edge`, encode as JPEG.
fn shrink_to_jpeg(data: &[u8], edge: u32) -> Result<Vec<u8>, ThumbError> {
    let mut reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| ThumbError::Decode(e.to_string()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| ThumbError::Decode(e.to_string()))?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img =
        DynamicImage::from_decoder(decoder).map_err(|e| ThumbError::Decode(e.to_string()))?;
    img.apply_orientation(orientation);

    // `thumbnail` is the fast path (box filter); quality is plenty at these
    // sizes and it is several times quicker than `resize` on a 48 MP photo.
    let small = if img.width() > edge || img.height() > edge {
        img.thumbnail(edge, edge)
    } else {
        img
    };
    // JPEG has no alpha; flatten onto white rather than letting the encoder
    // reject RGBA input.
    let rgb = small.to_rgb8();
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode_image(&rgb)
        .map_err(|e| ThumbError::Decode(e.to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let img = ImageBuffer::from_fn(width, height, |x, _| Rgba([x as u8, 0, 0, 255]));
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn jpeg_dimensions(data: &[u8]) -> (u32, u32) {
        ImageReader::new(Cursor::new(data))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap()
    }

    #[test]
    fn shrinks_large_and_keeps_small() {
        let big = shrink_to_jpeg(&png(1000, 500), SMALL_EDGE).unwrap();
        assert_eq!(jpeg_dimensions(&big), (320, 160));
        let small = shrink_to_jpeg(&png(100, 50), SMALL_EDGE).unwrap();
        assert_eq!(jpeg_dimensions(&small), (100, 50));
        assert!(shrink_to_jpeg(b"not an image", SMALL_EDGE).is_err());
    }

    #[test]
    fn cache_evicts_least_recently_used() {
        let mut cache = Cache::default();
        let key = |n: u64| Key {
            path: PathBuf::from(format!("/f{n}")),
            size: n,
            mtime: 0,
            thumb: Size::Small,
        };
        let chunk = Bytes::from(vec![0u8; CACHE_LIMIT / 2]);
        cache.insert(key(1), Cached::Ok(chunk.clone()));
        cache.insert(key(2), Cached::Ok(chunk.clone()));
        assert!(cache.get(&key(1)).is_some()); // touch 1, so 2 is the oldest
        cache.insert(key(3), Cached::Ok(chunk));
        assert!(cache.get(&key(1)).is_some());
        assert!(cache.get(&key(2)).is_none());
        assert!(cache.get(&key(3)).is_some());
        assert!(cache.total <= CACHE_LIMIT);
    }

    #[test]
    fn tool_kind_from_name() {
        assert_eq!(
            HeicTool::kind_for(Path::new("/usr/bin/magick")),
            Some(ToolKind::Magick)
        );
        assert_eq!(
            HeicTool::kind_for(Path::new("heif-thumbnail")),
            Some(ToolKind::HeifThumbnail)
        );
        assert_eq!(HeicTool::kind_for(Path::new("gimp")), None);
    }

    #[test]
    fn supports_depends_on_tool() {
        let without = Thumbnailer::new(None, None);
        assert!(without.supports("image/jpeg"));
        assert!(!without.supports("image/heic"));
        assert!(!without.supports("video/mp4"));
        let with = Thumbnailer::new(
            Some(HeicTool {
                program: "magick".into(),
                kind: ToolKind::Magick,
            }),
            VideoTool::from_program("/usr/bin/ffmpeg"),
        );
        assert!(with.supports("image/heic"));
        assert!(with.supports("video/quicktime"));
        assert!(VideoTool::from_program("gst-launch-1.0").is_some());
        assert!(VideoTool::from_program("vlc").is_none());
    }
}
