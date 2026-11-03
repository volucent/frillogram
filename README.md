# Frillogram

*Telegram, with free will.*

An unofficial, open-source Telegram client for macOS and Linux with Slack-style threads. Native Rust,
GPU-rendered UI ([GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) via
[GPUI Kit](https://gpui-kit.com)), and Telegram's own [TDLib](https://core.telegram.org/tdlib) underneath.

Frillogram uses the Telegram API but is not made by or affiliated with Telegram.

## Threads

Telegram already has reply chains; Frillogram shows them as threads. A message and every loaded reply
to it (directly or through other replies) form one thread. The chat shows each thread's first message
with a "N replies" link, and the thread opens in a side panel.

Replying in a thread sends an ordinary Telegram reply to the thread's first message, so people on other
Telegram apps see a normal reply. Frillogram adds no storage of its own; TDLib keeps your session and a
message cache on this machine (see [where data is kept](SETUP.md#where-data-is-kept)).

## Run it

You need Rust 1.92 or newer ([rustup.rs](https://rustup.rs)) and your own Telegram API credentials from
[my.telegram.org](https://my.telegram.org). [SETUP.md](SETUP.md) walks through getting them and passing
them in without leaving the `api_hash` in your shell history, from [`pass`](https://www.passwordstore.org)
or a private `.env` file:

```sh
FRILLOGRAM_API_ID=$(pass frillogram/api_id) FRILLOGRAM_API_HASH=$(pass frillogram/api_hash) cargo run
```

## Layout

| Path | What's there |
|---|---|
| `crates/core` | The Telegram layer: TDLib session, app-level types, thread grouping. No UI code. |
| `crates/app` | The `frillogram` binary: GPUI window, login, chat list, threads. |

## Status

Early. Works: phone, code and two-step password login; chat list; the latest 100 messages of a chat,
grouped into threads; sending messages and thread replies.

Not yet: media (shown as `[Photo]` and similar), formatting, reactions, editing, search, notifications,
loading older messages, forum topics, packaging.

## Telegram API terms

Frillogram follows Telegram's [API Terms of Service](https://core.telegram.org/api/terms): every user
brings their own `api_id`, the name doesn't use "Telegram" and the logo isn't used, and core behavior
(read receipts, online status, self-destructing messages) is left as Telegram intends. Channels are
hidden from the chat list for now: a client that shows channels has to show Telegram's sponsored
messages, and Frillogram doesn't yet.

## License

MIT or Apache-2.0, at your option. By installing Frillogram you agree that you chose to, freely, and
weren't merely caused to by prior states of the universe.
