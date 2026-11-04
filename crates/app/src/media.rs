//! Inline photos and GIFs, and sending images. What to download, convert,
//! draw and run is decided by pure functions at the top; `FrillogramApp`'s
//! methods below carry it out, and the `render_*` helpers draw it.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    sync::{Arc, OnceLock},
    time::{Duration, SystemTime},
};

use ::image::{ImageDecoder, ImageReader, metadata::Orientation};
use frillogram_core::{Animation, Media, Msg, Upload, config::MediaSettings};
use gpui_kit::{
    assets::IconName as Lucide,
    component::{
        ActiveTheme, Disableable, Icon, Sizable, ThemeStyled,
        button::{Button, ButtonVariants},
        h_flex,
        input::Textarea,
        spinner::Spinner,
    },
    prelude::FluentBuilder,
    *,
};

use crate::app::{FrillogramApp, Restore, park_draft, reply_thread_open, restore_failed_text, restore_target};

/// Frame rate of a converted GIF. GPUI keeps every decoded frame in memory,
/// so fewer frames keep a chat full of GIFs affordable.
const GIF_FPS: u32 = 15;
/// How many ffmpeg conversions run at once, so a chat full of GIFs doesn't
/// start dozens of them.
const MAX_CONVERSIONS: usize = 2;
/// How many downloads run at once without a click, so opening a chat with a
/// long history doesn't queue hundreds; each that ends starts the next.
const MAX_DOWNLOADS: usize = 4;
/// The largest real GIF that plays as it is, when ffmpeg can't shrink it.
/// GPUI decodes every frame to 4 bytes per pixel and keeps them all for the
/// session, so a big file or canvas can cost gigabytes; those show as stills.
const MAX_DIRECT_GIF_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DIRECT_GIF_PIXELS: u64 = 1920 * 1080;
/// Folders inside the TDLib data dir.
const GIF_CACHE_DIR: &str = "gif-cache";
const PASTED_DIR: &str = "pasted";
/// Pasted images are removed once their message is sent or has failed. This
/// catches those whose message never settled while Frillogram ran, e.g. when
/// it quit mid-upload; TDLib retries those from the file after a restart.
const PASTED_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// A download or conversion that was started.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Job {
    Running,
    Ready(PathBuf),
    Failed(String),
}

/// Downloads and conversions of the media Frillogram has shown, kept across
/// chat switches. TDLib's file ids are only valid for this session, and so is this.
#[derive(Default)]
pub(crate) struct MediaState {
    /// By TDLib file id. A file that isn't here hasn't been asked for.
    downloads: HashMap<i32, Job>,
    /// Sizes in bytes of downloaded files, by file id.
    sizes: HashMap<i32, u64>,
    /// GIFs converted by ffmpeg to a smaller GIF, by the original's file id.
    conversions: HashMap<i32, Job>,
    /// GIFs the user clicked to play while `autoplay_gifs` is off.
    play_requested: HashSet<i32>,
    /// Spoilers the user clicked to reveal, by chat and message id.
    revealed: HashSet<(i64, i64)>,
    /// Files being downloaded to open in the opener, so a second click
    /// doesn't open a second viewer.
    opening: HashSet<i32>,
    /// ffmpeg couldn't be started, so nothing is converted for the rest of
    /// the session and the user was told once.
    ffmpeg_missing: bool,
    /// TDLib's data dir, which holds converted GIFs and pasted images.
    data_dir: Option<PathBuf>,
    /// Set once the GIF cache's leftover partial files were removed.
    cache_swept: Arc<OnceLock<()>>,
    /// Pasted images, by the chat and temporary id of the message sending them.
    pastes: HashMap<(i64, i64), PathBuf>,
    /// Converted GIFs that GPUI couldn't decode, and were deleted to be
    /// converted again next session.
    discarded: Rc<RefCell<HashSet<PathBuf>>>,
}

/// What a media message draws.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Visual {
    /// The image file at this path.
    Image(PathBuf),
    /// The minithumbnail, or an empty box, while there's nothing better.
    Placeholder,
    /// A box saying why it can't be shown.
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Badge {
    /// A GIF shown as a still, which opens in the opener.
    Gif,
    /// A GIF that plays inline when clicked.
    Play,
    /// Covered media, shown when clicked.
    Spoiler,
}

/// What clicking a media message does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Click {
    Nothing,
    /// Starts or retries downloading this file.
    Download(i32),
    /// Plays this GIF inline.
    Play(i32),
    /// Opens this file, downloading it first if needed.
    Open(i32),
    /// Uncovers the message's spoiler.
    Reveal,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct View {
    pub visual: Visual,
    pub badge: Option<Badge>,
    pub click: Click,
    /// The file is on its way to the opener.
    pub loading: bool,
}

/// How a GIF can play inline.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Playback {
    /// A real GIF, which GPUI animates itself if it's small enough.
    Gif,
    /// An MP4 or a GIF, which plays once ffmpeg converted it to a GIF at the
    /// size it's drawn.
    Convert,
    /// Neither, so it shows as a still.
    Still,
}

/// A GIF to convert: the downloaded file, where the GIF goes and how wide it is.
#[derive(Debug, Clone, PartialEq)]
struct Conversion {
    file_id: i32,
    input: PathBuf,
    output: PathBuf,
    width: u32,
}

/// Where an attachment goes, fixed when the user asked to send it: the file
/// picker isn't modal, so the user may be in another chat by the time it closes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SendTarget {
    chat_id: i64,
    in_thread: bool,
    /// The open thread's root, for a send from the thread composer.
    reply_to: Option<i64>,
}

/// Whether a send to `target` takes the text of its composer as the caption:
/// only while that composer still belongs to it. Elsewhere it's another
/// chat's or thread's text.
fn takes_caption(target: SendTarget, selected_chat: Option<i64>, thread_open: bool) -> bool {
    selected_chat == Some(target.chat_id) && (!target.in_thread || thread_open)
}

/// Whether a real GIF of `bytes` with a `width` × `height` canvas is small
/// enough to play without being shrunk first.
fn gif_fits_in_memory(bytes: u64, width: u32, height: u32) -> bool {
    bytes <= MAX_DIRECT_GIF_BYTES && u64::from(width) * u64::from(height) <= MAX_DIRECT_GIF_PIXELS
}

impl MediaState {
    pub(crate) fn new(data_dir: Option<PathBuf>) -> Self {
        MediaState { data_dir, ..Default::default() }
    }

    fn download(&self, file_id: i32) -> Option<&Job> {
        self.downloads.get(&file_id)
    }

    fn plays(&self, animation: &Animation, settings: &MediaSettings) -> bool {
        settings.autoplay_gifs || self.play_requested.contains(&animation.file_id)
    }

    fn playback(&self, animation: &Animation, settings: &MediaSettings) -> Playback {
        let gif = animation.mime_type == "image/gif";
        let convertible = !self.ffmpeg_missing && self.gif_path(animation, settings).is_some();
        if convertible && (gif || animation.mime_type.starts_with("video/")) {
            Playback::Convert
        } else if gif {
            Playback::Gif
        } else {
            Playback::Still
        }
    }

    /// Whether the downloaded real GIF `animation` can play as it is.
    fn plays_directly(&self, animation: &Animation) -> bool {
        let bytes = self.sizes.get(&animation.file_id);
        bytes.is_some_and(|&bytes| gif_fits_in_memory(bytes, animation.width, animation.height))
    }

    /// Where `animation` converted to a GIF is cached, if it can be converted.
    fn gif_path(&self, animation: &Animation, settings: &MediaSettings) -> Option<PathBuf> {
        if settings.ffmpeg.is_empty() {
            return None;
        }
        let width = gif_width(animation, settings);
        gif_cache_path(self.data_dir.as_deref()?, &animation.unique_id, width)
    }

    /// Whether `path` is a GIF that ffmpeg made, which can be made again.
    fn is_converted(&self, path: &Path) -> bool {
        self.data_dir.as_ref().is_some_and(|dir| path.starts_with(dir.join(GIF_CACHE_DIR)))
    }

