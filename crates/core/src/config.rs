//! Frillogram's configuration: the user's `config.toml`, the environment
//! variables that override it, and the TDLib [`Config`] resolved from both.

use std::{
    ffi::OsString,
    fmt::Write as _,
    ops::RangeInclusive,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use chrono::{
    NaiveDate,
    format::{Item, StrftimeItems},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// `config.example.toml`, which lists every key with its default.
pub const EXAMPLE: &str = include_str!("../../../config.example.toml");

/// Everything `config.toml` can set. A new section is a field here, its
/// struct with a `validate`, and its lines in `config.example.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub telegram: TelegramSettings,
    pub appearance: Appearance,
    pub colors: Colors,
    pub layout: Layout,
    pub time: Time,
    pub chats: Chats,
    pub media: MediaSettings,
}

/// `[colors.light]` and `[colors.dark]`: the palettes of the two themes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Colors {
    pub light: Palette,
    pub dark: Palette,
}

impl Default for Colors {
    fn default() -> Self {
        Colors { light: Palette::light(), dark: Palette::dark() }
    }
}

/// The keys a palette in the file sets. A key left out takes its own
/// theme's default, so a partial `[colors.dark]` stays dark.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PaletteFile {
    background: Option<String>,
    sidebar: Option<String>,
    surface: Option<String>,
    foreground: Option<String>,
    muted: Option<String>,
    border: Option<String>,
    accent: Option<String>,
    danger: Option<String>,
    avatar_colors: Option<Vec<String>>,
    avatar_text: Option<String>,
}

impl PaletteFile {
    fn over(self, default: Palette) -> Palette {
        Palette {
            background: self.background.unwrap_or(default.background),
            sidebar: self.sidebar.unwrap_or(default.sidebar),
            surface: self.surface.unwrap_or(default.surface),
            foreground: self.foreground.unwrap_or(default.foreground),
            muted: self.muted.unwrap_or(default.muted),
            border: self.border.unwrap_or(default.border),
            accent: self.accent.unwrap_or(default.accent),
            danger: self.danger.unwrap_or(default.danger),
            avatar_colors: self.avatar_colors.unwrap_or(default.avatar_colors),
            avatar_text: self.avatar_text.unwrap_or(default.avatar_text),
        }
    }
}

impl<'de> Deserialize<'de> for Colors {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Default, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct ColorsFile {
            light: PaletteFile,
            dark: PaletteFile,
        }
        let file = ColorsFile::deserialize(deserializer)?;
        Ok(Colors { light: file.light.over(Palette::light()), dark: file.dark.over(Palette::dark()) })
    }
}

/// One theme's colors, each a validated `#rrggbb`. The defaults are soft
/// pastels without pure white or black, which are hard on sensitive eyes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Palette {
    pub background: String,
    pub sidebar: String,
    pub surface: String,
    pub foreground: String,
    pub muted: String,
    pub border: String,
    pub accent: String,
    pub danger: String,
    pub avatar_colors: Vec<String>,
    pub avatar_text: String,
}

impl Palette {
    fn light() -> Self {
        Palette {
            background: "#f3eee6".into(),
            sidebar: "#e9e3f0".into(),
            surface: "#f8f4ee".into(),
            foreground: "#3d3a4b".into(),
            muted: "#77718a".into(),
            border: "#dcd4cb".into(),
            accent: "#c5b6f2".into(),
            danger: "#b0505b".into(),
            avatar_colors: ["#f5c6aa", "#f1b9c7", "#cdbff3", "#b6dfc6", "#b4d3f1", "#f2dea4", "#a9dbd7"]
                .map(String::from)
                .into(),
            avatar_text: "#3d3a4b".into(),
        }
    }

    fn dark() -> Self {
        Palette {
            background: "#2c2b36".into(),
            sidebar: "#26252f".into(),
            surface: "#35343f".into(),
            foreground: "#d8d2e3".into(),
            muted: "#9d97ae".into(),
            border: "#403e4d".into(),
            accent: "#b3a4e8".into(),
            danger: "#e3949c".into(),
            avatar_colors: ["#dfb197", "#dba5b3", "#b8abe0", "#a2cbb2", "#a0bfdd", "#dcc993", "#96c6c2"]
                .map(String::from)
                .into(),
            avatar_text: "#2c2b36".into(),
        }
    }

    fn validate(&self, section: &str) -> Result<(), Invalid> {
        let colors = [
            ("background", &self.background),
            ("sidebar", &self.sidebar),
            ("surface", &self.surface),
            ("foreground", &self.foreground),
            ("muted", &self.muted),
            ("border", &self.border),
            ("accent", &self.accent),
            ("danger", &self.danger),
            ("avatar_text", &self.avatar_text),
        ];
        for (key, color) in colors {
            if !is_hex_color(color) {
                return Err(invalid(
                    section,
                    key,
                    format!("{color:?} isn't a color. Use \"#rrggbb\", e.g. \"#c5b6f2\"."),
                ));
            }
        }
        if self.avatar_colors.is_empty() {
            return Err(invalid(section, "avatar_colors", "is empty. List at least one \"#rrggbb\" color."));
        }
        if let Some(color) = self.avatar_colors.iter().find(|color| !is_hex_color(color)) {
            return Err(invalid(
                section,
                "avatar_colors",
                format!("{color:?} isn't a color. List colors as \"#rrggbb\", e.g. [\"#f5c6aa\", \"#b4d3f1\"]."),
            ));
        }
        Ok(())
    }
}

/// `[telegram]`, as written in the file. [`Config`] is what it resolves to.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelegramSettings {
    /// 0 means unset.
    pub api_id: i32,
    /// A command line run with `sh -c` that prints the `api_hash`; "" means unset.
    pub api_hash_command: String,
    /// "" means the default data directory.
    pub data_dir: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub theme: ThemeChoice,
    /// A validated `#rrggbb` color for the theme's primary color.
    #[serde(deserialize_with = "blank_is_none", serialize_with = "none_is_blank")]
    pub accent: Option<String>,
    #[serde(deserialize_with = "blank_is_none", serialize_with = "none_is_blank")]
    pub font_family: Option<String>,
    /// In px.
    pub font_size: u32,
    pub density: Density,
    /// Side of the avatars in the chat list and next to messages, in px.
    pub avatar_size: u32,
}

impl Default for Appearance {
    fn default() -> Self {
        // 16px is the theme's own base size.
        Appearance {
            theme: ThemeChoice::System,
            accent: None,
            font_family: None,
            font_size: 16,
            density: Density::Comfortable,
            avatar_size: 36,
        }
    }
}

/// How much space rows and headers get around their content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// Follows the OS appearance, also when it changes.
    #[default]
    System,
    Light,
    Dark,
}

/// Sizes in px.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    pub window_width: u32,
    pub window_height: u32,
    pub sidebar_width: u32,
    pub thread_panel_width: u32,
}

impl Default for Layout {
    fn default() -> Self {
        Layout { window_width: 1180, window_height: 760, sidebar_width: 300, thread_panel_width: 400 }
    }
}

