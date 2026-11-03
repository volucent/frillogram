# AGENTS.md

Frillogram is a Telegram client: `crates/core` wraps TDLib and has no UI code, and `crates/app` is the GPUI
binary. README.md says what it does, SETUP.md how to run it, and `config.example.toml` lists every setting.

## The Arch Way

Frillogram is configured the way Arch Linux is: the user owns a plain-text file, and the program does what
that file says. Apply this to every change.

- **One file is the interface.** Anything a user could reasonably want different (colors, fonts, sizes,
  layout, time formats, media behavior, external programs) is a key in `config.toml`. Hard-coding a
  user-visible choice is a missing key.
- **Zero config works.** Every key has a built-in default, and a missing file means "all defaults".
- **`config.example.toml` is the reference.** It lists every key, commented out, set to its default, with a
  one-line explanation, like `/etc/pacman.conf`. A new or changed key lands in the parser, its default and
  this file in the same change.
- **Strict and explicit.** An unknown key, wrong type or bad value is an error that names the file, the key
  and what's allowed, shown on startup instead of connecting. Frillogram never guesses; the user fixes the
  file.
- **The file is the user's.** Frillogram reads it and never writes, migrates or "fixes" it.
- **Delegate to the user's tools.** Secrets come from a command the user picks (`api_hash_command`), files
  open with the user's opener, and video converts with the user's `ffmpeg`. Each is a configurable command
  line, and a missing tool degrades one feature with a clear message, never the whole app.
- **Simple over clever.** Write the smallest code that does the job; add an abstraction when a second real
  use appears.
- **Upstream behavior.** Telegram's API terms are part of the design (see README): users bring their own
  `api_id`, read receipts and online status work as Telegram intends, and channels stay hidden until
  sponsored messages are shown.

## Working in this repo

- Before calling a change done: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo test --workspace` all pass. The app needs real Telegram credentials to run, so logic gets unit
  tests in a `mod tests` beside it, and pure helpers are split out of GPUI code to make that possible.
- UI code only sees the small types in `crates/core/src/model.rs`; TDLib's generated types stay in core.
- Comments explain why, in a line or two, in plain sentences. Match the surrounding code.
- The `api_hash` and the TDLib data dir are secrets: keep them out of logs, errors, commits and the repo
  tree.
- Docs change with behavior: README, SETUP.md and `config.example.toml` describe what the code does now.