    /// The files of `media` to download without being asked.
    fn wanted_downloads(&self, media: &Media, settings: &MediaSettings) -> Vec<i32> {
        match media {
            Media::Photo(photo) if settings.autoload => vec![photo.file_id],
            Media::Photo(_) => Vec::new(),
            Media::Animation(animation) => {
                let mut files = Vec::new();
                if settings.autoload
                    && let Some(thumbnail) = animation.thumbnail.as_ref().filter(|t| t.format.is_image())
                {
                    files.push(thumbnail.file_id);
                }
                let wanted = if self.play_requested.contains(&animation.file_id) {
                    true
                } else {
                    settings.autoload && settings.autoplay_gifs
                };
                if wanted && self.playback(animation, settings) != Playback::Still {
                    files.push(animation.file_id);
                }
                files
            }
        }
    }

    /// The conversion `media` is waiting for, if it's a GIF that plays, is
    /// downloaded and wasn't converted yet.
    fn wanted_conversion(&self, media: &Media, settings: &MediaSettings) -> Option<Conversion> {
        let Media::Animation(animation) = media else { return None };
        if !self.plays(animation, settings)
            || self.playback(animation, settings) != Playback::Convert
            || self.conversions.contains_key(&animation.file_id)
        {
            return None;
        }
        let Some(Job::Ready(input)) = self.download(animation.file_id) else { return None };
        Some(Conversion {
            file_id: animation.file_id,
            input: input.clone(),
            output: self.gif_path(animation, settings)?,
            width: gif_width(animation, settings),
        })
    }

    /// The downloads and conversions to start for `messages`, newest first
    /// since a chat opens at its bottom, within the caps on how many run.
    fn jobs_to_start(&self, messages: &[Msg], settings: &MediaSettings) -> (Vec<i32>, Vec<Conversion>) {
        let running = |jobs: &HashMap<i32, Job>| jobs.values().filter(|job| **job == Job::Running).count();
        let download_slots = MAX_DOWNLOADS.saturating_sub(running(&self.downloads));
        let conversion_slots = MAX_CONVERSIONS.saturating_sub(running(&self.conversions));
        let mut downloads = Vec::new();
        let mut conversions: Vec<Conversion> = Vec::new();
        for media in messages.iter().rev().filter_map(|m| m.media.as_ref()) {
            for file_id in self.wanted_downloads(media, settings) {
                if downloads.len() < download_slots && self.download(file_id).is_none() && !downloads.contains(&file_id)
                {
                    downloads.push(file_id);
                }
            }
            if conversions.len() < conversion_slots
                && let Some(conversion) = self.wanted_conversion(media, settings)
                && !conversions.iter().any(|c| c.file_id == conversion.file_id)
            {
                conversions.push(conversion);
            }
        }
        (downloads, conversions)
    }

    /// What `message`'s media draws: covered while it's an unrevealed
    /// spoiler, and never opened outside the app if it's protected.
    pub(crate) fn message_view(&self, message: &Msg, settings: &MediaSettings) -> Option<View> {
        let media = message.media.as_ref()?;
        if media.has_spoiler() && !self.revealed.contains(&(message.chat_id, message.id)) {
            let badge = Some(Badge::Spoiler);
            return Some(View { visual: Visual::Placeholder, badge, click: Click::Reveal, loading: false });
        }
        let mut view = self.view(media, settings);
        if !message.can_be_saved && matches!(view.click, Click::Open(_)) {
            view.click = Click::Nothing;
        }
        Some(view)
    }

    pub(crate) fn view(&self, media: &Media, settings: &MediaSettings) -> View {
        let mut view = self.inline_view(media, settings);
        if let Click::Open(file_id) = view.click
            && self.opening.contains(&file_id)
        {
            view.click = Click::Nothing;
            view.loading = true;
        }
        view
    }

    fn inline_view(&self, media: &Media, settings: &MediaSettings) -> View {
        let view = |visual, badge, click| View { visual, badge, click, loading: false };
        let animation = match media {
            Media::Photo(photo) => {
                let id = photo.file_id;
                return match self.download(id) {
                    Some(Job::Ready(path)) => view(Visual::Image(path.clone()), None, Click::Open(id)),
                    Some(Job::Failed(error)) => view(Visual::Failed(error.clone()), None, Click::Download(id)),
                    Some(Job::Running) => view(Visual::Placeholder, None, Click::Nothing),
                    None => view(Visual::Placeholder, None, Click::Download(id)),
                };
            }
            Media::Animation(animation) => animation,
        };
        let id = animation.file_id;
        // The thumbnail, while the GIF can't play or isn't ready.
        let still = || {
            let thumbnail = animation.thumbnail.as_ref().filter(|t| t.format.is_image());
            match thumbnail.and_then(|t| self.download(t.file_id)) {
                Some(Job::Ready(path)) => Visual::Image(path.clone()),
                _ => Visual::Placeholder,
            }
        };
        let playback = self.playback(animation, settings);
        if playback == Playback::Still {
            // The sender picks the mime type, and the opener runs whatever
            // it's given, so only real GIFs and videos open.
            let click = if is_gif_or_video(&animation.mime_type) { Click::Open(id) } else { Click::Nothing };
            return view(still(), Some(Badge::Gif), click);
        }
        if !self.plays(animation, settings) {
            return view(still(), Some(Badge::Play), Click::Play(id));
        }
        match self.download(id) {
            Some(Job::Ready(path)) => match (playback, self.conversions.get(&id)) {
                (Playback::Gif, _) if self.plays_directly(animation) => {
                    view(Visual::Image(path.clone()), None, Click::Open(id))
                }
                (Playback::Gif, _) => view(still(), Some(Badge::Gif), Click::Open(id)),
                (_, Some(Job::Ready(gif))) => view(Visual::Image(gif.clone()), None, Click::Open(id)),
                (_, Some(Job::Failed(_))) => view(still(), Some(Badge::Gif), Click::Open(id)),
                _ => view(still(), None, Click::Open(id)),
            },
            Some(Job::Failed(error)) => view(Visual::Failed(error.clone()), None, Click::Download(id)),
            Some(Job::Running) => view(still(), None, Click::Nothing),
            // Clicked to play while `autoload` is off; the download starts next.
            None => view(still(), None, Click::Download(id)),
        }
    }

    /// Marks `file_id` as on its way to the opener, unless it already is.
    fn begin_open(&mut self, file_id: i32) -> bool {
        self.opening.insert(file_id)
    }
}

/// The size an image of `width` × `height` px is drawn at: its aspect ratio
/// fitted into `max_width` × `max_height`, never enlarged. Without a known
/// size it gets a square box.
pub(crate) fn fit(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        let side = max_width.min(max_height).max(1);
        return (side, side);
    }
    let scale = (f64::from(max_width) / f64::from(width)).min(f64::from(max_height) / f64::from(height)).min(1.);
    let scaled = |side: u32| ((f64::from(side) * scale).round() as u32).max(1);
    (scaled(width), scaled(height))
}

/// How wide a converted GIF is: as drawn, so no frame is bigger than needed.
fn gif_width(animation: &Animation, settings: &MediaSettings) -> u32 {
    fit(animation.width, animation.height, settings.max_width, settings.max_height).0
}

/// Where the GIF made from the file `unique_id` at `width` px is cached.
/// The id names a file, so one that isn't plain base64url is refused.
fn gif_cache_path(data_dir: &Path, unique_id: &str, width: u32) -> Option<PathBuf> {
    let plain = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    if unique_id.is_empty() || !unique_id.chars().all(plain) {
        return None;
    }
    Some(data_dir.join(GIF_CACHE_DIR).join(format!("{unique_id}-{width}.gif")))
}

/// The name ffmpeg writes `output` under until it's complete. It's unique to
/// this process and moment, so a conversion left running by an earlier
/// launch can't write into the same file.
fn partial_path(output: &Path, pid: u32, nanos: u128) -> PathBuf {
    let name = output.file_name().unwrap_or_default().to_string_lossy();
    output.with_file_name(format!("{name}.{pid}-{nanos}.part"))
}

/// Removes the partial files that conversions of an earlier launch left in
/// the GIF cache `dir`. Run before this session's first conversion there.
fn sweep_partials(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().ends_with(".part") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Whether a file starting with `header` is a GIF.
fn is_gif(header: &[u8]) -> bool {
    header.starts_with(b"GIF8")
}

/// Whether the file at `path` exists and starts like a GIF.
fn has_gif_header(path: &Path) -> bool {
    let mut header = [0; 4];
    std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut header)).is_ok() && is_gif(&header)
}