/// strftime formats for message times, as chrono reads them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Time {
    pub today_format: String,
    pub older_format: String,
    /// The day separators of days before yesterday in this year, and in other years.
    pub day_format: String,
    pub other_year_day_format: String,
}

impl Default for Time {
    fn default() -> Self {
        Time {
            today_format: "%H:%M".into(),
            older_format: "%b %-d, %H:%M".into(),
            day_format: "%A, %B %-d".into(),
            other_year_day_format: "%A, %B %-d, %Y".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Chats {
    /// How many of the latest messages are loaded when a chat is opened.
    pub history_limit: u32,
    /// A message within this many minutes of the one above it, from the same
    /// sender, is shown without its name and avatar; 0 turns that off.
    pub group_minutes: u32,
}

impl Default for Chats {
    fn default() -> Self {
        Chats { history_limit: 100, group_minutes: 5 }
    }
}

/// `[media]`: how photos and GIFs show, and the programs that handle them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MediaSettings {
    /// Download photos and GIFs as their messages load, instead of on click.
    pub autoload: bool,
    /// Play GIFs inline as soon as they're loaded, instead of on click.
    pub autoplay_gifs: bool,
    /// The largest an inline image is drawn, in px.
    pub max_width: u32,
    pub max_height: u32,
    /// The program that converts MP4 "GIFs" to GIF; "" turns that off.
    pub ffmpeg: String,
    /// The program, plus arguments split on whitespace, that opens a file
    /// given as its last argument; "" means `open` on macOS, `xdg-open` elsewhere.
    pub open_command: String,
}

impl Default for MediaSettings {
    fn default() -> Self {
        MediaSettings {
            autoload: true,
            autoplay_gifs: true,
            max_width: 400,
            max_height: 320,
            ffmpeg: "ffmpeg".into(),
            open_command: String::new(),
        }
    }
}

/// A value that parsed but isn't allowed: its key, as `[section] key`, and why.
#[derive(Debug)]
struct Invalid {
    key: String,
    message: String,
}

fn invalid(section: &str, key: &str, message: impl Into<String>) -> Invalid {
    Invalid { key: format!("[{section}] {key}"), message: message.into() }
}

fn check_range(section: &str, key: &str, value: u32, range: RangeInclusive<u32>, unit: &str) -> Result<(), Invalid> {
    if range.contains(&value) {
        return Ok(());
    }
    let (min, max) = range.into_inner();
    Err(invalid(section, key, format!("{value} is out of range: use {min} to {max}{unit}.")))
}

impl Settings {
    fn validate(&self) -> Result<(), Invalid> {
        self.telegram.validate()?;
        self.appearance.validate()?;
        self.colors.light.validate("colors.light")?;
        self.colors.dark.validate("colors.dark")?;
        self.layout.validate()?;
        self.time.validate()?;
        self.chats.validate()?;
        self.media.validate()
    }
}

impl TelegramSettings {
    fn validate(&self) -> Result<(), Invalid> {
        if self.api_id < 0 {
            return Err(invalid("telegram", "api_id", "api_id must be positive, or 0 for unset."));
        }
        Ok(())
    }
}

impl Appearance {
    const FONT_SIZE: RangeInclusive<u32> = 10..=32;
    const AVATAR_SIZE: RangeInclusive<u32> = 20..=64;

    fn validate(&self) -> Result<(), Invalid> {
        if let Some(accent) = &self.accent
            && !is_hex_color(accent)
        {
            return Err(invalid(
                "appearance",
                "accent",
                format!(
                    "{accent:?} isn't a color. Use \"#rrggbb\", for example \"#2563eb\", or \"\" for the palettes'."
                ),
            ));
        }
        if let Some(family) = &self.font_family
            && family.trim() != family
        {
            return Err(invalid(
                "appearance",
                "font_family",
                format!("{family:?} starts or ends with spaces. Use a font family name, or \"\" for the theme's."),
            ));
        }
        check_range("appearance", "font_size", self.font_size, Self::FONT_SIZE, " px")?;
        check_range("appearance", "avatar_size", self.avatar_size, Self::AVATAR_SIZE, " px")
    }
}

/// Whether `color` is `#rrggbb`.
fn is_hex_color(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit())
}

impl Layout {
    const WINDOW_WIDTH: RangeInclusive<u32> = 480..=7680;
    const WINDOW_HEIGHT: RangeInclusive<u32> = 360..=4320;
    const SIDEBAR_WIDTH: RangeInclusive<u32> = 160..=640;
    const THREAD_PANEL_WIDTH: RangeInclusive<u32> = 240..=960;

    fn validate(&self) -> Result<(), Invalid> {
        check_range("layout", "window_width", self.window_width, Self::WINDOW_WIDTH, " px")?;
        check_range("layout", "window_height", self.window_height, Self::WINDOW_HEIGHT, " px")?;
        check_range("layout", "sidebar_width", self.sidebar_width, Self::SIDEBAR_WIDTH, " px")?;
        check_range("layout", "thread_panel_width", self.thread_panel_width, Self::THREAD_PANEL_WIDTH, " px")
    }
}

impl Time {
    fn validate(&self) -> Result<(), Invalid> {
        // chrono's `to_string` panics when a format can't be written, so each
        // format is test-written here with the type it will be used on.
        let date = NaiveDate::from_ymd_opt(2000, 1, 2).expect("a valid date");
        let moment = date.and_hms_opt(3, 4, 5).expect("a valid time").and_utc().fixed_offset();
        let formats = [
            ("today_format", &self.today_format, false),
            ("older_format", &self.older_format, false),
            ("day_format", &self.day_format, true),
            ("other_year_day_format", &self.other_year_day_format, true),
        ];
        for (key, format, date_only) in formats {
            let valid = StrftimeItems::new(format).all(|item| !matches!(item, Item::Error));
            if !valid {
                return Err(invalid(
                    "time",
                    key,
                    format!(
                        "{format:?} isn't a valid strftime format. The specifiers are listed at \
                         https://docs.rs/chrono/latest/chrono/format/strftime/"
                    ),
                ));
            }
            let mut out = String::new();
            let written = if date_only {
                write!(out, "{}", date.format(format))
            } else {
                write!(out, "{}", moment.format(format))
            };
            if written.is_err() {
                return Err(invalid(
                    "time",
                    key,
                    format!(
                        "{key} can't use time or zone fields like %H, %z or %s; it formats a date. Use date \
                         specifiers like %b %-d, %Y."
                    ),
                ));
            }
        }
        Ok(())
    }
}

impl Chats {
    const HISTORY_LIMIT: RangeInclusive<u32> = 1..=1000;
    const GROUP_MINUTES: RangeInclusive<u32> = 0..=1440;

    fn validate(&self) -> Result<(), Invalid> {
        check_range("chats", "history_limit", self.history_limit, Self::HISTORY_LIMIT, " messages")?;
        check_range("chats", "group_minutes", self.group_minutes, Self::GROUP_MINUTES, " minutes")
    }
}

impl MediaSettings {
    const MAX_WIDTH: RangeInclusive<u32> = 64..=2048;
    const MAX_HEIGHT: RangeInclusive<u32> = 64..=2048;

