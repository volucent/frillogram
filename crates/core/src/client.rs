use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use tdlib_rs::{enums, functions, types};

use crate::{AuthStep, Event, Msg};

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

/// Why [`Config::from_env`] failed, so the setup screen can say what to fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// `FRILLOGRAM_API_ID` or `FRILLOGRAM_API_HASH` is missing or malformed.
    Credentials(String),
    /// There's no usable place for TDLib's database.
    DataDir(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Credentials(error) | ConfigError::DataDir(error) => f.write_str(error),
        }
    }
}

impl Config {
    /// Reads `FRILLOGRAM_API_ID` and `FRILLOGRAM_API_HASH`. Each user gets
    /// their own pair at <https://my.telegram.org>; they're never committed.
    /// `FRILLOGRAM_DATA_DIR` overrides where the TDLib database lives.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_vars(|name| std::env::var_os(name), dirs::data_local_dir)
    }

    /// [`Config::from_env`] with the environment and the default data
    /// directory passed in, so it can be tested.
    fn from_vars(
        var: impl Fn(&str) -> Option<OsString>,
        default_dir: impl FnOnce() -> Option<PathBuf>,
    ) -> Result<Self, ConfigError> {
        let credentials = |error: String| ConfigError::Credentials(error);
        // An empty or blank value usually means a failed `$(pass …)`, so it
        // counts as unset.
        let text = |name: &str| {
            var(name)
                .map(OsString::into_string)
                .transpose()
                .map(|value| value.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()))
                .map_err(|_| credentials(format!("{name} is not valid UTF-8")))
        };
        let api_id = text("FRILLOGRAM_API_ID")?
            .ok_or_else(|| credentials("FRILLOGRAM_API_ID is not set".into()))?
            .parse()
            .map_err(|_| credentials("FRILLOGRAM_API_ID must be a number".into()))?;
        let api_hash =
            text("FRILLOGRAM_API_HASH")?.ok_or_else(|| credentials("FRILLOGRAM_API_HASH is not set".into()))?;
        // Not trimmed: spaces can be part of a path.
        let data_dir = match var("FRILLOGRAM_DATA_DIR").filter(|dir| !dir.to_string_lossy().trim().is_empty()) {
            // A relative path would depend on where Frillogram is launched
            // from, and could put the session inside a repository.
            Some(dir) if !Path::new(&dir).is_absolute() => {
                return Err(ConfigError::DataDir("FRILLOGRAM_DATA_DIR must be an absolute path".into()));
            }
            Some(dir) => PathBuf::from(dir),
            None => default_dir()
                .ok_or_else(|| ConfigError::DataDir("no local data directory on this system".into()))?
                .join("frillogram"),
        }
        .join("tdlib");
        Ok(Config { api_id, api_hash, data_dir })
    }
}

/// A TDLib client. Cheap to clone: it's only TDLib's client id.
#[derive(Debug, Clone, Copy)]
pub struct Telegram {
    id: i32,
}

impl Telegram {
    /// Creates the TDLib client and starts delivering its updates.
    pub fn start(config: Config) -> (Self, async_channel::Receiver<Event>) {
        let id = tdlib_rs::create_client();
        let (tx, rx) = async_channel::unbounded();

        std::thread::Builder::new()
            .name("tdlib-receive".into())
            .spawn(move || {
                let events = tx.clone();
                // `receive` blocks for up to 2s and also resolves pending
                // requests, so it must never wait on a request itself.
                // It keeps running after the event receiver is dropped, so
                // requests still in flight resolve; updates are then discarded.
                let receive = std::panic::AssertUnwindSafe(move || {
                    let mut forwarding = true;
                    loop {
                        let Some((update, _)) = tdlib_rs::receive() else { continue };
                        if !forwarding {
                            continue;
                        }
                        if let enums::Update::AuthorizationState(u) = &update
                            && matches!(u.authorization_state, enums::AuthorizationState::WaitTdlibParameters)
                        {
                            send_parameters(id, config.clone(), events.clone());
                        }
                        let Some(event) = Event::from_update(update) else { continue };
                        forwarding = events.send_blocking(event).is_ok();
                    }
                });
                // tdlib-rs panics on JSON it can't parse. Nothing would answer
                // requests after that, so tell the UI instead of hanging.
                // This only covers the receive thread: tdlib-rs's generated
                // request functions can still panic on the caller's thread
                // on a malformed response.
                if std::panic::catch_unwind(receive).is_err() {
                    let error = "TDLib stopped unexpectedly. Restart Frillogram to reconnect.";
                    let _ = tx.send_blocking(Event::Error(error.into()));
                    let _ = tx.send_blocking(Event::Auth(AuthStep::Closed));
                }
            })
            .expect("failed to spawn the TDLib receive thread");

        // TDLib emits nothing until it receives its first request.
        std::thread::spawn(move || {
            let _ = futures_executor::block_on(functions::set_log_verbosity_level(1, id));
        });

        (Telegram { id }, rx)
    }

