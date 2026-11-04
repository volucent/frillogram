# Setup

Frillogram doesn't ship Telegram API credentials. Telegram's
[API terms](https://core.telegram.org/api/terms) give each app its own `api_id`, so everyone who runs
Frillogram creates their own (it takes a few minutes and is free).

## 1. Get API credentials

1. Log in at [my.telegram.org](https://my.telegram.org) with your phone number. The login code arrives
   as a message in your Telegram app, not as an SMS.
2. Open **API development tools**.
3. Fill in the form:
   - **App title:** anything that doesn't contain "Telegram", for example `Frillogram`.
   - **Short name:** 5–32 letters or digits, for example `frillogram`.
   - **Platform:** Desktop.
   - **URL** and **Description:** optional.
4. Click **Create application** and note the **api_id** (a number) and **api_hash** (32 hex characters).

Each Telegram account can have one app; once created, the page shows the same credentials again.
If creating it fails with a bare "ERROR", it's usually the phone number: Telegram tends to refuse new
or VoIP numbers. An account that has been in use for a while normally works.

The `api_hash` is a secret: anyone with it can make requests as your app. Never commit it, paste it into
issues, or put it in screenshots.

## 2. Run Frillogram

You need Rust 1.92 or newer ([rustup.rs](https://rustup.rs)); on Linux, also the packages under
[Linux prerequisites](#linux-prerequisites).

The simplest way is the config file, with the `api_hash` kept in a password manager such as
[`pass`](https://www.passwordstore.org) and read by a command Frillogram runs on startup. No
environment variables are needed:

```sh
pass insert frillogram/api_hash
mkdir -p ~/.config/frillogram
$EDITOR ~/.config/frillogram/config.toml
```

Put your own `api_id` in the file:

```toml
[telegram]
api_id = 12345
api_hash_command = "pass frillogram/api_hash"
```

Then run it:

```sh
cargo run
```

`api_hash_command` is run with `sh -c`, and the first line it prints, without surrounding whitespace, is
the `api_hash`. If it fails or prints nothing, Frillogram shows its error output and doesn't connect. Any
command works, for example `security find-generic-password -s frillogram -w` with the macOS keychain. The
`api_hash` itself is never read from the config file, since config files often end up in published
dotfiles; Frillogram refuses to start if the file has an `api_hash` key.

### With environment variables instead

`FRILLOGRAM_API_ID` and `FRILLOGRAM_API_HASH` work too, and override the config file. Read them from a
password manager when launching:

```sh
pass insert frillogram/api_id
pass insert frillogram/api_hash
FRILLOGRAM_API_ID=$(pass frillogram/api_id) FRILLOGRAM_API_HASH=$(pass frillogram/api_hash) cargo run
```

Or put them in a `.env` file in the repository root (it's in `.gitignore`), readable only by you, and load
it into your shell. Create the file private first, so the hash is never readable by others, then add the
values in an editor; typed into the shell, even in a heredoc, they'd end up in its history. Note that
`install` replaces an existing `.env` with an empty file:

```sh
install -m 600 /dev/null .env
$EDITOR .env
```

The file should contain:

```sh
FRILLOGRAM_API_ID=12345
FRILLOGRAM_API_HASH=0123456789abcdef0123456789abcdef
```

Then load it and run:

```sh
set -a; source .env; set +a
cargo run
```

To also keep the hash out of the environment of build scripts, run `cargo build` first and then start
`target/debug/frillogram` with the variables set.

You can also pass them inline, but then the `api_hash` lands in your shell history:

```sh
FRILLOGRAM_API_ID=12345 FRILLOGRAM_API_HASH=0123456789abcdef0123456789abcdef cargo run
```

The first build downloads a prebuilt TDLib from the
[tdlib-rs project's GitHub releases](https://github.com/FedericoBruzzone/tdlib-rs/releases) and links it into
the binary, and takes a few minutes.

### Try it without an account

`frillogram --preview` (`cargo run -- --preview`) opens a window of made-up sample chats instead of
connecting to Telegram. It needs no credentials, so `api_hash_command` isn't run, and nothing is sent,
downloaded or stored: opening a chat shows its sample messages, and what you send is only added to the
window. It still reads your config file, and reports errors in it as usual, so it's a quick way to try
colors, fonts and sizes, or to look at the app on a machine without a Telegram login.

### Linux prerequisites

The Linux build hasn't been verified yet. TDLib is linked against LLVM's libc++, and GPUI's X11/Wayland
backend links libxcb, xkbcommon and fontconfig. On Debian or Ubuntu:

```sh
sudo apt install build-essential pkg-config libc++-dev libc++abi-dev \
  libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev libfontconfig-dev libfreetype-dev
```

At runtime GPUI renders through Vulkan (`libvulkan1` plus your GPU's driver, e.g. `mesa-vulkan-drivers`)
and loads `libwayland-client` when running under Wayland.

### ffmpeg, for GIFs (optional)

Telegram stores most GIFs as silent MP4 videos. Frillogram plays them inline by converting each one to a
GIF once with [ffmpeg](https://ffmpeg.org), at the size it's shown, and caches the result in the `tdlib`
folder ([where data is kept](#where-data-is-kept)). Real GIFs are shrunk the same way, since every frame
of a GIF is kept in memory. Without ffmpeg, everything else works: those MP4 GIFs show as stills that
open in your viewer when clicked, and real GIFs play as they are unless they're over 8 MB or Full HD. Install it with `brew install ffmpeg` on macOS,
`pacman -S ffmpeg` on Arch Linux or `sudo apt install ffmpeg` on Debian or Ubuntu. `[media] ffmpeg` in
the config file names another program or path, and `""` turns conversion off.

## 3. Log in

Frillogram asks for your phone number, then the login code, then your two-step verification password if
you have one set. Telegram usually sends the code as a message to your Telegram app on another device; if
you aren't logged in anywhere else, it may come by SMS or a phone call instead. The login screen says
which.

## Configuration

Frillogram reads one TOML file, and every setting has a default, so the file is optional. It's the first
of these that's set, on macOS and Linux alike:

1. `--config <path>` on the command line, e.g. `cargo run -- --config ~/frillogram-test.toml`.
2. The `FRILLOGRAM_CONFIG` environment variable.
3. `$XDG_CONFIG_HOME/frillogram/config.toml`.
4. `~/.config/frillogram/config.toml`.

A file named by `--config` or `FRILLOGRAM_CONFIG` has to exist; at the other two places, no file means
all defaults. [`config.example.toml`](config.example.toml) lists every key, commented out and set to its
default, with what it does and which values it takes; `frillogram --print-default-config` prints it, so
`cargo run -- --print-default-config > ~/.config/frillogram/config.toml` is a good start. Its sections
are:

- `[telegram]`: `api_id`, `api_hash_command` and `data_dir`, described above and below.
- `[appearance]`: `theme` (`"system"`, which follows the OS's light or dark appearance, `"light"` or
  `"dark"`), `accent` (a `"#rrggbb"` color for both themes, or `""` for each palette's own),
  `font_family`, `font_size`, `density` (`"comfortable"` or `"compact"`) and `avatar_size`.
- `[colors.light]` and `[colors.dark]`: each theme's palette as `"#rrggbb"` colors: `background`,
  `sidebar`, `surface`, `foreground`, `muted`, `border`, `accent`, `danger`, `avatar_colors` and
  `avatar_text`. The defaults are soft pastels with no pure white or black, which are easier on the
  eyes; a key left out keeps its own theme's default.
- `[layout]`: the window's size when it opens, and the widths of the chat list and the thread panel.
- `[time]`: the [strftime formats](https://docs.rs/chrono/latest/chrono/format/strftime/) of message
  times, today and older, and of the day separators between messages, this year and in other years.
- `[chats]`: `history_limit`, how many of the latest messages to load when a chat is opened, and
  `group_minutes`, how close together one sender's messages have to be to share a name and avatar.
- `[media]`: whether photos and GIFs download (`autoload`) and GIFs play (`autoplay_gifs`) on their own
  or on a click, their largest inline size (`max_width`, `max_height`), the `ffmpeg` that converts MP4
  GIFs, and `open_command`, the program a clicked photo or GIF opens in (`open` on macOS and `xdg-open`
  on Linux by default).

Frillogram only reads the file, never writes it, and doesn't watch it: restart Frillogram after changing
it. An unknown key, a wrong type or a value out of range is an error: Frillogram shows it with the file,
the line or key and the allowed values, and doesn't connect until it's fixed.

The environment variables below override the file's `[telegram]` keys. An empty or blank value counts as
unset, so a failed `$(pass …)` falls back to the file.

| Variable | Overrides | Meaning |
|---|---|---|
| `FRILLOGRAM_CONFIG` | | Where the config file is, instead of the default places. |
| `FRILLOGRAM_API_ID` | `api_id` | Your app's `api_id` from my.telegram.org. |
| `FRILLOGRAM_API_HASH` | `api_hash_command` | Your app's `api_hash` from my.telegram.org. When it's set, `api_hash_command` isn't run. |
| `FRILLOGRAM_DATA_DIR` | `data_dir` | An absolute path outside the repository to keep TDLib's database in (under `tdlib/`) instead of the default below. With a relative path, Frillogram shows an error and doesn't connect. |

An `api_id` and an `api_hash` are required, from either the file or the environment.

## Where data is kept

TDLib's database (your login session and message cache) lives in a `tdlib` folder inside
`~/Library/Application Support/frillogram` on macOS and `~/.local/share/frillogram` on Linux. Set
`data_dir` in `[telegram]` (where a leading `~/` means your home directory) or `FRILLOGRAM_DATA_DIR` to
use another folder instead of `frillogram`, for example a separate one for a second account or for
testing; the `tdlib` folder is created inside it. Use an absolute path outside the repository, so the
session never ends up in a commit and doesn't depend on where you launch from: with a relative one,
Frillogram shows an error and doesn't connect. An empty value means the default. It isn't encrypted yet.
Also in the `tdlib` folder, `gif-cache` holds GIFs converted by ffmpeg, and `pasted` holds images pasted
into a composer, each removed once its message is sent or has failed (or, failing that, by a paste 30
days later); both are safe to delete.

Deleting the `tdlib` folder only logs you out locally: the session stays active on Telegram's side. To end
it, terminate it from another Telegram app, in the **Devices** (mobile apps) or **Active Sessions**
(Telegram Desktop) list in Settings.