    fn validate(&self) -> Result<(), Invalid> {
        check_range("media", "max_width", self.max_width, Self::MAX_WIDTH, " px")?;
        check_range("media", "max_height", self.max_height, Self::MAX_HEIGHT, " px")?;
        // A path can contain spaces, so it isn't trimmed or split; spaces at
        // its ends are almost always a typo.
        if self.ffmpeg.trim() != self.ffmpeg {
            return Err(invalid(
                "media",
                "ffmpeg",
                format!(
                    "{:?} starts or ends with spaces. Use a program name or path, e.g. \"ffmpeg\", or \"\" to turn \
                     GIF conversion off.",
                    self.ffmpeg
                ),
            ));
        }
        if !self.open_command.is_empty() && self.open_command.trim().is_empty() {
            return Err(invalid(
                "media",
                "open_command",
                "is only spaces. Use a program and its arguments, e.g. \"xdg-open\", or \"\" for the system's.",
            ));
        }
        if shlex::split(&self.open_command).is_none() {
            return Err(invalid(
                "media",
                "open_command",
                format!(
                    "{:?} has an unclosed quote. Quote words with spaces like a shell, e.g. \"open -a 'Adobe \
                     Photoshop 2024'\".",
                    self.open_command
                ),
            ));
        }
        Ok(())
    }
}

/// The words of a command line from the config, split like a shell would
/// but without running one. Lines with bad quoting were refused on startup.
pub fn command_words(command: &str) -> Vec<String> {
    shlex::split(command).unwrap_or_default()
}

/// Reads a string where "" means unset.
fn blank_is_none<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let value = String::deserialize(deserializer)?;
    Ok((!value.is_empty()).then_some(value))
}

fn none_is_blank<S: Serializer>(value: &Option<String>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(value.as_deref().unwrap_or_default())
}

/// Parses and checks `text`, the contents of the config file at `path`.
fn parse(text: &str, path: &Path) -> Result<Settings, ConfigError> {
    let file = path.display();
    let table: toml::Table = toml::from_str(text).map_err(|error| syntax_error(&error, text, path, false))?;
    // People publish their dotfiles, so the hash itself is refused outright.
    if table.get("telegram").and_then(|telegram| telegram.get("api_hash")).is_some() {
        return Err(ConfigError::File(format!(
            "{file} ([telegram] api_hash):\nThe api_hash isn't read from the config file, since config files often \
             end up in published dotfiles. Remove it and use api_hash_command, for example \
             api_hash_command = \"pass frillogram/api_hash\", or the FRILLOGRAM_API_HASH environment variable."
        )));
    }
    let settings: Settings = toml::from_str(text).map_err(|error| syntax_error(&error, text, path, true))?;
    settings.validate().map_err(|Invalid { key, message }| ConfigError::File(format!("{file} ({key}):\n{message}")))?;
    Ok(settings)
}

/// Describes a TOML or type error by line, column and key. toml's own
/// `Display` quotes the line, which could be a plaintext `api_hash`.
fn syntax_error(error: &toml::de::Error, text: &str, path: &Path, typed: bool) -> ConfigError {
    let mut location = String::new();
    let mut message = error.message().trim().to_string();
    if let Some(span) = error.span() {
        let before = &text[..span.start];
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let line = before.matches('\n').count() + 1;
        let column = before[line_start..].chars().count() + 1;
        location = format!(", line {line}, column {column}");
        // The key is the text before `=` on that line, under the closest table header.
        let key = text[line_start..].lines().next().and_then(|line| line.split_once('=')).map(|(key, _)| key.trim());
        if let Some(key) = key.filter(|key| !key.is_empty() && !key.starts_with('[')) {
            let section = before[..line_start].lines().rev().map(str::trim).find(|line| line.starts_with('['));
            // A type error quotes the value, which for [telegram] keys could be a secret.
            if typed && section == Some("[telegram]") {
                message = match key {
                    "api_id" => "api_id: expected a whole number".into(),
                    _ => format!("{key}: expected a string"),
                };
            }
            location += &match section {
                Some(section) => format!(" ({section} {key})"),
                None => format!(" ({key})"),
            };
        }
    }
    ConfigError::File(format!("{}{location}:\n{message}", path.display()))
}

/// Who named the config file. A file the user named has to exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Flag,
    Env,
    Default,
}

/// Where the config file is looked for: `--config`, then `$FRILLOGRAM_CONFIG`,
/// then `$XDG_CONFIG_HOME/frillogram`, then `~/.config/frillogram`, on macOS too.
fn locate(
    flag: Option<PathBuf>,
    var: &impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Option<(PathBuf, Source)> {
    // A blank value usually means a failed `$(…)`, so it counts as unset.
    let set = |name: &str| var(name).filter(|value| !value.to_string_lossy().trim().is_empty());
    if let Some(path) = flag {
        return Some((path, Source::Flag));
    }
    if let Some(path) = set("FRILLOGRAM_CONFIG") {
        return Some((path.into(), Source::Env));
    }
    // The XDG spec says to ignore a relative $XDG_CONFIG_HOME.
    let config_home = set("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.map(|home| home.join(".config")))?;
    Some((config_home.join("frillogram").join("config.toml"), Source::Default))
}

/// Reads the config file. One that isn't there at the default location means
/// all defaults.
fn read(path: &Path, source: Source) -> Result<Settings, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && source == Source::Default => {
            return Ok(Settings::default());
        }
        Err(error) => {
            let named_by = match source {
                Source::Flag => " (named by --config)",
                Source::Env => " (named by FRILLOGRAM_CONFIG)",
                Source::Default => "",
            };
            return Err(ConfigError::File(format!("Can't read {}{named_by}:\n{error}", path.display())));
        }
    };
    parse(&text, path)
}

/// What startup needs from the configuration.
#[derive(Debug)]
pub struct Loaded {
    /// The file's settings, or the defaults if there's no file or it has an error.
    pub settings: Settings,
    /// Where the config file is or would be; `None` without a home directory.
    pub path: Option<PathBuf>,
    /// The TDLib configuration, or why Frillogram can't start.
    pub config: Result<Config, ConfigError>,
}

/// Loads the config file (`flag` is `--config`) and resolves the TDLib
/// configuration from it and the environment.
pub fn load(flag: Option<PathBuf>) -> Loaded {
    load_with(flag, |name| std::env::var_os(name), dirs::home_dir(), dirs::data_local_dir)
}

/// [`load`] with the environment, home and default data directory passed in,
/// so it can be tested.
fn load_with(
    flag: Option<PathBuf>,
    var: impl Fn(&str) -> Option<OsString>,
    home: Option<PathBuf>,
    default_dir: impl FnOnce() -> Option<PathBuf>,
) -> Loaded {
    let location = locate(flag, &var, home.as_deref());
    let path = location.as_ref().map(|(path, _)| path.clone());
    let settings = match &location {
        Some((path, source)) => read(path, *source),
        None => Ok(Settings::default()),
    };
    match settings {
        Ok(settings) => {
            let config = Config::resolve(&settings.telegram, path.as_deref(), &var, home.as_deref(), default_dir);
            Loaded { settings, path, config }
        }
        Err(error) => Loaded { settings: Settings::default(), path, config: Err(error) },
    }
}

