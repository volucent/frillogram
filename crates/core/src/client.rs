use std::path::{Path, PathBuf};

use tdlib_rs::{enums, functions, types};

use crate::{AuthStep, Config, Event, Msg, Upload};

/// TDLib's download priority, 1 to 32, for files shown in the open chat.
const DOWNLOAD_PRIORITY: i32 = 16;

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
        let content = enums::InputMessageContent::InputMessageText(types::InputMessageText {
            text: types::FormattedText { text, entities: Vec::new() },
            link_preview_options: None,
            clear_draft: true,
        });
        self.send(chat_id, content, reply_to).await.map(|_| ())
    }

    /// Sends the local file at `path` as a photo or GIF of `size` (width, height)
    /// in px (0 if unknown), with `caption` ("" for none). Pass the real size: in
    /// a secret chat the file is end-to-end encrypted, so the server can't read
    /// it and the recipient would get 0 x 0. Like [`Telegram::send_text`], this returns once TDLib queued it;
    /// the upload follows, and a failed one arrives as [`Event::MessageSendFailed`]. Returns the queued
    /// message's temporary id, which that event and [`Event::MessageSent`] carry as `old_id`.
    pub async fn send_file(
        self,
        chat_id: i64,
        path: PathBuf,
        upload: Upload,
        size: (u32, u32),
        caption: String,
        reply_to: Option<i64>,
    ) -> Result<i64, String> {
        let file = enums::InputFile::Local(types::InputFileLocal { path: path.to_string_lossy().into_owned() });
        let caption = (!caption.is_empty()).then(|| types::FormattedText { text: caption, entities: Vec::new() });
        let (width, height) = (i32::try_from(size.0).unwrap_or(0), i32::try_from(size.1).unwrap_or(0));
        let content = match upload {
            Upload::Photo => enums::InputMessageContent::InputMessagePhoto(types::InputMessagePhoto {
                photo: file,
                thumbnail: None,
                added_sticker_file_ids: Vec::new(),
                width,
                height,
                caption,
                show_caption_above_media: false,
                self_destruct_type: None,
                has_spoiler: false,
            }),
            Upload::Animation => enums::InputMessageContent::InputMessageAnimation(types::InputMessageAnimation {
                animation: file,
                thumbnail: None,
                added_sticker_file_ids: Vec::new(),
                duration: 0,
                width,
                height,
                caption,
                show_caption_above_media: false,
                has_spoiler: false,
            }),
        };
        self.send(chat_id, content, reply_to).await
    }

    async fn send(
        self,
        chat_id: i64,
        content: enums::InputMessageContent,
        reply_to: Option<i64>,
    ) -> Result<i64, String> {
        let reply_to = reply_to.map(|message_id| {
            enums::InputMessageReplyTo::Message(types::InputMessageReplyToMessage {
                message_id,
                quote: None,
                checklist_task_id: 0,
            })
        });
        functions::send_message(chat_id, None, reply_to, None, content, self.id)
            .await
            .map(|enums::Message::Message(message)| message.id)
            .map_err(message)
    }

    /// Downloads a file, returning once it's complete, and returns its local
    /// path. A file that's already downloaded returns right away.
    pub async fn download(self, file_id: i32) -> Result<PathBuf, String> {
        let enums::File::File(file) =
            functions::download_file(file_id, DOWNLOAD_PRIORITY, 0, 0, true, self.id).await.map_err(message)?;
        // A synchronous download also returns when it was canceled.
        if file.local.is_downloading_completed && !file.local.path.is_empty() {
            Ok(PathBuf::from(file.local.path))
        } else {
            Err("The download didn't finish.".into())
        }
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