/// ffmpeg's arguments to turn the video or GIF `input` into a looping GIF at
/// `output`, `width` px wide. One palette is computed for the whole clip,
/// which looks far better than ffmpeg's default one.
fn gif_args(input: &Path, output: &Path, width: u32) -> Vec<OsString> {
    let filter = format!("fps={GIF_FPS},scale={width}:-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse");
    let mut args: Vec<OsString> =
        ["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"].into_iter().map(OsString::from).collect();
    args.push(input.into());
    // `output` is a temporary name without a .gif extension, so the format is explicit.
    for arg in ["-an", "-vf", &filter, "-loop", "0", "-f", "gif"] {
        args.push(arg.into());
    }
    args.push(output.into());
    args
}

#[derive(Debug, PartialEq)]
enum ConvertError {
    /// ffmpeg couldn't be started at all, which is worth telling the user once.
    CantRun(String),
    /// ffmpeg ran and failed on this one file.
    Failed(String),
}

/// Converts `input` to a GIF at `output` with `ffmpeg`, unless it's there
/// from an earlier run. Blocks until ffmpeg exits, so it runs on a thread of
/// its own.
fn convert(ffmpeg: &str, input: &Path, output: &Path, width: u32) -> Result<PathBuf, ConvertError> {
    // A cached file that isn't a GIF is converted again and replaced.
    if has_gif_header(output) {
        return Ok(output.to_path_buf());
    }
    let failed = |error: String| ConvertError::Failed(error);
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir).map_err(|error| failed(format!("Can't create the GIF cache: {error}")))?;
    }
    // Written under another name first, so a half-written GIF is never cached.
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let partial = partial_path(output, std::process::id(), nanos);
    let result =
        Command::new(ffmpeg)
            .args(gif_args(input, &partial, width))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| ConvertError::CantRun(format!("Can't run {ffmpeg}: {error}")))
            .and_then(|output| {
                if output.status.success() {
                    return Ok(());
                }
                let stderr = String::from_utf8_lossy(&output.stderr);
                let last_line = stderr.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or_default();
                Err(failed(format!("{ffmpeg} failed ({}): {}", output.status, last_line.trim())))
            })
            .and_then(|()| {
                if has_gif_header(&partial) { Ok(()) } else { Err(failed(format!("{ffmpeg} didn't write a GIF"))) }
            })
            .and_then(|()| std::fs::rename(&partial, output).map_err(|error| failed(error.to_string())));
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result.map(|()| output.to_path_buf())
}

/// Runs `job` on a thread of its own and resolves to its result, or `None`
/// if no thread could be started. For work that waits on another program:
/// GPUI's background executor has a fixed pool of threads, which decode
/// images too, and waiting there would stall them.
async fn on_own_thread<T: Send + 'static>(name: &str, job: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name(name.into()).spawn(move || {
        let _ = tx.send_blocking(job());
    });
    rx.recv().await.ok()
}

fn is_gif_or_video(mime_type: &str) -> bool {
    mime_type == "image/gif" || mime_type.starts_with("video/")
}

/// Whether `path` is an image or video by its extension. The opener picks a
/// program by extension, and a sender who names a file `x.command` or
/// `x.jar` could otherwise have it run on click.
fn is_openable(path: &Path) -> bool {
    const EXTENSIONS: [&str; 10] = ["jpg", "jpeg", "png", "webp", "gif", "mp4", "m4v", "mov", "webm", "mkv"];
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    EXTENSIONS.iter().any(|known| extension.eq_ignore_ascii_case(known))
}

/// The program and arguments that open `path`: `open_command` split like a
/// shell would with the path added last, or the platform's opener for "".
fn opener(open_command: &str, path: &Path) -> (String, Vec<OsString>) {
    let default = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let mut words = frillogram_core::config::command_words(open_command).into_iter();
    let program = words.next().unwrap_or_else(|| default.to_string());
    let args = words.map(OsString::from).chain(std::iter::once(path.as_os_str().to_owned())).collect();
    (program, args)
}

/// Opens `path` with the opener, which is waited for on a thread of its own:
/// one like feh only exits when its window closes. The first message says
/// whether it started; a second comes if it then failed. Errors leave out
/// the path and the opener's output, which may quote it, since it's inside
/// the private data dir.
fn open_path(open_command: &str, path: &Path) -> async_channel::Receiver<Result<(), String>> {
    let (program, args) = opener(open_command, path);
    let (tx, rx) = async_channel::bounded(2);
    let _ = std::thread::Builder::new().name("opener".into()).spawn(move || {
        let child =
            Command::new(&program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                let _ = tx.send_blocking(Err(format!("Can't run {program} to open the file: {error}")));
                return;
            }
        };
        let _ = tx.send_blocking(Ok(()));
        if let Ok(status) = child.wait()
            && !status.success()
        {
            let error =
                format!("{program} couldn't open the file ({status}). Set [media] open_command to change the opener.");
            let _ = tx.send_blocking(Err(error));
        }
    });
    rx
}

/// The width and height of the image at `path` as it's shown, from its
/// header alone, or 0 × 0 if it can't be read.
fn image_size(path: &Path) -> (u32, u32) {
    let decoder = ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(::image::ImageError::IoError)
        .and_then(|reader| reader.into_decoder());
    let Ok(mut decoder) = decoder else { return (0, 0) };
    let size = decoder.dimensions();
    oriented(size, decoder.orientation().unwrap_or(Orientation::NoTransforms))
}

/// `size` as shown with the EXIF `orientation`, which may turn it sideways.
fn oriented((width, height): (u32, u32), orientation: Orientation) -> (u32, u32) {
    match orientation {
        Orientation::Rotate90 | Orientation::Rotate270 | Orientation::Rotate90FlipH | Orientation::Rotate270FlipH => {
            (height, width)
        }
        _ => (width, height),
    }
}

/// The image on the clipboard, unless there's text too: copying from a
/// document often carries a picture of it, and then the text is meant.
fn pasted_image(item: &ClipboardItem) -> Option<&Image> {
    if item.entries.iter().any(|entry| matches!(entry, ClipboardEntry::String(_))) {
        return None;
    }
    item.entries.iter().find_map(|entry| match entry {
        ClipboardEntry::Image(image) => Some(image),
        _ => None,
    })
}

/// The extension a pasted image is saved with, if it can be sent.
fn paste_extension(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Png => Some("png"),
        ImageFormat::Jpeg => Some("jpg"),
        ImageFormat::Webp => Some("webp"),
        ImageFormat::Gif => Some("gif"),
        _ => None,
    }
}

/// The error for files that can't be sent, by name.
fn unsupported_files(names: &[String]) -> String {
    let files = names.join(", ");
    format!("Can't send {files}: only JPEG, PNG and WebP photos and GIFs can be sent.")
}

/// Which of the pasted images `files` (path and age, if known) to remove:
/// those older than [`PASTED_MAX_AGE`], unless a message still sends them.
fn old_pastes(files: Vec<(PathBuf, Option<Duration>)>, pending: &HashSet<PathBuf>) -> Vec<PathBuf> {
    files
        .into_iter()
        .filter(|(path, age)| age.is_some_and(|age| age > PASTED_MAX_AGE) && !pending.contains(path))
        .map(|(path, _)| path)
        .collect()
}

/// Removes the old pasted images in `dir`; see [`old_pastes`].
fn remove_old_pastes(dir: &Path, pending: &HashSet<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    let files = entries
        .flatten()
        .map(|entry| {
            let modified = entry.metadata().and_then(|metadata| metadata.modified()).ok();
            (entry.path(), modified.and_then(|modified| now.duration_since(modified).ok()))
        })
        .collect();
    for path in old_pastes(files, pending) {
        let _ = std::fs::remove_file(path);
    }
}

impl FrillogramApp {
    /// Starts the downloads and conversions the selected chat's media needs.
    /// Call it when messages load, arrive or change, and when a job ends.
    pub(crate) fn fetch_media(&mut self, cx: &mut Context<Self>) {
        let (downloads, conversions) = self.media.jobs_to_start(&self.messages, &self.settings.media);
        for file_id in downloads {
            self.start_download(file_id, cx);
        }
        for conversion in conversions {
            self.start_conversion(conversion, cx);
        }
    }

