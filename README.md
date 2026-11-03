# Frillogram

*Telegram, with free will.*

An unofficial, open-source Telegram client for macOS and Linux with Slack-style threads. Native Rust,
GPU-rendered UI ([GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) via
[GPUI Kit](https://gpui-kit.com)), and Telegram's own [TDLib](https://core.telegram.org/tdlib) underneath.

Frillogram uses the Telegram API but is not made by or affiliated with Telegram.

## Threads

Telegram already has reply chains; Frillogram shows them as threads. A message and every loaded reply
to it (directly or through other replies) form one thread. The chat shows each thread's first message
with a "N replies" link, and the thread opens in a side panel. Hovering any message shows a button
that opens its thread, or starts one.

Replying in a thread sends an ordinary Telegram reply to the thread's first message, so people on other
Telegram apps see a normal reply. Frillogram keeps no database of its own. TDLib keeps your session and a message
cache on this machine, and Frillogram adds only a small cache of converted GIFs and your pasted images, both
inside its `tdlib` folder (see [where data is kept](SETUP.md#where-data-is-kept)).

## Run it

You need Rust 1.92 or newer ([rustup.rs](https://rustup.rs)) and your own Telegram API credentials from
[my.telegram.org](https://my.telegram.org). [SETUP.md](SETUP.md) walks through getting them. The simplest
way to pass them in is `~/.config/frillogram/config.toml`, with the `api_hash` read from
[`pass`](https://www.passwordstore.org) or another password manager, so it never sits in the file:

```toml
[telegram]
api_id = 12345
api_hash_command = "pass frillogram/api_hash"
```

Then `cargo run`. Environment variables work too, and SETUP.md covers them.

## Configuration

Everything you can change (theme, accent color, font, sizes, time formats, how much history to load, how
photos and GIFs load and which programs open and convert them) is a key in that one file, and every key has a default, so the file is optional.
[`config.example.toml`](config.example.toml) lists them all, and [SETUP.md](SETUP.md#configuration) says
where the file goes and how it's read.

## Layout

| Path | What's there |
|---|---|
| `crates/core` | The Telegram layer: TDLib session, app-level types, thread grouping. No UI code. |
| `crates/app` | The `frillogram` binary: GPUI window, login, chat list, threads. |

## Status

Early. Works: phone, code and two-step password login; chat list; the latest messages of a chat (100 by
default), grouped into threads; sending messages and thread replies; photos and GIFs shown inline, and
sending images by attaching, dropping or pasting them. Only photos and GIFs render inline: video, voice
messages, files and stickers keep their `[Video]`-style labels. Telegram stores most GIFs as silent MP4s,
which play inline once your `ffmpeg` converts them; without it they show as stills that open in your
viewer. Spoiler media stays covered until you click it, and media from chats that restrict saving never
opens outside Frillogram.

Not yet: other media, formatting, reactions, editing, search, notifications, loading older messages,
forum topics, packaging.

## Telegram API terms

Frillogram follows Telegram's [API Terms of Service](https://core.telegram.org/api/terms): every user
brings their own `api_id`, the name doesn't use "Telegram" and the logo isn't used, and core behavior
(read receipts, online status, self-destructing messages) is left as Telegram intends. Channels are
hidden from the chat list for now: a client that shows channels has to show Telegram's sponsored
messages, and Frillogram doesn't yet.

## License

MIT or Apache-2.0, at your option. By installing Frillogram you agree that you chose to, freely, and
weren't merely caused to by prior states of the universe.