impl Loaded {
    /// Refuses a `[appearance] font_family` that isn't among `installed`, so
    /// a typo isn't silently drawn in a fallback font. Only the UI can list
    /// the fonts, so it calls this. An earlier error is kept.
    pub fn check_font(&mut self, installed: &[String]) {
        let Some(family) = &self.settings.appearance.font_family else { return };
        if self.config.is_err() || installed.contains(family) {
            return;
        }
        let file = self.path.as_deref().map_or("The config file".into(), |path| path.display().to_string());
        let mut message =
            format!("{file} ([appearance] font_family):\n{family:?} isn't an installed font family. Use one that is");
        match installed.iter().find(|name| name.eq_ignore_ascii_case(family)) {
            Some(name) => message += &format!(", for example {name:?}."),
            None => message += ", or \"\" for the theme's.",
        }
        self.config = Err(ConfigError::File(message));
    }
}

/// Telegram API credentials and where TDLib keeps its database.
#[derive(Clone)]
pub struct Config {
    pub api_id: i32,
    pub api_hash: String,
    pub data_dir: PathBuf,
}

// `api_hash` is a secret, so it never shows up in logs.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("api_id", &self.api_id)
            .field("api_hash", &"<redacted>")
            .field("data_dir", &self.data_dir)
            .finish()
    }
}

/// Why Frillogram can't start, so the setup screen can say what to fix. Each
/// message may span several lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The config file can't be read, or has an unknown key or a bad value.
    File(String),
    /// The `api_id` or `api_hash` is missing or malformed, or
    /// `api_hash_command` failed.
    Credentials(String),
    /// There's no usable place for TDLib's database.
    DataDir(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::File(error) | ConfigError::Credentials(error) | ConfigError::DataDir(error) => {
                f.write_str(error)
            }
        }
    }
}

impl Config {
    /// Resolves `[telegram]` from the file at `path`, with
    /// `FRILLOGRAM_API_ID`, `FRILLOGRAM_API_HASH` and `FRILLOGRAM_DATA_DIR`
    /// overriding it. Each user gets their own credentials at
    /// <https://my.telegram.org>; they're never committed.
    fn resolve(
        file: &TelegramSettings,
        path: Option<&Path>,
        var: &impl Fn(&str) -> Option<OsString>,
        home: Option<&Path>,
        default_dir: impl FnOnce() -> Option<PathBuf>,
    ) -> Result<Self, ConfigError> {
        let file_name = path.map_or("the config file".into(), |path| path.display().to_string());
        let credentials = ConfigError::Credentials;
        // An empty or blank value usually means a failed `$(pass …)`, so it
        // counts as unset.
        let text = |name: &str| {
            var(name)
                .map(OsString::into_string)
                .transpose()
                .map(|value| value.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()))
                .map_err(|_| credentials(format!("{name} is not valid UTF-8")))
        };
        let api_id = match text("FRILLOGRAM_API_ID")? {
            Some(id) => match id.parse::<i32>() {
                Ok(id) if id > 0 => id,
                _ => return Err(credentials("FRILLOGRAM_API_ID must be a positive number".into())),
            },
            None if file.api_id != 0 => file.api_id,
            None => {
                return Err(credentials(format!(
                    "api_id is not set. Set it in the [telegram] section of {file_name}, or set FRILLOGRAM_API_ID."
                )));
            }
        };
        // The environment wins without running the command, which may ask
        // for a passphrase.
        let api_hash = match text("FRILLOGRAM_API_HASH")? {
            Some(hash) => check_hash(hash, "FRILLOGRAM_API_HASH")?,
            None if !file.api_hash_command.is_empty() => {
                check_hash(run_hash_command(&file.api_hash_command)?, "api_hash_command's output")?
            }
            None => {
                return Err(credentials(format!(
                    "api_hash is not set. Set api_hash_command in the [telegram] section of {file_name}, for \
                     example api_hash_command = \"pass frillogram/api_hash\", or set FRILLOGRAM_API_HASH."
                )));
            }
        };
        // A relative path would depend on where Frillogram is launched
        // from, and could put the session inside a repository.
        let file_dir = match file.data_dir.as_str() {
            "" => None,
            dir => Some(expand_home(dir, home).filter(|dir| dir.is_absolute()).ok_or_else(|| {
                ConfigError::DataDir(format!(
                    "data_dir in the [telegram] section of {file_name} must be an absolute path; a leading ~/ is \
                     expanded."
                ))
            })?),
        };
        // Not trimmed: spaces can be part of a path.
        let data_dir = match var("FRILLOGRAM_DATA_DIR").filter(|dir| !dir.to_string_lossy().trim().is_empty()) {
            Some(dir) if !Path::new(&dir).is_absolute() => {
                return Err(ConfigError::DataDir("FRILLOGRAM_DATA_DIR must be an absolute path".into()));
            }
            Some(dir) => PathBuf::from(dir),
            None => match file_dir {
                Some(dir) => dir,
                None => default_dir()
                    .ok_or_else(|| ConfigError::DataDir("no local data directory on this system".into()))?
                    .join("frillogram"),
            },
        }
        .join("tdlib");
        Ok(Config { api_id, api_hash, data_dir })
    }
}

/// `dir` with a leading `~/` replaced by the home directory, if there is one.
fn expand_home(dir: &str, home: Option<&Path>) -> Option<PathBuf> {
    match dir.strip_prefix("~/") {
        Some(rest) => home.map(|home| home.join(rest)),
        None => Some(PathBuf::from(dir)),
    }
}