    pub async fn submit_phone(self, phone: String) -> Result<(), String> {
        functions::set_authentication_phone_number(phone, None, self.id).await.map_err(message)
    }

    pub async fn submit_code(self, code: String) -> Result<(), String> {
        functions::check_authentication_code(code, self.id).await.map_err(message)
    }

    pub async fn submit_password(self, password: String) -> Result<(), String> {
        functions::check_authentication_password(password, self.id).await.map_err(message)
    }

    /// Asks TDLib to deliver every chat of the main list as [`Event::Chat`]
    /// updates, `limit` per request.
    pub async fn load_chats(self, limit: i32) -> Result<(), String> {
        // A request may load fewer chats than asked for while more remain;
        // only "Not Found" (404) means the list is exhausted.
        loop {
            match functions::load_chats(Some(enums::ChatList::Main), limit, self.id).await {
                Ok(()) => {}
                Err(error) if error.code == 404 => return Ok(()),
                Err(error) => return Err(message(error)),
            }
        }
    }

    /// Tells TDLib the chat is on screen, so it sends updates for it.
    pub async fn open_chat(self, chat_id: i64) -> Result<(), String> {
        functions::open_chat(chat_id, self.id).await.map_err(message)
    }

    /// Tells TDLib these messages are on screen, which marks them read.
    pub async fn view_messages(self, chat_id: i64, message_ids: Vec<i64>) -> Result<(), String> {
        functions::view_messages(chat_id, message_ids, None, false, self.id).await.map_err(message)
    }

    pub async fn close_chat(self, chat_id: i64) -> Result<(), String> {
        functions::close_chat(chat_id, self.id).await.map_err(message)
    }

    /// The latest `limit` messages of a chat, oldest first.
    pub async fn history(self, chat_id: i64, limit: usize) -> Result<Vec<Msg>, String> {
        let mut messages = Vec::new();
        let mut from = 0;
        // TDLib returns whatever it has locally first, often a single
        // message, so keep asking until the limit or the start of the chat.
        while messages.len() < limit {
            // TDLib returns at most 100 messages per request.
            let want = (limit - messages.len()).min(100) as i32;
            let enums::Messages::Messages(batch) =
                functions::get_chat_history(chat_id, from, 0, want, false, self.id).await.map_err(message)?;
            let batch: Vec<types::Message> = batch.messages.into_iter().flatten().collect();
            let Some(oldest) = batch.last() else { break };
            from = oldest.id;
            messages.extend(batch.into_iter().map(Msg::from));
        }
        messages.reverse();
        Ok(messages)
    }

    /// Sends a text message. It arrives back as [`Event::NewMessage`] while
    /// sending and [`Event::MessageSent`] once the server accepts it.
    pub async fn send_text(self, chat_id: i64, text: String, reply_to: Option<i64>) -> Result<(), String> {
        let reply_to = reply_to.map(|message_id| {
            enums::InputMessageReplyTo::Message(types::InputMessageReplyToMessage {
                message_id,
                quote: None,
                checklist_task_id: 0,
            })
        });
        let content = enums::InputMessageContent::InputMessageText(types::InputMessageText {
            text: types::FormattedText { text, entities: Vec::new() },
            link_preview_options: None,
            clear_draft: true,
        });
        functions::send_message(chat_id, None, reply_to, None, content, self.id).await.map(|_| ()).map_err(message)
    }
}

