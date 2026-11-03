use std::path::PathBuf;

use tdlib_rs::{enums, functions, types};

use crate::{Event, Msg};

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

impl Config {
    /// Reads `FRILLOGRAM_API_ID` and `FRILLOGRAM_API_HASH`. Each user gets
    /// their own pair at <https://my.telegram.org>; they're never committed.
    /// `FRILLOGRAM_DATA_DIR` overrides where the TDLib database lives.
    pub fn from_env() -> Result<Self, String> {
        let api_id = std::env::var("FRILLOGRAM_API_ID")
            .map_err(|_| "FRILLOGRAM_API_ID is not set".to_string())?
            .parse()
            .map_err(|_| "FRILLOGRAM_API_ID must be a number".to_string())?;
        let api_hash =
            std::env::var("FRILLOGRAM_API_HASH").map_err(|_| "FRILLOGRAM_API_HASH is not set".to_string())?;
        let data_dir = match std::env::var_os("FRILLOGRAM_DATA_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => dirs::data_local_dir().ok_or("no local data directory on this system")?.join("frillogram"),
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
                // `receive` blocks for up to 2s and also resolves pending
                // requests, so it must never wait on a request itself.
                loop {
                    let Some((update, _)) = tdlib_rs::receive() else { continue };
                    if let enums::Update::AuthorizationState(u) = &update
                        && matches!(u.authorization_state, enums::AuthorizationState::WaitTdlibParameters)
                    {
                        send_parameters(id, config.clone(), tx.clone());
                    }
                    let Some(event) = Event::from_update(update) else { continue };
                    if tx.send_blocking(event).is_err() {
                        break;
                    }
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

    /// Asks TDLib to deliver up to `limit` more chats of the main list as
    /// [`Event::Chat`] updates.
    pub async fn load_chats(self, limit: i32) -> Result<(), String> {
        functions::load_chats(Some(enums::ChatList::Main), limit, self.id).await.map_err(message)
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
            let want = (limit - messages.len()) as i32;
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

fn message(error: types::Error) -> String {
    error.message
}