/// Whether `text` is 32 hex characters, which is what an `api_hash` looks like.
fn looks_like_hash(text: &str) -> bool {
    text.len() == 32 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// Returns `hash` if it's 32 hex characters. The error names `source` and
/// never the value.
fn check_hash(hash: String, source: &str) -> Result<String, ConfigError> {
    if looks_like_hash(&hash) {
        Ok(hash)
    } else {
        Err(ConfigError::Credentials(format!(
            "{source} isn't a valid api_hash: it should be 32 hexadecimal characters, as shown at \
             https://my.telegram.org."
        )))
    }
}

/// Runs `api_hash_command` with `sh -c` and returns the first line of its
/// output, trimmed; later lines are ignored, as in `pass` entries that keep
/// notes below the secret. Errors never include the command line, which a
/// pasted hash would put there, nor its stdout.
fn run_hash_command(command: &str) -> Result<String, ConfigError> {
    if looks_like_hash(command.trim()) {
        return Err(ConfigError::Credentials(
            "api_hash_command is a command that prints the hash, not the hash itself. Config files often end up in \
             published dotfiles, so use for example api_hash_command = \"pass frillogram/api_hash\", or the \
             FRILLOGRAM_API_HASH environment variable."
                .into(),
        ));
    }
    let fail = |what: String, stderr: &[u8]| {
        let stderr = String::from_utf8_lossy(stderr);
        let mut message = format!("api_hash_command {what}.");
        // sh quotes an unknown command's name in its error, so lines naming
        // the command or any of its words are left out.
        let words: Vec<&str> = command.split_whitespace().filter(|word| word.len() >= 8).collect();
        let lines: Vec<&str> = stderr.lines().filter(|line| !words.iter().any(|word| line.contains(word))).collect();
        let lines = lines.join("\n");
        if !lines.trim().is_empty() {
            message += &format!("\nIts error output:\n{}", lines.trim());
        }
        ConfigError::Credentials(message)
    };
    // stdin stays the terminal's, so a passphrase prompt there still works.
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::inherit())
        .output()
        .map_err(|error| ConfigError::Credentials(format!("Can't run api_hash_command with sh: {error}")))?;
    if !output.status.success() {
        // 127 is "command not found", whose message names the command.
        let stderr: &[u8] = if output.status.code() == Some(127) { b"" } else { &output.stderr };
        return Err(fail(format!("failed ({})", output.status), stderr));
    }
    let Ok(stdout) = String::from_utf8(output.stdout) else {
        return Err(fail("printed something that isn't UTF-8".into(), &output.stderr));
    };
    match stdout.lines().next().map(str::trim) {
        None | Some("") => Err(fail("printed nothing".into(), &output.stderr)),
        Some(hash) => Ok(hash.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const PATH: &str = "/home/u/.config/frillogram/config.toml";

    fn vars_from(vars: Vec<(&str, OsString)>) -> impl Fn(&str) -> Option<OsString> {
        let vars: HashMap<String, OsString> = vars.into_iter().map(|(name, value)| (name.to_string(), value)).collect();
        move |name| vars.get(name).cloned()
    }

    fn resolve_os(file: &TelegramSettings, vars: Vec<(&str, OsString)>) -> Result<Config, ConfigError> {
        let var = vars_from(vars);
        Config::resolve(file, Some(Path::new(PATH)), &var, Some(Path::new("/home/u")), || Some(PathBuf::from("/data")))
    }

    fn resolve(file: &TelegramSettings, vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        resolve_os(file, vars.iter().map(|&(name, value)| (name, value.into())).collect())
    }

    fn config(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        resolve(&TelegramSettings::default(), vars)
    }

    fn credentials_error(error: &str) -> ConfigError {
        ConfigError::Credentials(error.into())
    }

    fn file_settings(text: &str) -> Result<Settings, ConfigError> {
        parse(text, Path::new(PATH))
    }

    /// The message of a `File` error, without the path.
    fn file_error(text: &str) -> String {
        match file_settings(text).unwrap_err() {
            ConfigError::File(message) => message.strip_prefix(PATH).unwrap().to_string(),
            error => panic!("not a file error: {error:?}"),
        }
    }

    const HASH: &str = "0123456789abcdef0123456789abcdef";
    const HASH2: &str = "fedcba9876543210fedcba9876543210";
    const CREDENTIALS: [(&str, &str); 2] =
        [("FRILLOGRAM_API_ID", " 123 "), ("FRILLOGRAM_API_HASH", " 0123456789abcdef0123456789abcdef\n")];

    #[test]
    fn credentials_are_trimmed() {
        let config = config(&CREDENTIALS).unwrap();
        assert_eq!((config.api_id, config.api_hash.as_str()), (123, HASH));
        assert_eq!(config.data_dir, Path::new("/data/frillogram/tdlib"));
    }

    #[test]
    fn debug_never_shows_the_hash() {
        let config = config(&CREDENTIALS).unwrap();
        assert!(!format!("{config:?}").contains(HASH));
    }

    #[test]
    fn blank_credentials_count_as_unset() {
        let error = config(&[("FRILLOGRAM_API_ID", "  "), CREDENTIALS[1]]).unwrap_err();
        assert_eq!(
            error,
            credentials_error(&format!(
                "api_id is not set. Set it in the [telegram] section of {PATH}, or set FRILLOGRAM_API_ID."
            ))
        );
        let error = config(&[CREDENTIALS[0], ("FRILLOGRAM_API_HASH", "")]).unwrap_err();
        assert!(error.to_string().starts_with("api_hash is not set. Set api_hash_command"), "{error}");
        let error = config(&[("FRILLOGRAM_API_ID", "12a"), CREDENTIALS[1]]).unwrap_err();
        assert_eq!(error, credentials_error("FRILLOGRAM_API_ID must be a positive number"));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_credentials_are_reported_as_such() {
        use std::os::unix::ffi::OsStringExt;
        let invalid = || OsString::from_vec(vec![b'1', 0xff]);
        let file = TelegramSettings::default();
        let error =
            resolve_os(&file, vec![("FRILLOGRAM_API_ID", invalid()), (CREDENTIALS[1].0, CREDENTIALS[1].1.into())]);
        assert_eq!(error.unwrap_err(), credentials_error("FRILLOGRAM_API_ID is not valid UTF-8"));
        let error =
            resolve_os(&file, vec![(CREDENTIALS[0].0, CREDENTIALS[0].1.into()), ("FRILLOGRAM_API_HASH", invalid())]);
        assert_eq!(error.unwrap_err(), credentials_error("FRILLOGRAM_API_HASH is not valid UTF-8"));
    }

    #[test]
    fn blank_data_dir_counts_as_unset() {
        let config = config(&[CREDENTIALS[0], CREDENTIALS[1], ("FRILLOGRAM_DATA_DIR", " ")]).unwrap();
        assert_eq!(config.data_dir, Path::new("/data/frillogram/tdlib"));
    }

    #[test]
    fn data_dir_must_be_absolute() {
        let error = config(&[CREDENTIALS[0], CREDENTIALS[1], ("FRILLOGRAM_DATA_DIR", "tdata")]).unwrap_err();
        assert_eq!(error, ConfigError::DataDir("FRILLOGRAM_DATA_DIR must be an absolute path".into()));
        let config = config(&[CREDENTIALS[0], CREDENTIALS[1], ("FRILLOGRAM_DATA_DIR", "/elsewhere")]).unwrap();
        assert_eq!(config.data_dir, Path::new("/elsewhere/tdlib"));
    }

    fn telegram(api_id: i32, api_hash_command: &str, data_dir: &str) -> TelegramSettings {
        TelegramSettings { api_id, api_hash_command: api_hash_command.into(), data_dir: data_dir.into() }
    }

    #[test]
    fn file_credentials_work_without_env_vars() {
        let config = resolve(&telegram(42, &format!("echo {HASH}"), ""), &[]).unwrap();
        assert_eq!((config.api_id, config.api_hash.as_str()), (42, HASH));
        assert_eq!(config.data_dir, Path::new("/data/frillogram/tdlib"));
    }

    #[test]
    fn env_vars_override_the_file() {
        // The command would fail, so passing shows it never ran.
        let file = telegram(42, "exit 3", "/from-file");
        let config = resolve(&file, &[CREDENTIALS[0], CREDENTIALS[1], ("FRILLOGRAM_DATA_DIR", "/from-env")]).unwrap();
        assert_eq!((config.api_id, config.api_hash.as_str()), (123, HASH));
        assert_eq!(config.data_dir, Path::new("/from-env/tdlib"));
        // Blank env vars count as unset, so the file applies.
        let file = telegram(42, &format!("echo {HASH2}"), "/from-file");
        let blank = [("FRILLOGRAM_API_ID", " "), ("FRILLOGRAM_API_HASH", ""), ("FRILLOGRAM_DATA_DIR", "")];
        let config = resolve(&file, &blank).unwrap();
        assert_eq!((config.api_id, config.api_hash.as_str()), (42, HASH2));
        assert_eq!(config.data_dir, Path::new("/from-file/tdlib"));
    }

    #[test]
    fn data_dir_expands_a_leading_tilde() {
        let config = resolve(&telegram(1, &format!("echo {HASH}"), "~/tg"), &[]).unwrap();
        assert_eq!(config.data_dir, Path::new("/home/u/tg/tdlib"));
        for relative in ["tg", "~", "~other/tg"] {
            let error = resolve(&telegram(1, &format!("echo {HASH}"), relative), &[]).unwrap_err();
            assert!(matches!(error, ConfigError::DataDir(_)), "{relative}: {error:?}");
            assert!(error.to_string().contains(PATH), "{error}");
        }
    }

    #[test]
    fn failing_hash_command_shows_stderr_but_never_stdout() {
        // The command prints "hidden", which its own text doesn't contain.
        let error = run_hash_command("printf 'hid%s' den; echo oops >&2; exit 3").unwrap_err();
        let message = error.to_string();
        assert!(matches!(error, ConfigError::Credentials(_)));
        assert!(message.contains("exit status: 3"), "{message}");
        assert!(message.contains("oops"), "{message}");
        assert!(!message.contains("hidden"), "{message}");
        assert_eq!(
            run_hash_command("exit 3").unwrap_err(),
            credentials_error("api_hash_command failed (exit status: 3).")
        );
    }

    #[test]
    fn empty_hash_command_output_is_an_error() {
        assert_eq!(run_hash_command("true").unwrap_err(), credentials_error("api_hash_command printed nothing."));
        assert_eq!(run_hash_command("echo '  '").unwrap_err(), credentials_error("api_hash_command printed nothing."));
        assert_eq!(run_hash_command(&format!("echo {HASH}")).unwrap(), HASH);
    }

    #[test]
    fn hash_command_uses_only_the_first_line() {
        assert_eq!(run_hash_command(&format!("printf '{HASH}\\nurl: example.com\\n'")).unwrap(), HASH);
        let error = run_hash_command("printf '\\n%s\\n' secret").unwrap_err();
        assert_eq!(error, credentials_error("api_hash_command printed nothing."));
    }

    #[test]
    fn hashes_must_be_32_hex_characters() {
        for bad in ["abc", "0123456789abcdef0123456789abcdeg", "0123456789abcdef0123456789abcdef0"] {
            let error = config(&[CREDENTIALS[0], ("FRILLOGRAM_API_HASH", bad)]).unwrap_err();
            assert!(matches!(error, ConfigError::Credentials(_)));
            let message = error.to_string();
            assert!(message.contains("FRILLOGRAM_API_HASH") && message.contains("32"), "{message}");
            assert!(!message.contains(bad), "{message}");
            let error = resolve(&telegram(1, &format!("echo {bad}x"), ""), &[]).unwrap_err();
            let message = error.to_string();
            assert!(message.contains("api_hash_command's output"), "{message}");
            assert!(!message.contains(bad), "{message}");
        }
    }

    #[test]
    fn a_hash_in_api_hash_command_is_refused_without_echoing_it() {
        let error = resolve(&telegram(1, HASH, ""), &[]).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("pass frillogram/api_hash") && message.contains("FRILLOGRAM_API_HASH"), "{message}");
        assert!(!message.contains(HASH), "{message}");
    }

    #[test]
    fn hash_command_errors_never_show_the_command() {
        // Not found: sh's stderr names the command, so it isn't shown.
        let command = format!("{HASH}-missing");
        let message = run_hash_command(&command).unwrap_err().to_string();
        assert_eq!(message, "api_hash_command failed (exit status: 127).");
        // Other failures keep their stderr unless it names the command's words.
        let command = format!("echo {HASH} >&2; echo oops >&2; exit 3");
        let message = run_hash_command(&command).unwrap_err().to_string();
        assert!(message.contains("oops") && !message.contains(HASH), "{message}");
    }

    #[test]
    fn telegram_type_errors_never_echo_the_value() {
        let error = file_error(&format!("[telegram]\napi_id = \"{HASH}\"\n"));
        assert_eq!(error, ", line 2, column 10 ([telegram] api_id):\napi_id: expected a whole number");
        let error = file_error("[telegram]\napi_hash_command = 123456\n");
        assert!(error.contains("api_hash_command: expected a string") && !error.contains("123456"), "{error}");
    }

    #[test]
    fn env_api_id_must_be_positive() {
        for bad in ["0", "-5", "99999999999"] {
            let error = config(&[("FRILLOGRAM_API_ID", bad), CREDENTIALS[1]]).unwrap_err();
            assert_eq!(error, credentials_error("FRILLOGRAM_API_ID must be a positive number"), "{bad}");
        }
    }

    #[test]
    fn day_formats_cannot_use_time_or_zone_fields() {
        for format in ["%H:%M", "%z", "%s", "%Z"] {
            for key in ["day_format", "other_year_day_format"] {
                let error = file_error(&format!("[time]\n{key} = \"{format}\"\n"));
                assert!(error.contains(&format!("[time] {key}")), "{error}");
                assert!(error.contains("can't use time or zone fields"), "{error}");
            }
        }
        // The same fields are fine in the formats that take a time.
        assert!(file_settings("[time]\ntoday_format = \"%H:%M %z\"\nolder_format = \"%s\"\n").is_ok());
        assert!(file_settings("[time]\nday_format = \"%A, %b %-d\"\n").is_ok());
    }

    #[test]
    fn file_sets_every_section() {
        let settings = file_settings(
            r##"
            [telegram]
            api_id = 7
            api_hash_command = "pass frillogram/api_hash"
            data_dir = "~/tg"

            [appearance]
            theme = "dark"
            accent = "#2563EB"
            font_family = "Inter"
            font_size = 14
            density = "compact"
            avatar_size = 28

            [colors.dark]
            background = "#202028"
            avatar_colors = ["#aabbcc"]

            [layout]
            window_width = 1000
            window_height = 700
            sidebar_width = 250
            thread_panel_width = 350

            [time]
            today_format = "%H:%M:%S"
            older_format = "%Y-%m-%d"
            day_format = "%a %-d %b"
            other_year_day_format = "%a %-d %b %Y"

            [chats]
            history_limit = 50
            group_minutes = 0

            [media]
            autoload = false
            autoplay_gifs = false
            max_width = 600
            max_height = 500
            ffmpeg = "/opt/homebrew/bin/ffmpeg"
            open_command = "open -a Preview"
            "##,
        )
        .unwrap();
        assert_eq!(
            settings,
            Settings {
                telegram: telegram(7, "pass frillogram/api_hash", "~/tg"),
                appearance: Appearance {
                    theme: ThemeChoice::Dark,
                    accent: Some("#2563EB".into()),
                    font_family: Some("Inter".into()),
                    font_size: 14,
                    density: Density::Compact,
                    avatar_size: 28,
                },
                // A partial dark palette keeps the dark defaults, not the light ones.
                colors: Colors {
                    light: Palette::light(),
                    dark: Palette {
                        background: "#202028".into(),
                        avatar_colors: vec!["#aabbcc".into()],
                        ..Palette::dark()
                    },
                },
                layout: Layout { window_width: 1000, window_height: 700, sidebar_width: 250, thread_panel_width: 350 },
                time: Time {
                    today_format: "%H:%M:%S".into(),
                    older_format: "%Y-%m-%d".into(),
                    day_format: "%a %-d %b".into(),
                    other_year_day_format: "%a %-d %b %Y".into(),
                },
                chats: Chats { history_limit: 50, group_minutes: 0 },
                media: MediaSettings {
                    autoload: false,
                    autoplay_gifs: false,
                    max_width: 600,
                    max_height: 500,
                    ffmpeg: "/opt/homebrew/bin/ffmpeg".into(),
                    open_command: "open -a Preview".into(),
                },
            }
        );
    }

    #[test]
    fn palette_colors_are_checked() {
        assert_eq!(
            file_error("[colors.light]\nbackground = \"white\"\n"),
            " ([colors.light] background):\n\"white\" isn't a color. Use \"#rrggbb\", e.g. \"#c5b6f2\"."
        );
        assert!(file_error("[colors.dark]\navatar_colors = []\n").contains("([colors.dark] avatar_colors):\nis empty"));
        assert!(file_error("[colors.dark]\navatar_colors = [\"#abc\"]\n").contains("[colors.dark] avatar_colors"));
        assert!(file_error("[colors.dark]\nbackgrund = \"#000000\"\n").contains("unknown field `backgrund`"));
        assert!(file_error("[colors.sepia]\n").contains("unknown field `sepia`"));
    }

    #[test]
    fn missing_keys_keep_their_defaults() {
        let settings = file_settings("[layout]\nsidebar_width = 200\n").unwrap();
        assert_eq!(settings.layout, Layout { sidebar_width: 200, ..Layout::default() });
        assert_eq!(settings.appearance, Appearance::default());
        assert_eq!(file_settings("").unwrap(), Settings::default());
    }

    #[test]
    fn unknown_keys_are_errors() {
        assert_eq!(
            file_error("[layout]\nsidebar_widht = 200\n"),
            ", line 2, column 1 ([layout] sidebar_widht):\nunknown field `sidebar_widht`, expected one of \
             `window_width`, `window_height`, `sidebar_width`, `thread_panel_width`"
        );
        assert_eq!(
            file_error("[colours]\n"),
            ", line 1, column 2:\nunknown field `colours`, expected one of `telegram`, `appearance`, `colors`, \
             `layout`, `time`, `chats`, `media`"
        );
        assert!(file_error("font_size = 12\n").contains("(font_size):\nunknown field `font_size`"));
    }

    #[test]
    fn wrong_types_name_line_and_key() {
        assert_eq!(
            file_error("[appearance]\ntheme = \"dark\"\nfont_size = 14.5\n"),
            ", line 3, column 13 ([appearance] font_size):\ninvalid type: floating point `14.5`, expected u32"
        );
        assert_eq!(
            file_error("[appearance]\ntheme = \"blue\"\n"),
            ", line 2, column 9 ([appearance] theme):\nunknown variant `blue`, expected one of `system`, `light`, `dark`"
        );
    }

    #[test]
    fn bad_values_say_what_is_allowed() {
        assert_eq!(
            file_error("[layout]\nsidebar_width = 50\n"),
            " ([layout] sidebar_width):\n50 is out of range: use 160 to 640 px."
        );
        assert_eq!(
            file_error("[chats]\nhistory_limit = 0\n"),
            " ([chats] history_limit):\n0 is out of range: use 1 to 1000 messages."
        );
        assert!(file_error("[appearance]\nfont_size = 100\n").contains("use 10 to 32 px"));
        assert!(file_error("[layout]\nwindow_width = 100\n").contains("[layout] window_width"));
        assert!(file_error("[layout]\nwindow_height = 9000\n").contains("[layout] window_height"));
        assert!(file_error("[layout]\nthread_panel_width = 10\n").contains("[layout] thread_panel_width"));
        for accent in ["blue", "#12345", "#1234567", "#12345g", "2563eb"] {
            let error = file_error(&format!("[appearance]\naccent = \"{accent}\"\n"));
            assert!(error.contains("isn't a color. Use \"#rrggbb\""), "{accent}: {error}");
        }
        assert!(file_error("[appearance]\nfont_family = \" Inter\"\n").contains("[appearance] font_family"));
        assert_eq!(
            file_error("[time]\nolder_format = \"%Q\"\n"),
            " ([time] older_format):\n\"%Q\" isn't a valid strftime format. The specifiers are listed at \
             https://docs.rs/chrono/latest/chrono/format/strftime/"
        );
        assert!(file_error("[time]\ntoday_format = \"%\"\n").contains("[time] today_format"));
        assert!(file_error("[time]\nday_format = \"%Q\"\n").contains("[time] day_format"));
        assert!(file_error("[time]\nother_year_day_format = \"%\"\n").contains("[time] other_year_day_format"));
        assert_eq!(
            file_error("[appearance]\navatar_size = 8\n"),
            " ([appearance] avatar_size):\n8 is out of range: use 20 to 64 px."
        );
        assert_eq!(
            file_error("[chats]\ngroup_minutes = 2000\n"),
            " ([chats] group_minutes):\n2000 is out of range: use 0 to 1440 minutes."
        );
        assert_eq!(
            file_error("[appearance]\ndensity = \"cozy\"\n"),
            ", line 2, column 11 ([appearance] density):\nunknown variant `cozy`, expected `comfortable` or `compact`"
        );
        assert!(file_error("[telegram]\napi_id = -1\n").contains("api_id must be positive"));
        assert!(file_error("[telegram]\napi_id = 99999999999\n").contains("[telegram] api_id"));
    }

    #[test]
    fn media_values_are_checked() {
        assert_eq!(
            file_error("[media]\nmax_width = 10\n"),
            " ([media] max_width):\n10 is out of range: use 64 to 2048 px."
        );
        assert!(file_error("[media]\nmax_height = 5000\n").contains("[media] max_height"));
        assert!(file_error("[media]\nffmpeg = \"ffmpeg \"\n").contains("[media] ffmpeg"));
        assert!(file_error("[media]\nopen_command = \"  \"\n").contains("[media] open_command"));
        let error = file_error("[media]\nopen_command = \"open -a 'Adobe Photoshop\"\n");
        assert!(error.contains("[media] open_command") && error.contains("unclosed quote"), "{error}");
        assert!(file_error("[media]\nautoload = \"yes\"\n").contains("([media] autoload):\ninvalid type"));
        // "" turns conversion off, and means the system's opener.
        let settings = file_settings("[media]\nffmpeg = \"\"\nopen_command = \"\"\n").unwrap();
        assert_eq!((settings.media.ffmpeg.as_str(), settings.media.open_command.as_str()), ("", ""));
        let settings = file_settings("[media]\nffmpeg = \"/Applications/My Tools/ffmpeg\"\n").unwrap();
        assert_eq!(settings.media.ffmpeg, "/Applications/My Tools/ffmpeg");
    }

    #[test]
    fn empty_strings_mean_the_theme_s_own() {
        let settings = file_settings("[appearance]\naccent = \"\"\nfont_family = \"\"\n").unwrap();
        assert_eq!((settings.appearance.accent, settings.appearance.font_family), (None, None));
    }

    #[test]
    fn plaintext_api_hash_is_refused_without_echoing_it() {
        let error = file_error("[telegram]\napi_hash = \"0123456789abcdef\"\n");
        assert!(error.starts_with(" ([telegram] api_hash):\n"), "{error}");
        assert!(error.contains("api_hash_command") && error.contains("FRILLOGRAM_API_HASH"), "{error}");
        assert!(!error.contains("0123456789abcdef"), "{error}");
        // Unquoted, it's a TOML syntax error, which mustn't quote the line either.
        let error = file_error("[telegram]\napi_hash = 0123456789abcdef\n");
        assert!(error.contains("([telegram] api_hash)"), "{error}");
        assert!(!error.contains("0123456789abcdef"), "{error}");
    }

    /// `config.example.toml` with every `# key = value` line uncommented.
    fn uncommented_example() -> String {
        let is_key = |key: &str| !key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c == '_');
        EXAMPLE
            .lines()
            .map(|line| match line.strip_prefix("# ") {
                Some(rest) if rest.split_once(" = ").is_some_and(|(key, _)| is_key(key)) => rest,
                _ => line,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn example_file_lists_every_key_at_its_default() {
        let example = uncommented_example();
        assert_eq!(file_settings(&example).unwrap(), Settings::default());
        // Also catches a key missing from the example, which would parse to its default too.
        let keys: toml::Value = toml::from_str(&example).unwrap();
        assert_eq!(keys, toml::Value::try_from(Settings::default()).unwrap());
    }

    #[test]
    fn example_file_as_shipped_is_all_defaults() {
        assert_eq!(file_settings(EXAMPLE).unwrap(), Settings::default());
    }

    fn locate_with(flag: Option<&str>, vars: &[(&str, &str)], home: Option<&str>) -> Option<(PathBuf, Source)> {
        let var = vars_from(vars.iter().map(|&(name, value)| (name, value.into())).collect());
        locate(flag.map(PathBuf::from), &var, home.map(Path::new))
    }

    #[test]
    fn config_file_location_precedence() {
        let all = [("FRILLOGRAM_CONFIG", "/env.toml"), ("XDG_CONFIG_HOME", "/xdg")];
        assert_eq!(locate_with(Some("flag.toml"), &all, Some("/home/u")), Some(("flag.toml".into(), Source::Flag)));
        assert_eq!(locate_with(None, &all, Some("/home/u")), Some(("/env.toml".into(), Source::Env)));
        let xdg = Some(("/xdg/frillogram/config.toml".into(), Source::Default));
        assert_eq!(locate_with(None, &all[1..], Some("/home/u")), xdg);
        let home = Some(("/home/u/.config/frillogram/config.toml".into(), Source::Default));
        assert_eq!(locate_with(None, &[], Some("/home/u")), home);
        // Blank values count as unset, and a relative XDG_CONFIG_HOME is ignored.
        let blank = [("FRILLOGRAM_CONFIG", " "), ("XDG_CONFIG_HOME", "")];
        assert_eq!(locate_with(None, &blank, Some("/home/u")), home);
        assert_eq!(locate_with(None, &[("XDG_CONFIG_HOME", "rel")], Some("/home/u")), home);
        assert_eq!(locate_with(None, &[], None), None);
    }

    /// A file in a fresh temporary directory, removed when dropped.
    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str, text: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("frillogram-test-{}-{name}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("config.toml");
            std::fs::write(&path, text).unwrap();
            TempFile(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    fn load_test(flag: Option<PathBuf>, vars: &[(&str, &str)]) -> Loaded {
        let var = vars_from(vars.iter().map(|&(name, value)| (name, value.into())).collect());
        load_with(flag, var, Some("/nonexistent-home".into()), || Some(PathBuf::from("/data")))
    }

    #[test]
    fn missing_file_at_the_default_location_means_defaults() {
        let loaded = load_test(None, &CREDENTIALS);
        assert_eq!(loaded.settings, Settings::default());
        assert_eq!(loaded.path, Some("/nonexistent-home/.config/frillogram/config.toml".into()));
        assert_eq!(loaded.config.unwrap().api_id, 123);
    }

    #[test]
    fn missing_file_the_user_named_is_an_error() {
        let loaded = load_test(Some("/nonexistent/config.toml".into()), &CREDENTIALS);
        let error = loaded.config.unwrap_err().to_string();
        assert!(error.starts_with("Can't read /nonexistent/config.toml (named by --config):\n"), "{error}");
        let loaded = load_test(None, &[("FRILLOGRAM_CONFIG", "/nonexistent/config.toml"), CREDENTIALS[0]]);
        let error = loaded.config.unwrap_err().to_string();
        assert!(error.contains("(named by FRILLOGRAM_CONFIG)"), "{error}");
    }

    #[test]
    fn loads_the_named_file() {
        let file = TempFile::new(
            "load",
            &format!("[telegram]\napi_id = 9\napi_hash_command = \"echo {HASH}\"\n[chats]\nhistory_limit = 5\n"),
        );
        let loaded = load_test(Some(file.0.clone()), &[]);
        assert_eq!(loaded.settings.chats.history_limit, 5);
        let config = loaded.config.unwrap();
        assert_eq!((config.api_id, config.api_hash.as_str()), (9, HASH));
        // A bad file keeps the defaults for the error screen.
        let file = TempFile::new("bad", "[layout]\nsidebar_width = 1\n");
        let loaded = load_test(Some(file.0.clone()), &CREDENTIALS);
        assert_eq!(loaded.settings, Settings::default());
        assert!(matches!(loaded.config, Err(ConfigError::File(_))));
    }

    #[test]
    fn unknown_font_is_an_error() {
        let installed = ["Inter".to_string(), "Menlo".to_string()];
        let loaded = |family: &str| {
            let mut settings = Settings::default();
            settings.appearance.font_family = Some(family.into());
            Loaded { settings, path: Some(PATH.into()), config: config(&CREDENTIALS) }
        };
        let mut known = loaded("Inter");
        known.check_font(&installed);
        assert!(known.config.is_ok());
        let mut typo = loaded("inter");
        typo.check_font(&installed);
        let error = typo.config.unwrap_err().to_string();
        assert!(error.contains("isn't an installed font family") && error.contains("for example \"Inter\""), "{error}");
    }
}