/// Answers TDLib's `authorizationStateWaitTdlibParameters`. Runs on its own
/// thread because the receive thread has to keep running to resolve it.
fn send_parameters(id: i32, config: Config, tx: async_channel::Sender<Event>) {
    std::thread::spawn(move || {
        if let Err(error) = create_private_dir(&config.data_dir) {
            let _ = tx.send_blocking(Event::Error(format!("Can't create {}: {error}", config.data_dir.display())));
            return;
        }
        let result = futures_executor::block_on(functions::set_tdlib_parameters(
            false,
            config.data_dir.join("db").to_string_lossy().into_owned(),
            config.data_dir.join("files").to_string_lossy().into_owned(),
            String::new(),
            true,
            true,
            true,
            false,
            config.api_id,
            config.api_hash,
            "en".into(),
            "Desktop".into(),
            String::new(),
            env!("CARGO_PKG_VERSION").into(),
            id,
        ));
        if let Err(error) = result {
            let _ = tx.send_blocking(Event::Error(error.message));
        }
    });
}

/// Creates `dir` readable only by the current user, since TDLib's database
/// in it isn't encrypted.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        // Also tightens a directory created before this was in place.
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

fn message(error: types::Error) -> String {
    error.message
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn config(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let vars: Vec<(&str, OsString)> = vars.iter().map(|&(name, value)| (name, value.into())).collect();
        config_os(vars)
    }

    fn config_os(vars: Vec<(&str, OsString)>) -> Result<Config, ConfigError> {
        let vars: HashMap<&str, OsString> = vars.into_iter().collect();
        Config::from_vars(|name| vars.get(name).cloned(), || Some(PathBuf::from("/data")))
    }

    fn credentials_error(error: &str) -> ConfigError {
        ConfigError::Credentials(error.into())
    }

    const CREDENTIALS: [(&str, &str); 2] = [("FRILLOGRAM_API_ID", " 123 "), ("FRILLOGRAM_API_HASH", " abc\n")];

    #[test]
    fn credentials_are_trimmed() {
        let config = config(&CREDENTIALS).unwrap();
        assert_eq!((config.api_id, config.api_hash.as_str()), (123, "abc"));
        assert_eq!(config.data_dir, Path::new("/data/frillogram/tdlib"));
    }

    #[test]
    fn blank_credentials_count_as_unset() {
        let error = config(&[("FRILLOGRAM_API_ID", "  "), CREDENTIALS[1]]).unwrap_err();
        assert_eq!(error, credentials_error("FRILLOGRAM_API_ID is not set"));
        let error = config(&[CREDENTIALS[0], ("FRILLOGRAM_API_HASH", "")]).unwrap_err();
        assert_eq!(error, credentials_error("FRILLOGRAM_API_HASH is not set"));
        let error = config(&[("FRILLOGRAM_API_ID", "12a"), CREDENTIALS[1]]).unwrap_err();
        assert_eq!(error, credentials_error("FRILLOGRAM_API_ID must be a number"));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_credentials_are_reported_as_such() {
        use std::os::unix::ffi::OsStringExt;
        let invalid = || OsString::from_vec(vec![b'1', 0xff]);
        let error = config_os(vec![("FRILLOGRAM_API_ID", invalid()), (CREDENTIALS[1].0, CREDENTIALS[1].1.into())]);
        assert_eq!(error.unwrap_err(), credentials_error("FRILLOGRAM_API_ID is not valid UTF-8"));
        let error = config_os(vec![(CREDENTIALS[0].0, CREDENTIALS[0].1.into()), ("FRILLOGRAM_API_HASH", invalid())]);
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
}
