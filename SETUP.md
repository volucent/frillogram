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

You need Rust ([rustup.rs](https://rustup.rs)). Pass the credentials as environment variables:

```sh
FRILLOGRAM_API_ID=12345 FRILLOGRAM_API_HASH=0123456789abcdef0123456789abcdef cargo run
```

The first build downloads a prebuilt TDLib and links it into the binary, and takes a few minutes.

To avoid typing them each time, keep them in a password manager and read them when launching, for example
with [`pass`](https://www.passwordstore.org):

```sh
pass insert frillogram/api_id
pass insert frillogram/api_hash
FRILLOGRAM_API_ID=$(pass frillogram/api_id) FRILLOGRAM_API_HASH=$(pass frillogram/api_hash) cargo run
```

Or put them in a `.env` file in the repository root (it's in `.gitignore`) and load it into your shell:

```sh
cat > .env <<'EOF'
FRILLOGRAM_API_ID=12345
FRILLOGRAM_API_HASH=0123456789abcdef0123456789abcdef
EOF
set -a; source .env; set +a
cargo run
```

## 3. Log in

Frillogram asks for your phone number, then the login code Telegram sends to your other devices, then your
two-step verification password if you have one set.

## Where data is kept

TDLib's database (your login session and message cache) lives in
`~/Library/Application Support/frillogram` on macOS and `~/.local/share/frillogram` on Linux. Set
`FRILLOGRAM_DATA_DIR` to keep it somewhere else, for example a separate folder for a second account or
for testing. It isn't encrypted yet. Deleting the folder logs you out of Frillogram on this machine.

| Variable | Required | Meaning |
|---|---|---|
| `FRILLOGRAM_API_ID` | yes | Your app's `api_id` from my.telegram.org. |
| `FRILLOGRAM_API_HASH` | yes | Your app's `api_hash` from my.telegram.org. |
| `FRILLOGRAM_DATA_DIR` | no | Where to keep TDLib's database instead of the default above. |