    fn start_download(&mut self, file_id: i32, cx: &mut Context<Self>) {
        let Some(tg) = self.tg else { return };
        self.media.downloads.insert(file_id, Job::Running);
        cx.spawn(async move |this, cx| {
            let (job, bytes) = match tg.download(file_id).await {
                Ok(path) => {
                    // Whether a GIF is small enough to play as it is.
                    let stat = path.clone();
                    let bytes =
                        cx.background_executor().spawn(async move { std::fs::metadata(stat).ok().map(|m| m.len()) });
                    (Job::Ready(path), bytes.await)
                }
                Err(error) => (Job::Failed(error), None),
            };
            let _ = this.update(cx, |app, cx| {
                app.media.downloads.insert(file_id, job);
                if let Some(bytes) = bytes {
                    app.media.sizes.insert(file_id, bytes);
                }
                app.fetch_media(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn start_conversion(&mut self, conversion: Conversion, cx: &mut Context<Self>) {
        let Conversion { file_id, input, output, width } = conversion;
        self.media.conversions.insert(file_id, Job::Running);
        let ffmpeg = self.settings.media.ffmpeg.clone();
        let swept = self.media.cache_swept.clone();
        let task = on_own_thread("ffmpeg", move || {
            // Every conversion waits for this, so none of this session's is swept.
            swept.get_or_init(|| output.parent().into_iter().for_each(sweep_partials));
            convert(&ffmpeg, &input, &output, width)
        });
        cx.spawn(async move |this, cx| {
            let result =
                task.await.unwrap_or_else(|| Err(ConvertError::Failed("Can't start a thread for ffmpeg".into())));
            let _ = this.update(cx, |app, cx| {
                let job = match result {
                    Ok(path) => Job::Ready(path),
                    Err(ConvertError::Failed(error)) => Job::Failed(error),
                    // Most likely ffmpeg isn't installed: GIFs fall back to
                    // stills, nothing more is downloaded for them, and the
                    // user hears why once.
                    Err(ConvertError::CantRun(error)) => {
                        if !std::mem::replace(&mut app.media.ffmpeg_missing, true) {
                            app.show_error(
                                format!(
                                    "{error}, so most GIFs show as stills. Install ffmpeg, or set [media] ffmpeg to \
                                     its path, or to \"\" to stop converting."
                                ),
                                None,
                            );
                        }
                        Job::Failed(error)
                    }
                };
                app.media.conversions.insert(file_id, job);
                app.fetch_media(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Handles a click on the media of the message `message` (chat and id).
    fn media_click(&mut self, click: Click, message: (i64, i64), cx: &mut Context<Self>) {
        match click {
            Click::Nothing => {}
            Click::Download(file_id) => {
                self.media.downloads.remove(&file_id);
                self.start_download(file_id, cx);
            }
            Click::Play(file_id) => {
                self.media.play_requested.insert(file_id);
                // A download that failed earlier is retried.
                if matches!(self.media.download(file_id), Some(Job::Failed(_))) {
                    self.media.downloads.remove(&file_id);
                }
                // A click doesn't wait behind the downloads already queued.
                if self.media.download(file_id).is_none() {
                    self.start_download(file_id, cx);
                }
                self.fetch_media(cx);
            }
            Click::Open(file_id) => {
                if self.media.begin_open(file_id) {
                    self.open_file(file_id, cx);
                }
            }
            Click::Reveal => {
                self.media.revealed.insert(message);
                self.fetch_media(cx);
            }
        }
        cx.notify();
    }

    /// Says so, instead of doing nothing, when an image is sent in
    /// `--preview`, which has no Telegram to send it to.
    fn refuse_images_in_preview(&mut self, cx: &mut Context<Self>) -> bool {
        if self.preview {
            self.show_error("Images aren't sent in --preview; only text is added to the window.".into(), None);
            cx.notify();
        }
        self.preview
    }

    /// Shows an error about `chat_id`'s media, or a general one without it.
    fn media_error(&mut self, chat_id: Option<i64>, error: String) {
        match chat_id {
            Some(chat_id) => self.chat_error(chat_id, error),
            None => self.show_error(error, None),
        }
    }

    /// Opens a file with the opener, downloading it first if it isn't yet.
    /// The file stays in `opening` until the opener started.
    fn open_file(&mut self, file_id: i32, cx: &mut Context<Self>) {
        let Some(tg) = self.tg else {
            self.media.opening.remove(&file_id);
            return;
        };
        let open_command = self.settings.media.open_command.clone();
        let chat_id = self.selected_chat;
        cx.spawn(async move |this, cx| {
            // Returns right away for a file that's downloaded already.
            let (opener, started) = match tg.download(file_id).await {
                Ok(path) if !is_openable(&path) => {
                    (None, Err("Won't open this file: it isn't a photo, GIF or video.".to_string()))
                }
                Ok(path) => {
                    let opener = open_path(&open_command, &path);
                    let started = opener.recv().await.unwrap_or_else(|_| Err("Can't start the opener.".into()));
                    (Some(opener), started)
                }
                Err(error) => (None, Err(format!("Can't download the file: {error}"))),
            };
            let _ = this.update(cx, |app, cx| {
                app.media.opening.remove(&file_id);
                if let Err(error) = started {
                    app.media_error(chat_id, error);
                }
                cx.notify();
            });
            // A later failure, once the opener exits.
            if let Some(opener) = opener
                && let Ok(Err(error)) = opener.recv().await
            {
                let _ = this.update(cx, |app, cx| {
                    app.media_error(chat_id, error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Where a send from the main or thread composer goes, or `None` if it's
    /// refused, with the error shown.
    fn send_target(&mut self, in_thread: bool, cx: &mut Context<Self>) -> Option<SendTarget> {
        let chat_id = self.selected_chat?;
        let reply_to = self.reply_target(in_thread, chat_id, cx)?;
        Some(SendTarget { chat_id, in_thread, reply_to })
    }

    /// Asks for images to send from the main composer or the thread's.
    fn pick_files(&mut self, in_thread: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Decided now: the picker doesn't block the app, and the user may go
        // to another chat or thread before closing it. The files still go
        // where Attach was clicked.
        if self.refuse_images_in_preview(cx) {
            return;
        }
        let Some(target) = self.send_target(in_thread, cx) else { return };
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Send".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let paths = match paths.await {
                Ok(Ok(Some(paths))) => paths,
                // Canceled.
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |app, cx| {
                        app.show_error(format!("Can't open the file picker: {error}"), None);
                        cx.notify();
                    });
                    return;
                }
            };
            let _ = this.update_in(cx, |app, window, cx| app.send_files_to(target, paths, false, window, cx));
        })
        .detach();
    }

    /// Sends image files to the selected chat, or as replies in the open thread.
    pub(crate) fn send_files(
        &mut self,
        paths: Vec<PathBuf>,
        in_thread: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.refuse_images_in_preview(cx) {
            return;
        }
        if let Some(target) = self.send_target(in_thread, cx) {
            self.send_files_to(target, paths, false, window, cx);
        }
    }

    /// Sends image files to `target`. The text of its composer, if that
    /// still belongs to it, goes with the first one that's sent as its
    /// caption. It leaves the composer right away, so Enter can't send it
    /// again meanwhile, and comes back if no file was sent. `pasted` files
    /// are Frillogram's own, and removed once they're no longer needed.
    fn send_files_to(
        &mut self,
        target: SendTarget,
        paths: Vec<PathBuf>,
        pasted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tg) = self.tg else { return };
        let mut files = Vec::new();
        let mut unsupported = Vec::new();
        for path in paths {
            match Upload::for_path(&path) {
                Some(upload) => files.push((path, upload)),
                None => unsupported.push(
                    path.file_name()
                        .map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned()),
                ),
            }
        }
        if !unsupported.is_empty() {
            self.chat_error(target.chat_id, unsupported_files(&unsupported));
            cx.notify();
        }
        if files.is_empty() {
            return;
        }
        let thread_open = reply_thread_open(target.reply_to, self.open_thread, &self.threads, &self.messages);
        let mut caption = None;
        if takes_caption(target, self.selected_chat, thread_open) {
            let input = if target.in_thread { self.thread_composer.clone() } else { self.composer.clone() };
            let text = input.read(cx).value().trim().to_string();
            input.update(cx, |input, cx| {
                if !text.is_empty() {
                    input.set_value("", window, cx);
                }
                // Clicking Attach moved focus to it.
                input.focus(window, cx);
            });
            caption = (!text.is_empty()).then_some((input, text));
        }
        let to_measure: Vec<PathBuf> = files.iter().map(|(path, _)| path.clone()).collect();
        let sizes = cx.background_spawn(async move { to_measure.iter().map(|path| image_size(path)).collect() });

        cx.spawn_in(window, async move |this, cx| {
            let sizes: Vec<(u32, u32)> = sizes.await;
            for ((path, upload), size) in files.into_iter().zip(sizes) {
                let text = caption.as_ref().map(|(_, text)| text.clone()).unwrap_or_default();
                let result = tg.send_file(target.chat_id, path.clone(), upload, size, text, target.reply_to).await;
                let _ = this.update(cx, |app, cx| match result {
                    Ok(message_id) => {
                        caption = None;
                        if pasted {
                            app.media.pastes.insert((target.chat_id, message_id), path);
                        }
                    }
                    Err(error) => {
                        if pasted {
                            remove_in_background(path, cx);
                        }
                        app.chat_error(target.chat_id, error);
                        cx.notify();
                    }
                });
            }
            let Some((input, text)) = caption else { return };
            let _ = this.update_in(cx, |app, window, cx| {
                let selected = app.selected_chat == Some(target.chat_id);
                let thread_open = reply_thread_open(target.reply_to, app.open_thread, &app.threads, &app.messages);
                match restore_target(selected, target.in_thread, thread_open) {
                    Restore::Composer => restore_failed_text(&input, text, window, cx),
                    Restore::Drop => {}
                    Restore::Draft => park_draft(&mut app.drafts, target.chat_id, &text),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Sends an image pasted into a composer, saved as a file first since
    /// TDLib uploads from a path.
    fn send_pasted_image(&mut self, image: &Image, in_thread: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.refuse_images_in_preview(cx) {
            return;
        }
        let Some(extension) = paste_extension(image.format) else {
            self.show_error(
                "Can't send this kind of image: paste a PNG, JPEG, WebP or GIF.".into(),
                self.selected_chat,
            );
            cx.notify();
            return;
        };
        let Some(dir) = self.media.data_dir.as_ref().map(|dir| dir.join(PASTED_DIR)) else { return };
        let Some(target) = self.send_target(in_thread, cx) else { return };
        let pending: HashSet<PathBuf> = self.media.pastes.values().cloned().collect();
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = dir.join(format!("paste-{nanos}.{extension}"));
        let bytes = image.bytes.clone();
        let saved = cx.background_spawn(async move {
            remove_old_pastes(&dir, &pending);
            std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, bytes)).map(|()| path)
        });
        cx.spawn_in(window, async move |this, cx| {
            let saved = saved.await;
            let _ = this.update_in(cx, |app, window, cx| match saved {
                Ok(path) => app.send_files_to(target, vec![path], true, window, cx),
                Err(error) => {
                    app.chat_error(target.chat_id, format!("Can't save the pasted image: {error}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Removes the pasted image of a message that was sent or failed: TDLib
    /// has no more use for it then.
    pub(crate) fn paste_settled(&mut self, chat_id: i64, old_id: i64, cx: &mut Context<Self>) {
        if let Some(path) = self.media.pastes.remove(&(chat_id, old_id)) {
            remove_in_background(path, cx);
        }
    }

    /// A photo or GIF of `message`, at its final size from the first frame,
    /// so the message list doesn't jump when it loads.
    pub(crate) fn render_media(&self, message: &Msg, cx: &mut Context<Self>) -> Option<AnyElement> {
        let media = message.media.as_ref()?;
        let settings = &self.settings.media;
        let (width, height) = media.size();
        let (width, height) = fit(width, height, settings.max_width, settings.max_height);
        let view = self.media.message_view(message, settings)?;
        let (muted, muted_fg, radius) = (cx.theme().muted, cx.theme().muted_foreground, cx.theme().radius_lg);
        // Badges over an image use the palette, softened, not black and white.
        let (overlay, overlay_fg) = (cx.theme().popover.opacity(0.88), cx.theme().foreground);
        let minithumbnail =
            media.minithumbnail().map(|bytes| Arc::new(Image::from_bytes(ImageFormat::Jpeg, bytes.to_vec())));

        let load_hint = matches!((&view.visual, view.click), (Visual::Placeholder, Click::Download(_)));
        let content = match view.visual {
            Visual::Image(path) => {
                let loading = minithumbnail.clone();
                // A converted GIF GPUI can't decode is deleted, once, so the
                // next session converts it again instead of failing forever.
                let discard = self.media.is_converted(&path).then(|| (path.clone(), self.media.discarded.clone()));
                img(path)
                    // GPUI keeps an image's frame and loading state by id, so
                    // without one a GIF never advances past its first frame.
                    .id(("media-image", message.id as u64))
                    .size_full()
                    .rounded(radius)
                    .object_fit(ObjectFit::Cover)
                    .with_loading(move || render_minithumbnail(loading.clone(), radius))
                    .with_fallback(move || {
                        if let Some((path, discarded)) = &discard
                            && discarded.borrow_mut().insert(path.clone())
                        {
                            let _ = std::fs::remove_file(path);
                        }
                        render_media_label("Can't show this image", muted_fg)
                    })
                    .into_any_element()
            }
            Visual::Placeholder => render_minithumbnail(minithumbnail, radius),
            Visual::Failed(error) => render_media_label(&format!("Couldn't load: {error}"), muted_fg),
        };
        let click = view.click;
        let key = (message.chat_id, message.id);
        Some(
            div()
                .id(("media", message.id as u64))
                .relative()
                .flex_none()
                .w(px(width as f32))
                .h(px(height as f32))
                .rounded(radius)
                .bg(muted)
                .when(click != Click::Nothing, |media| {
                    media.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.media_click(click, key, cx)))
                })
                .child(content)
                .when(load_hint, |media| {
                    media.child(div().absolute().inset_0().child(render_media_label("Click to load", muted_fg)))
                })
                .children(view.badge.map(|badge| render_badge(badge, overlay, overlay_fg)))
                // Telegram's "restrict saving content": it never leaves the app.
                .when(!message.can_be_saved, |media| {
                    media.child(
                        div()
                            .absolute()
                            .top_2()
                            .right_2()
                            .p_0p5()
                            .rounded_sm()
                            .bg(overlay)
                            .child(Icon::new(Lucide::Lock).xsmall().text_color(overlay_fg)),
                    )
                })
                .when(view.loading, |media| {
                    media.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().p_1().rounded_full().bg(overlay).child(Spinner::new().color(overlay_fg))),
                    )
                })
                .into_any_element(),
        )
    }

    /// A composer in a rounded box with its Attach button. Pasting an image
    /// into it sends it.
    pub(crate) fn render_composer(&self, in_thread: bool, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input = if in_thread { &self.thread_composer } else { &self.composer };
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        let this = cx.weak_entity();
        // The box draws the border and focus ring, so the text area draws none.
        let textarea = Textarea::new(input).appearance(false).on_paste(move |item, window, cx| {
            let Some(image) = pasted_image(item) else { return false };
            this.update(cx, |app, cx| app.send_pasted_image(image, in_thread, window, cx)).is_ok()
        });
        let has_text = !input.read(cx).value().trim().is_empty();
        let theme = cx.theme();
        let (border, surface, radius) = (theme.input, theme.popover, theme.radius_lg * 1.5);
        h_flex()
            .items_end()
            .gap_1()
            .p_1()
            .rounded(radius)
            .border_1()
            .border_color(border)
            .bg(surface)
            .shadow_xs()
            .when(focused, |composer| composer.focus_ring_style(window, cx))
            .child(
                Button::new(if in_thread { "attach-in-thread" } else { "attach" })
                    .ghost()
                    .small()
                    .icon(Lucide::Paperclip)
                    .tooltip("Attach photos or GIFs; you can also drop or paste them")
                    // Lines it up with the text's last line.
                    .mb(px(4.))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick_files(in_thread, window, cx))),
            )
            .child(div().flex_1().min_w_0().child(textarea))
            .child(
                // Enter sends too; the button makes that discoverable and
                // lights up once there's something to send.
                Button::new(if in_thread { "send-in-thread" } else { "send" })
                    .when(has_text, |button| button.primary())
                    .when(!has_text, |button| button.ghost())
                    .small()
                    .icon(Lucide::ArrowUp)
                    .tooltip("Send (Enter; Shift+Enter for a new line)")
                    .disabled(!has_text)
                    .mb(px(4.))
                    .mr(px(2.))
                    .on_click(cx.listener(move |this, _, window, cx| this.send_from_composer(in_thread, window, cx))),
            )
    }
}

/// Deletes `path` off the UI thread.
fn remove_in_background(path: PathBuf, cx: &mut App) {
    cx.background_spawn(async move {
        let _ = std::fs::remove_file(path);
    })
    .detach();
}

/// The blurry minithumbnail filling its box, or nothing over the box's color.
fn render_minithumbnail(minithumbnail: Option<Arc<Image>>, radius: Pixels) -> AnyElement {
    match minithumbnail {
        Some(image) => img(image).size_full().rounded(radius).object_fit(ObjectFit::Cover).into_any_element(),
        None => div().into_any_element(),
    }
}

/// A short muted text centered in the media box.
fn render_media_label(text: &str, color: Hsla) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_2()
        .text_xs()
        .text_center()
        .text_color(color)
        .child(text.to_string())
        .into_any_element()
}

fn render_badge(badge: Badge, background: Hsla, foreground: Hsla) -> impl IntoElement {
    div()
        .absolute()
        .left_2()
        .bottom_2()
        .px_1p5()
        .rounded_sm()
        .bg(background)
        .text_color(foreground)
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .child(match badge {
            Badge::Gif => "GIF",
            Badge::Play => "▶ GIF",
            Badge::Spoiler => "Spoiler",
        })
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's glob import brings its own `test` attribute.
    use std::{
        collections::HashSet,
        ffi::OsString,
        path::{Path, PathBuf},
        time::Duration,
    };

    use ::image::metadata::Orientation;
    use frillogram_core::{
        Animation, Media, Msg, Photo, SendState, SenderId, Thumbnail, ThumbnailFormat, config::MediaSettings,
    };
    use gpui_kit::{ClipboardEntry, ClipboardItem, ClipboardString, Image, ImageFormat};

    use super::{
        Badge, Click, Conversion, ConvertError, Job, MediaState, PASTED_MAX_AGE, SendTarget, Visual, convert, fit,
        gif_args, gif_cache_path, gif_fits_in_memory, has_gif_header, image_size, is_gif, is_openable, old_pastes,
        opener, oriented, partial_path, paste_extension, pasted_image, sweep_partials, takes_caption,
        unsupported_files,
    };

    /// A fresh directory under the system temp dir, named after the test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("frillogram-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn images_fit_both_limits_and_keep_their_aspect_ratio() {
        assert_eq!(fit(800, 600, 400, 320), (400, 300));
        assert_eq!(fit(600, 800, 400, 320), (240, 320));
        assert_eq!(fit(1000, 1000, 400, 320), (320, 320));
        // Small images aren't enlarged.
        assert_eq!(fit(200, 100, 400, 320), (200, 100));
        assert_eq!(fit(1, 1, 400, 320), (1, 1));
    }

    #[test]
    fn extreme_sizes_never_vanish_or_overflow() {
        assert_eq!(fit(100_000, 10, 400, 320), (400, 1));
        assert_eq!(fit(10, 100_000, 400, 320), (1, 320));
        assert_eq!(fit(u32::MAX, u32::MAX, 400, 320), (320, 320));
        // Unknown sizes get a square, and zero limits don't divide by zero.
        assert_eq!(fit(0, 600, 400, 320), (320, 320));
        assert_eq!(fit(800, 0, 400, 320), (320, 320));
        assert_eq!(fit(0, 0, 0, 0), (1, 1));
        assert_eq!(fit(800, 600, 0, 0), (1, 1));
    }

    #[test]
    fn ffmpeg_gets_a_palette_a_low_frame_rate_and_the_display_width() {
        let args = gif_args(Path::new("/in/cat.mp4"), Path::new("/out/cat.gif.part"), 400);
        let expected: Vec<OsString> = [
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-i",
            "/in/cat.mp4",
            "-an",
            "-vf",
            "fps=15,scale=400:-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse",
            "-loop",
            "0",
            "-f",
            "gif",
            "/out/cat.gif.part",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert_eq!(args, expected);
        // A path with spaces stays one argument.
        let args = gif_args(Path::new("/my files/a b.mp4"), Path::new("/o"), 10);
        assert!(args.contains(&OsString::from("/my files/a b.mp4")));
    }

    #[test]
    fn missing_ffmpeg_is_reported_as_such() {
        let dir = temp_dir("convert");
        let output = dir.join("x.gif");
        let error = convert("frillogram-no-such-ffmpeg", Path::new("/nonexistent.mp4"), &output, 100).unwrap_err();
        assert!(matches!(error, ConvertError::CantRun(ref message) if message.contains("frillogram-no-such-ffmpeg")));
        // Nothing is left behind, not even a partial file.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_a_gif_counts_as_a_cached_gif() {
        assert!(is_gif(b"GIF89a\x01\x00"));
        assert!(is_gif(b"GIF87a"));
        assert!(!is_gif(b""));
        assert!(!is_gif(b"GIF"));
        assert!(!is_gif(b"\x89PNG\r\n"));
        let dir = temp_dir("header");
        let (gif, empty, other) = (dir.join("a.gif"), dir.join("b.gif"), dir.join("c.gif"));
        std::fs::write(&gif, b"GIF89a....").unwrap();
        std::fs::write(&empty, b"").unwrap();
        std::fs::write(&other, b"<html>").unwrap();
        assert!(has_gif_header(&gif));
        assert!(!has_gif_header(&empty) && !has_gif_header(&other) && !has_gif_header(&dir.join("none.gif")));
        // A cached file that's no GIF is converted again, not returned.
        let error = convert("frillogram-no-such-ffmpeg", Path::new("/in.mp4"), &other, 100).unwrap_err();
        assert!(matches!(error, ConvertError::CantRun(_)));
        assert_eq!(convert("frillogram-no-such-ffmpeg", Path::new("/in.mp4"), &gif, 100), Ok(gif));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn partial_files_are_unique_and_swept() {
        let output = Path::new("/cache/AgAD-400.gif");
        let partial = partial_path(output, 42, 7);
        assert_eq!(partial, Path::new("/cache/AgAD-400.gif.42-7.part"));
        assert_ne!(partial, partial_path(output, 43, 7));
        assert_ne!(partial, partial_path(output, 42, 8));
        let dir = temp_dir("sweep");
        for name in ["a.gif", "a.gif.part", "a.gif.1-2.part"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        sweep_partials(&dir);
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, [OsString::from("a.gif")]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn gif_cache_is_keyed_by_unique_id_and_width() {
        let dir = Path::new("/data/tdlib");
        assert_eq!(gif_cache_path(dir, "AgAD-x_9", 400), Some(PathBuf::from("/data/tdlib/gif-cache/AgAD-x_9-400.gif")));
        assert_eq!(gif_cache_path(dir, "", 400), None);
        assert_eq!(gif_cache_path(dir, "../../etc", 400), None);
        assert!(state().is_converted(Path::new("/data/tdlib/gif-cache/AgAD-400.gif")));
        assert!(!state().is_converted(Path::new("/data/tdlib/files/animations/a.gif")));
    }

    #[test]
    fn opener_splits_like_a_shell_and_appends_the_path() {
        let path = Path::new("/files/my photo.jpg");
        let (program, args) = opener("open -a  Preview", path);
        assert_eq!(program, "open");
        assert_eq!(args, ["-a", "Preview", "/files/my photo.jpg"].map(OsString::from));
        let (_, args) = opener("open -a 'Adobe Photoshop 2024'", path);
        assert_eq!(args, ["-a", "Adobe Photoshop 2024", "/files/my photo.jpg"].map(OsString::from));
        let (program, args) = opener("", path);
        assert_eq!(program, if cfg!(target_os = "macos") { "open" } else { "xdg-open" });
        assert_eq!(args, [OsString::from("/files/my photo.jpg")]);
        assert_eq!(opener(" feh ", path).0, "feh");
    }

    #[test]
    fn only_a_clipboard_without_text_pastes_an_image() {
        let image = || ClipboardEntry::Image(Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]));
        let item = ClipboardItem { entries: vec![image()] };
        assert_eq!(pasted_image(&item).map(|image| image.format), Some(ImageFormat::Png));
        let item = ClipboardItem { entries: vec![ClipboardEntry::String(ClipboardString::new("hi".into())), image()] };
        assert!(pasted_image(&item).is_none());
        assert!(pasted_image(&ClipboardItem::new_string("hi".into())).is_none());
        assert_eq!(paste_extension(ImageFormat::Jpeg), Some("jpg"));
        assert_eq!(paste_extension(ImageFormat::Gif), Some("gif"));
        assert_eq!(paste_extension(ImageFormat::Tiff), None);
    }

    #[test]
    fn unsupported_files_are_named() {
        let error = unsupported_files(&["a.mp4".into(), "b.heic".into()]);
        assert_eq!(error, "Can't send a.mp4, b.heic: only JPEG, PNG and WebP photos and GIFs can be sent.");
    }

    #[test]
    fn only_old_settled_pastes_are_removed() {
        let day = Duration::from_secs(24 * 60 * 60);
        let pending = HashSet::from([PathBuf::from("/p/pending.png")]);
        let files = vec![
            (PathBuf::from("/p/new.png"), Some(day)),
            (PathBuf::from("/p/old.png"), Some(PASTED_MAX_AGE + day)),
            (PathBuf::from("/p/pending.png"), Some(PASTED_MAX_AGE + day)),
            (PathBuf::from("/p/unknown.png"), None),
        ];
        assert_eq!(old_pastes(files, &pending), [PathBuf::from("/p/old.png")]);
        // A paste waiting a day or more for the network is kept.
        assert!(PASTED_MAX_AGE >= 30 * day);
    }

    #[test]
    fn sent_images_get_their_shown_size() {
        let dir = temp_dir("size");
        let path = dir.join("wide.png");
        ::image::RgbImage::new(3, 2).save(&path).unwrap();
        assert_eq!(image_size(&path), (3, 2));
        std::fs::write(dir.join("junk.png"), b"not an image").unwrap();
        assert_eq!(image_size(&dir.join("junk.png")), (0, 0));
        assert_eq!(image_size(&dir.join("missing.png")), (0, 0));
        // A photo taken sideways is sent at the size it's shown.
        assert_eq!(oriented((4000, 3000), Orientation::Rotate90), (3000, 4000));
        assert_eq!(oriented((4000, 3000), Orientation::Rotate270FlipH), (3000, 4000));
        assert_eq!(oriented((4000, 3000), Orientation::Rotate180), (4000, 3000));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn attachments_take_the_caption_only_from_their_own_composer() {
        let main = SendTarget { chat_id: 1, in_thread: false, reply_to: None };
        let thread = SendTarget { chat_id: 1, in_thread: true, reply_to: Some(10) };
        assert!(takes_caption(main, Some(1), false));
        assert!(takes_caption(thread, Some(1), true));
        // The user went to another chat, or the thread closed or changed.
        assert!(!takes_caption(main, Some(2), false));
        assert!(!takes_caption(main, None, false));
        assert!(!takes_caption(thread, Some(1), false));
        assert!(!takes_caption(thread, Some(2), true));
    }

    #[test]
    fn big_gifs_dont_play_as_they_are() {
        assert!(gif_fits_in_memory(2 * 1024 * 1024, 480, 270));
        assert!(gif_fits_in_memory(8 * 1024 * 1024, 1920, 1080));
        assert!(!gif_fits_in_memory(8 * 1024 * 1024 + 1, 480, 270));
        assert!(!gif_fits_in_memory(1024, 4000, 4000));
        assert!(!gif_fits_in_memory(u64::MAX, u32::MAX, u32::MAX));
    }

    fn photo() -> Media {
        Media::Photo(Photo { file_id: 1, width: 800, height: 600, minithumbnail: None, has_spoiler: false })
    }

    fn gif(mime_type: &str, thumbnail: Option<ThumbnailFormat>) -> Media {
        Media::Animation(Animation {
            file_id: 2,
            unique_id: "AgADuniq".into(),
            width: 480,
            height: 270,
            mime_type: mime_type.into(),
            thumbnail: thumbnail.map(|format| Thumbnail { file_id: 3, format }),
            minithumbnail: None,
            has_spoiler: false,
        })
    }

    fn message(media: Media, can_be_saved: bool) -> Msg {
        Msg {
            id: 50,
            chat_id: 7,
            sender: SenderId::User(1),
            date: 0,
            text: String::new(),
            media: Some(media),
            reply_to: None,
            outgoing: false,
            send_state: SendState::Sent,
            can_be_saved,
        }
    }

    fn state() -> MediaState {
        MediaState::new(Some("/data/tdlib".into()))
    }

    fn ready(path: &str) -> Job {
        Job::Ready(path.into())
    }

    fn no_ffmpeg() -> MediaSettings {
        MediaSettings { ffmpeg: String::new(), ..MediaSettings::default() }
    }

    #[test]
    fn photos_load_then_open() {
        let settings = MediaSettings::default();
        let mut state = state();
        assert_eq!(state.wanted_downloads(&photo(), &settings), [1]);
        let view = state.view(&photo(), &settings);
        assert_eq!((view.visual, view.click), (Visual::Placeholder, Click::Download(1)));
        state.downloads.insert(1, Job::Running);
        assert_eq!(state.view(&photo(), &settings).click, Click::Nothing);
        state.downloads.insert(1, ready("/f/1.jpg"));
        let view = state.view(&photo(), &settings);
        assert_eq!((view.visual, view.click), (Visual::Image("/f/1.jpg".into()), Click::Open(1)));
        state.downloads.insert(1, Job::Failed("Nope".into()));
        let view = state.view(&photo(), &settings);
        assert_eq!((view.visual, view.click), (Visual::Failed("Nope".into()), Click::Download(1)));
        // Without autoload, nothing downloads until clicked.
        let settings = MediaSettings { autoload: false, ..MediaSettings::default() };
        assert!(state.wanted_downloads(&photo(), &settings).is_empty());
    }

    #[test]
    fn small_real_gifs_play_without_ffmpeg() {
        let settings = no_ffmpeg();
        let mut state = state();
        let media = gif("image/gif", Some(ThumbnailFormat::Jpeg));
        assert_eq!(state.wanted_downloads(&media, &settings), [3, 2]);
        state.downloads.insert(2, ready("/f/2.gif"));
        state.downloads.insert(3, ready("/f/3.jpg"));
        assert_eq!(state.wanted_conversion(&media, &settings), None);
        // Until its size is known, it's the still.
        assert_eq!(state.view(&media, &settings).visual, Visual::Image("/f/3.jpg".into()));
        state.sizes.insert(2, 500_000);
        let view = state.view(&media, &settings);
        assert_eq!((view.visual, view.badge, view.click), (Visual::Image("/f/2.gif".into()), None, Click::Open(2)));
        // Too big to keep every frame of, it stays a still that opens.
        state.sizes.insert(2, 50 * 1024 * 1024);
        let view = state.view(&media, &settings);
        assert_eq!(
            (view.visual, view.badge, view.click),
            (Visual::Image("/f/3.jpg".into()), Some(Badge::Gif), Click::Open(2))
        );
    }

    #[test]
    fn real_gifs_are_shrunk_by_ffmpeg() {
        let settings = MediaSettings::default();
        let mut state = state();
        let media = gif("image/gif", Some(ThumbnailFormat::Jpeg));
        state.downloads.insert(2, ready("/f/2.gif"));
        state.sizes.insert(2, 50 * 1024 * 1024);
        let conversion = state.wanted_conversion(&media, &settings).unwrap();
        assert_eq!((conversion.input, conversion.width), ("/f/2.gif".into(), 400));
        state.conversions.insert(2, ready("/c/2.gif"));
        assert_eq!(state.view(&media, &settings).visual, Visual::Image("/c/2.gif".into()));
    }

    #[test]
    fn mp4_gifs_convert_once_downloaded() {
        let settings = MediaSettings::default();
        let mut state = state();
        let media = gif("video/mp4", Some(ThumbnailFormat::Jpeg));
        state.downloads.insert(3, ready("/f/3.jpg"));
        assert_eq!(state.view(&media, &settings).visual, Visual::Image("/f/3.jpg".into()));
        state.downloads.insert(2, ready("/f/2.mp4"));
        let conversion = Conversion {
            file_id: 2,
            input: "/f/2.mp4".into(),
            output: "/data/tdlib/gif-cache/AgADuniq-400.gif".into(),
            width: 400,
        };
        assert_eq!(state.wanted_conversion(&media, &settings), Some(conversion));
        state.conversions.insert(2, ready("/c/2.gif"));
        assert_eq!(state.wanted_conversion(&media, &settings), None);
        let view = state.view(&media, &settings);
        assert_eq!((view.visual, view.click), (Visual::Image("/c/2.gif".into()), Click::Open(2)));
        // A failed conversion shows the still, which opens the MP4.
        state.conversions.insert(2, Job::Failed("bad".into()));
        let view = state.view(&media, &settings);
        assert_eq!(
            (view.visual, view.badge, view.click),
            (Visual::Image("/f/3.jpg".into()), Some(Badge::Gif), Click::Open(2))
        );
    }

    #[test]
    fn mp4_gifs_without_ffmpeg_are_stills() {
        let settings = no_ffmpeg();
        let state = state();
        let media = gif("video/mp4", Some(ThumbnailFormat::Mpeg4));
        // Neither the MP4 nor an MP4 thumbnail is any use inline.
        assert!(state.wanted_downloads(&media, &settings).is_empty());
        let view = state.view(&media, &settings);
        assert_eq!((view.visual, view.badge, view.click), (Visual::Placeholder, Some(Badge::Gif), Click::Open(2)));
        // Nor without a place to cache the GIF.
        let view = MediaState::new(None).view(&media, &MediaSettings::default());
        assert_eq!(view.badge, Some(Badge::Gif));
    }

    #[test]
    fn downloads_start_newest_first_a_few_at_a_time() {
        let photo = |file_id| {
            let media = Media::Photo(Photo { file_id, width: 8, height: 8, minithumbnail: None, has_spoiler: false });
            message(media, true)
        };
        let messages: Vec<Msg> = (1..=10).map(photo).collect();
        let settings = MediaSettings::default();
        let mut state = state();
        assert_eq!(state.jobs_to_start(&messages, &settings).0, [10, 9, 8, 7]);
        state.downloads.extend([(10, Job::Running), (9, Job::Running), (8, ready("/f/8.jpg"))]);
        assert_eq!(state.jobs_to_start(&messages, &settings).0, [7, 6]);
    }

    #[test]
    fn animations_that_arent_gifs_or_videos_never_open() {
        let view = state().view(&gif("application/x-sh", None), &MediaSettings::default());
        assert_eq!((view.badge, view.click), (Some(Badge::Gif), Click::Nothing));
        assert!(is_openable(Path::new("/f/photos/1.JPG")) && is_openable(Path::new("/f/animations/2.mp4")));
        for path in ["/f/animations/x.command", "/f/animations/x.jar", "/f/animations/x.terminal", "/f/x"] {
            assert!(!is_openable(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn once_ffmpeg_cant_run_gifs_are_stills_and_only_the_still_downloads() {
        let settings = MediaSettings::default();
        let mut state = state();
        let mp4 = gif("video/mp4", Some(ThumbnailFormat::Jpeg));
        assert_eq!(state.wanted_downloads(&mp4, &settings), [3, 2]);
        state.ffmpeg_missing = true;
        assert_eq!(state.wanted_downloads(&mp4, &settings), [3]);
        let view = state.view(&mp4, &settings);
        assert_eq!((view.badge, view.click), (Some(Badge::Gif), Click::Open(2)));
        // A real GIF plays as it is again, if it's small.
        let real = gif("image/gif", Some(ThumbnailFormat::Jpeg));
        state.downloads.insert(2, ready("/f/2.gif"));
        state.sizes.insert(2, 1000);
        assert_eq!(state.wanted_conversion(&real, &settings), None);
        assert_eq!(state.view(&real, &settings).visual, Visual::Image("/f/2.gif".into()));
    }

    #[test]
    fn without_autoplay_gifs_wait_for_a_click() {
        let settings = MediaSettings { autoplay_gifs: false, ..MediaSettings::default() };
        let mut state = state();
        let media = gif("video/mp4", Some(ThumbnailFormat::Jpeg));
        assert_eq!(state.wanted_downloads(&media, &settings), [3]);
        let view = state.view(&media, &settings);
        assert_eq!((view.badge, view.click), (Some(Badge::Play), Click::Play(2)));
        state.play_requested.insert(2);
        assert_eq!(state.wanted_downloads(&media, &settings), [3, 2]);
        assert_eq!(state.view(&media, &settings).badge, None);
        state.downloads.insert(2, ready("/f/2.mp4"));
        assert!(state.wanted_conversion(&media, &settings).is_some());
    }

    #[test]
    fn a_file_opens_once_until_its_opener_started() {
        let settings = MediaSettings::default();
        let mut state = state();
        state.downloads.insert(1, ready("/f/1.jpg"));
        assert!(state.begin_open(1));
        assert!(!state.begin_open(1));
        let view = state.view(&photo(), &settings);
        assert_eq!((view.click, view.loading), (Click::Nothing, true));
        state.opening.remove(&1);
        let view = state.view(&photo(), &settings);
        assert_eq!((view.click, view.loading), (Click::Open(1), false));
        assert!(state.begin_open(1));
    }

    #[test]
    fn spoilers_stay_covered_until_clicked() {
        let settings = MediaSettings::default();
        let mut state = state();
        let Media::Animation(animation) = gif("image/gif", Some(ThumbnailFormat::Jpeg)) else { unreachable!() };
        let covered = message(Media::Animation(Animation { has_spoiler: true, ..animation }), true);
        state.downloads.insert(2, ready("/f/2.gif"));
        state.sizes.insert(2, 1000);
        let view = state.message_view(&covered, &settings).unwrap();
        assert_eq!((view.visual, view.badge, view.click), (Visual::Placeholder, Some(Badge::Spoiler), Click::Reveal));
        // It still downloads meanwhile, so it shows at once when revealed.
        assert_eq!(state.wanted_downloads(covered.media.as_ref().unwrap(), &no_ffmpeg()), [3, 2]);
        state.revealed.insert((7, 50));
        let view = state.message_view(&covered, &no_ffmpeg()).unwrap();
        assert_eq!(view.visual, Visual::Image("/f/2.gif".into()));
        // Revealing is per message.
        let other = Msg { id: 51, ..covered };
        assert_eq!(state.message_view(&other, &settings).unwrap().click, Click::Reveal);
    }

    #[test]
    fn protected_media_never_opens_outside_the_app() {
        let settings = MediaSettings::default();
        let mut state = state();
        state.downloads.insert(1, ready("/f/1.jpg"));
        assert_eq!(state.message_view(&message(photo(), true), &settings).unwrap().click, Click::Open(1));
        let view = state.message_view(&message(photo(), false), &settings).unwrap();
        assert_eq!((view.visual, view.click), (Visual::Image("/f/1.jpg".into()), Click::Nothing));
        // Playing a GIF still works.
        let settings = MediaSettings { autoplay_gifs: false, ..MediaSettings::default() };
        let protected_gif = message(gif("video/mp4", Some(ThumbnailFormat::Jpeg)), false);
        assert_eq!(state.message_view(&protected_gif, &settings).unwrap().click, Click::Play(2));
        assert!(state.message_view(&Msg { media: None, ..protected_gif }, &settings).is_none());
    }
}
