//! App-level types. TDLib's generated types are huge and change with every
//! TDLib release, so the UI only ever sees these small, stable shapes.

use std::path::Path;

use base64::Engine as _;
use tdlib_rs::{enums, types};

/// Who sent a message: a user, or a chat posting on its own behalf
/// (channels, anonymous group admins).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SenderId {
    User(i64),
    Chat(i64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Msg {
    pub id: i64,
    pub chat_id: i64,
    pub sender: SenderId,
    /// Unix timestamp.
    pub date: i32,
    /// The text, or a media message's caption, which may be empty. Media
    /// that isn't shown inline is a label here instead, like `[Video]`.
    pub text: String,
    /// A photo or GIF shown inline.
    pub media: Option<Media>,
    /// The message this one replies to, if it is in the same chat.
    pub reply_to: Option<i64>,
    pub outgoing: bool,
    pub send_state: SendState,
    /// False for protected content: the chat forbids saving and forwarding,
    /// so files from it mustn't be opened outside Frillogram.
    pub can_be_saved: bool,
}

/// Media shown inline. Other kinds stay labels in [`Msg::text`].
#[derive(Debug, Clone, PartialEq)]
pub enum Media {
    Photo(Photo),
    Animation(Animation),
}

impl Media {
    /// The size the sender gave, in px; either may be 0 if unknown.
    pub fn size(&self) -> (u32, u32) {
        match self {
            Media::Photo(p) => (p.width, p.height),
            Media::Animation(a) => (a.width, a.height),
        }
    }

    /// Whether the sender hid it behind a spoiler, to be blurred until revealed.
    pub fn has_spoiler(&self) -> bool {
        match self {
            Media::Photo(p) => p.has_spoiler,
            Media::Animation(a) => a.has_spoiler,
        }
    }

    /// A tiny JPEG of the image, to blur up while the real one loads.
    pub fn minithumbnail(&self) -> Option<&[u8]> {
        match self {
            Media::Photo(p) => p.minithumbnail.as_deref(),
            Media::Animation(a) => a.minithumbnail.as_deref(),
        }
    }
}

/// One size of a photo, picked to fit the message list.
#[derive(Debug, Clone, PartialEq)]
pub struct Photo {
    /// TDLib's file id, valid for this session only.
    pub file_id: i32,
    pub width: u32,
    pub height: u32,
    pub minithumbnail: Option<Vec<u8>>,
    pub has_spoiler: bool,
}

/// A GIF, which Telegram usually stores as a silent MP4.
#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    pub file_id: i32,
    /// Stays the same across sessions, unlike `file_id`; may be empty.
    pub unique_id: String,
    pub width: u32,
    pub height: u32,
    /// Usually "image/gif" or "video/mp4".
    pub mime_type: String,
    pub thumbnail: Option<Thumbnail>,
    pub minithumbnail: Option<Vec<u8>>,
    pub has_spoiler: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Thumbnail {
    pub file_id: i32,
    pub format: ThumbnailFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailFormat {
    Jpeg,
    Png,
    Webp,
    /// A still GIF.
    Gif,
    Mpeg4,
    Other,
}

impl ThumbnailFormat {
    /// Whether it's a still image that can be drawn as it is.
    pub fn is_image(self) -> bool {
        matches!(self, ThumbnailFormat::Jpeg | ThumbnailFormat::Png | ThumbnailFormat::Webp | ThumbnailFormat::Gif)
    }
}

/// How a local file is sent: as a photo or as a GIF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upload {
    Photo,
    Animation,
}

impl Upload {
    /// What `path` is sent as, by its extension; `None` if it can't be sent.
    pub fn for_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "jpg" | "jpeg" | "png" | "webp" => Some(Upload::Photo),
            "gif" => Some(Upload::Animation),
            _ => None,
        }
    }
}

/// Whether a message has reached Telegram's servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendState {
    Sent,
    /// Sent from here and still on its way.
    Pending,
    /// Sent from here and rejected; TDLib keeps it in the chat.
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatSummary {
    pub id: i64,
    pub title: String,
    pub last_message: Option<String>,
    pub unread: i32,
    /// Position in the main chat list; 0 means the chat is not in it.
    /// Chats sort by `(order, id)` descending, which also puts pinned chats first.
    pub order: i64,
    pub is_channel: bool,
}

/// What the login screen should ask for next.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthStep {
    /// TDLib is starting up or applying parameters; nothing to ask yet.
    Starting,
    Phone,
    Code {
        hint: String,
    },
    Password {
        hint: String,
    },
    /// A step Frillogram can't handle yet (QR or email login, sign-up, Premium purchase).
    Unsupported(String),
    Ready,
    Closed,
}

/// Updates pushed from TDLib, already reduced to what the UI needs.
#[derive(Debug, Clone)]
pub enum Event {
    Auth(AuthStep),
    Chat(ChatSummary),
    ChatTitle {
        chat_id: i64,
        title: String,
    },
    ChatLastMessage {
        chat_id: i64,
        preview: Option<String>,
        order: i64,
    },
    ChatOrder {
        chat_id: i64,
        order: i64,
    },
    ChatUnread {
        chat_id: i64,
        unread: i32,
    },
    NewMessage(Msg),
    /// A message sent from here was accepted by the server and got its final id.
    MessageSent {
        old_id: i64,
        message: Msg,
    },
    /// A message sent from here was rejected, with Telegram's reason. TDLib
    /// keeps it in the chat as `message`, under a new id.
    MessageSendFailed {
        old_id: i64,
        message: Msg,
        error: String,
    },
    /// Messages left the chat for good: deleted (not just dropped from TDLib's
    /// cache), or sent from here and scheduled instead.
    MessagesDeleted {
        chat_id: i64,
        message_ids: Vec<i64>,
    },
    /// A message's content changed, e.g. after an edit, which can also
    /// swap its media.
    MessageContent {
        chat_id: i64,
        message_id: i64,
        text: String,
        media: Option<Media>,
    },
    User {
        id: i64,
        name: String,
    },
    Error(String),
}

impl Event {
    pub(crate) fn from_update(update: enums::Update) -> Option<Self> {
        use enums::Update as U;
        Some(match update {
            U::AuthorizationState(u) => Event::Auth(AuthStep::from(u.authorization_state)),
            U::NewChat(u) => Event::Chat(ChatSummary::from(u.chat)),
            U::ChatTitle(u) => Event::ChatTitle { chat_id: u.chat_id, title: u.title },
            // `positions` is the chat's full list, so a missing Main entry means 0.
            U::ChatLastMessage(u) => Event::ChatLastMessage {
                chat_id: u.chat_id,
                preview: u.last_message.as_ref().map(preview),
                order: main_list_order(&u.positions),
            },
            U::ChatDraftMessage(u) => Event::ChatOrder { chat_id: u.chat_id, order: main_list_order(&u.positions) },
            U::ChatPosition(u) => match u.position.list {
                enums::ChatList::Main => Event::ChatOrder { chat_id: u.chat_id, order: u.position.order },
                _ => return None,
            },
            U::ChatReadInbox(u) => Event::ChatUnread { chat_id: u.chat_id, unread: u.unread_count },
            // Scheduled messages (date 0) aren't part of the chat history.
            U::NewMessage(u) if u.message.scheduling_state.is_none() => Event::NewMessage(Msg::from(u.message)),
            // A sent message can come back scheduled, e.g. a video waiting to
            // be processed, so its pending copy leaves the history.
            U::MessageSendSucceeded(u) if u.message.scheduling_state.is_some() => {
                Event::MessagesDeleted { chat_id: u.message.chat_id, message_ids: vec![u.old_message_id] }
            }
            U::MessageSendFailed(u) if u.message.scheduling_state.is_some() => {
                Event::MessagesDeleted { chat_id: u.message.chat_id, message_ids: vec![u.old_message_id] }
            }
            U::MessageSendSucceeded(u) => {
                Event::MessageSent { old_id: u.old_message_id, message: Msg::from(u.message) }
            }
            U::MessageSendFailed(u) => Event::MessageSendFailed {
                old_id: u.old_message_id,
                message: Msg::from(u.message),
                error: u.error.message,
            },
            U::DeleteMessages(u) if u.is_permanent => {
                Event::MessagesDeleted { chat_id: u.chat_id, message_ids: u.message_ids }
            }
            U::MessageContent(u) => {
                let (text, media) = body(&u.new_content);
                Event::MessageContent { chat_id: u.chat_id, message_id: u.message_id, text, media }
            }
            U::User(u) => Event::User { id: u.user.id, name: user_name(&u.user) },
            _ => return None,
        })
    }
}

impl From<enums::AuthorizationState> for AuthStep {
    fn from(state: enums::AuthorizationState) -> Self {
        use enums::AuthorizationState as A;
        match state {
            A::WaitTdlibParameters => AuthStep::Starting,
            A::WaitPhoneNumber => AuthStep::Phone,
            A::WaitCode(s) => AuthStep::Code { hint: code_hint(&s.code_info) },
            A::WaitPassword(s) => AuthStep::Password { hint: s.password_hint },
            A::WaitOtherDeviceConfirmation(_) => AuthStep::Unsupported("QR code login isn't supported yet.".into()),
            A::WaitEmailAddress(_) | A::WaitEmailCode(_) => {
                AuthStep::Unsupported("Email login isn't supported yet.".into())
            }
            A::WaitRegistration(_) => {
                AuthStep::Unsupported("This number has no Telegram account. Sign up in an official app first.".into())
            }
            A::WaitPremiumPurchase(_) => {
                AuthStep::Unsupported("Telegram requires Premium to log in from this number.".into())
            }
            A::Ready => AuthStep::Ready,
            A::LoggingOut | A::Closing => AuthStep::Starting,
            A::Closed => AuthStep::Closed,
        }
    }
}

impl From<types::Chat> for ChatSummary {
    fn from(chat: types::Chat) -> Self {
        ChatSummary {
            id: chat.id,
            title: chat.title,
            last_message: chat.last_message.as_ref().map(preview),
            unread: chat.unread_count,
            order: main_list_order(&chat.positions),
            is_channel: matches!(chat.r#type, enums::ChatType::Supergroup(ref s) if s.is_channel),
        }
    }
}

impl From<types::Message> for Msg {
    fn from(message: types::Message) -> Self {
        let reply_to = match &message.reply_to {
            // A reply to a message in another chat (chat_id differs) can't be
            // threaded here; 0 means the original is unknown.
            Some(enums::MessageReplyTo::Message(r)) if r.chat_id == message.chat_id && r.message_id != 0 => {
                Some(r.message_id)
            }
            _ => None,
        };
        let (text, media) = body(&message.content);
        Msg {
            id: message.id,
            chat_id: message.chat_id,
            sender: match &message.sender_id {
                enums::MessageSender::User(u) => SenderId::User(u.user_id),
                enums::MessageSender::Chat(c) => SenderId::Chat(c.chat_id),
            },
            date: message.date,
            text,
            media,
            reply_to,
            outgoing: message.is_outgoing,
            send_state: match message.sending_state {
                None => SendState::Sent,
                Some(enums::MessageSendingState::Pending(_)) => SendState::Pending,
                Some(enums::MessageSendingState::Failed(_)) => SendState::Failed,
            },
            can_be_saved: message.can_be_saved,
        }
    }
}

/// Where the login code went, e.g. "Code sent by SMS to +1 555 123 4567".
fn code_hint(info: &types::AuthenticationCodeInfo) -> String {
    use enums::AuthenticationCodeType as T;
    let phone = &info.phone_number;
    match &info.r#type {
        T::TelegramMessage(_) => format!("Code sent to your Telegram app on another device ({phone})."),
        T::Sms(_) | T::SmsWord(_) | T::SmsPhrase(_) => format!("Code sent by SMS to {phone}."),
        T::Call(_) => format!("You'll get a call at {phone} with the code."),
        T::FlashCall(_) | T::MissedCall(_) => format!("Code sent to {phone} as a phone call."),
        T::Fragment(_) => format!("Code sent to {phone} on fragment.com."),
        _ => format!("Code sent to {phone}."),
    }
}

/// The chat's order in the main list, or 0 if it isn't in it.
fn main_list_order(positions: &[types::ChatPosition]) -> i64 {
    positions.iter().find(|p| matches!(p.list, enums::ChatList::Main)).map_or(0, |p| p.order)
}

fn user_name(user: &types::User) -> String {
    let name = format!("{} {}", user.first_name, user.last_name);
    let name = name.trim();
    if name.is_empty() { "Deleted Account".into() } else { name.into() }
}

fn preview(message: &types::Message) -> String {
    content_text(&message.content).lines().next().unwrap_or_default().to_string()
}

/// A message's text and inline media. A photo or GIF leaves only its
/// caption as text; other media stays a label. Self-destructing media is
/// never shown: it stays a label, to be opened in an official app.
fn body(content: &enums::MessageContent) -> (String, Option<Media>) {
    use enums::MessageContent as C;
    let media = match content {
        C::MessagePhoto(m) if !m.is_secret => {
            photo(&m.photo, m.has_spoiler).map(|photo| (Media::Photo(photo), &m.caption))
        }
        C::MessageAnimation(m) if !m.is_secret => {
            Some((Media::Animation(animation(&m.animation, m.has_spoiler)), &m.caption))
        }
        _ => None,
    };
    match media {
        Some((media, caption)) => (caption.text.clone(), Some(media)),
        None => (content_text(content), None),
    }
}

/// The photo at the size the message list draws it, if it has any size.
fn photo(photo: &types::Photo, has_spoiler: bool) -> Option<Photo> {
    let size = pick_photo_size(&photo.sizes)?;
    Some(Photo {
        file_id: size.photo.id,
        width: size.width.max(0) as u32,
        height: size.height.max(0) as u32,
        minithumbnail: minithumbnail(photo.minithumbnail.as_ref()),
        has_spoiler,
    })
}

/// Longest side TDLib's photo sizes are picked by: enough for an inline image
/// on a high-density screen, without downloading the original.
const PHOTO_MAX_SIDE: i32 = 1280;

/// The largest size whose longer side is at most [`PHOTO_MAX_SIDE`], or else
/// the smallest.
fn pick_photo_size(sizes: &[types::PhotoSize]) -> Option<&types::PhotoSize> {
    let longer_side = |size: &types::PhotoSize| size.width.max(size.height);
    let area = |size: &types::PhotoSize| i64::from(size.width) * i64::from(size.height);
    sizes
        .iter()
        .filter(|size| longer_side(size) <= PHOTO_MAX_SIDE)
        .max_by_key(|size| area(size))
        .or_else(|| sizes.iter().min_by_key(|size| area(size)))
}

fn animation(animation: &types::Animation, has_spoiler: bool) -> Animation {
    use enums::ThumbnailFormat as F;
    Animation {
        file_id: animation.animation.id,
        unique_id: animation.animation.remote.unique_id.clone(),
        width: animation.width.max(0) as u32,
        height: animation.height.max(0) as u32,
        mime_type: animation.mime_type.clone(),
        thumbnail: animation.thumbnail.as_ref().map(|thumbnail| Thumbnail {
            file_id: thumbnail.file.id,
            format: match thumbnail.format {
                F::Jpeg => ThumbnailFormat::Jpeg,
                F::Png => ThumbnailFormat::Png,
                F::Webp => ThumbnailFormat::Webp,
                F::Gif => ThumbnailFormat::Gif,
                F::Mpeg4 => ThumbnailFormat::Mpeg4,
                _ => ThumbnailFormat::Other,
            },
        }),
        minithumbnail: minithumbnail(animation.minithumbnail.as_ref()),
        has_spoiler,
    }
}

/// The minithumbnail's JPEG bytes. TDLib's JSON sends bytes as base64; a
/// thumbnail that doesn't decode is just left out.
fn minithumbnail(minithumbnail: Option<&types::Minithumbnail>) -> Option<Vec<u8>> {
    let data = &minithumbnail?.data;
    base64::engine::general_purpose::STANDARD.decode(data).ok().filter(|bytes| !bytes.is_empty())
}

const SECRET_PHOTO: &str = "[Self-destructing photo, open it in an official app]";
const SECRET_GIF: &str = "[Self-destructing GIF, open it in an official app]";

/// Text for a message body, and the chat list's preview of it. Media becomes
/// a label followed by its caption.
fn content_text(content: &enums::MessageContent) -> String {
    use enums::MessageContent as C;
    let with_caption = |label: &str, caption: &types::FormattedText| {
        if caption.text.is_empty() { label.to_string() } else { format!("{label} {}", caption.text) }
    };
    match content {
        C::MessageText(m) => m.text.text.clone(),
        C::MessagePhoto(m) if m.is_secret => with_caption(SECRET_PHOTO, &m.caption),
        C::MessagePhoto(m) => with_caption("[Photo]", &m.caption),
        C::MessageVideo(m) => with_caption("[Video]", &m.caption),
        C::MessageAnimation(m) if m.is_secret => with_caption(SECRET_GIF, &m.caption),
        C::MessageAnimation(m) => with_caption("[GIF]", &m.caption),
        C::MessageDocument(m) => with_caption("[File]", &m.caption),
        C::MessageAudio(m) => with_caption("[Audio]", &m.caption),
        C::MessageVoiceNote(m) => with_caption("[Voice message]", &m.caption),
        C::MessageVideoNote(_) => "[Video message]".into(),
        C::MessageSticker(m) => format!("[Sticker {}]", m.sticker.emoji),
        C::MessageAnimatedEmoji(m) => m.emoji.clone(),
        C::MessagePoll(_) => "[Poll]".into(),
        C::MessageLocation(_) | C::MessageVenue(_) => "[Location]".into(),
        C::MessageContact(_) => "[Contact]".into(),
        _ => "[Unsupported message]".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAT: i64 = 7;

    fn message(id: i64, scheduling_state: Option<enums::MessageSchedulingState>) -> types::Message {
        types::Message {
            id,
            sender_id: enums::MessageSender::User(types::MessageSenderUser { user_id: 1 }),
            chat_id: CHAT,
            sending_state: None,
            scheduling_state,
            is_outgoing: true,
            is_pinned: false,
            is_from_offline: false,
            can_be_saved: true,
            has_timestamped_media: false,
            is_channel_post: false,
            is_paid_star_suggested_post: false,
            is_paid_ton_suggested_post: false,
            contains_unread_mention: false,
            date: 0,
            edit_date: 0,
            forward_info: None,
            import_info: None,
            interaction_info: None,
            unread_reactions: Vec::new(),
            fact_check: None,
            suggested_post_info: None,
            reply_to: None,
            topic_id: None,
            self_destruct_type: None,
            self_destruct_in: 0.,
            auto_delete_in: 0.,
            via_bot_user_id: 0,
            sender_business_bot_user_id: 0,
            sender_boost_count: 0,
            paid_message_star_count: 0,
            author_signature: String::new(),
            media_album_id: 0,
            effect_id: 0,
            restriction_info: None,
            summary_language_code: String::new(),
            content: enums::MessageContent::MessageUnsupported,
            reply_markup: None,
        }
    }

    fn scheduled() -> Option<enums::MessageSchedulingState> {
        Some(enums::MessageSchedulingState::SendWhenOnline)
    }

    fn sent(message: types::Message) -> Option<Event> {
        Event::from_update(enums::Update::MessageSendSucceeded(types::UpdateMessageSendSucceeded {
            message,
            old_message_id: 1,
        }))
    }

    fn failed(message: types::Message) -> Option<Event> {
        Event::from_update(enums::Update::MessageSendFailed(types::UpdateMessageSendFailed {
            message,
            old_message_id: 1,
            error: types::Error { code: 400, message: "Nope".into() },
        }))
    }

    fn deleted_pending_copy(event: Option<Event>) -> bool {
        matches!(event, Some(Event::MessagesDeleted { chat_id: CHAT, message_ids }) if message_ids == [1])
    }

    #[test]
    fn scheduled_new_messages_are_dropped() {
        let new = |message| Event::from_update(enums::Update::NewMessage(types::UpdateNewMessage { message }));
        assert!(new(message(2, scheduled())).is_none());
        assert!(matches!(new(message(2, None)), Some(Event::NewMessage(Msg { id: 2, .. }))));
    }

    #[test]
    fn sends_that_end_up_scheduled_delete_the_pending_copy() {
        assert!(deleted_pending_copy(sent(message(2, scheduled()))));
        assert!(deleted_pending_copy(failed(message(2, scheduled()))));
        assert!(matches!(sent(message(2, None)), Some(Event::MessageSent { old_id: 1, message: Msg { id: 2, .. } })));
        assert!(matches!(failed(message(2, None)), Some(Event::MessageSendFailed { old_id: 1, .. })));
    }

    fn file(id: i32) -> types::File {
        types::File { id, ..Default::default() }
    }

    fn size(id: i32, width: i32, height: i32) -> types::PhotoSize {
        types::PhotoSize { r#type: "x".into(), photo: file(id), width, height, progressive_sizes: Vec::new() }
    }

    fn caption(text: &str) -> types::FormattedText {
        types::FormattedText { text: text.into(), entities: Vec::new() }
    }

    fn with_content(content: enums::MessageContent) -> Msg {
        Msg::from(types::Message { content, ..message(2, None) })
    }

    #[test]
    fn photo_size_is_the_largest_that_fits_else_the_smallest() {
        let sizes = [size(1, 90, 60), size(2, 1280, 853), size(3, 800, 533), size(4, 2560, 1706)];
        assert_eq!(pick_photo_size(&sizes).map(|s| s.photo.id), Some(2));
        // Portrait: the longer side is the height.
        let sizes = [size(1, 853, 1280), size(2, 1000, 1500)];
        assert_eq!(pick_photo_size(&sizes).map(|s| s.photo.id), Some(1));
        let too_big = [size(1, 4000, 3000), size(2, 2000, 1500), size(3, 3000, 2000)];
        assert_eq!(pick_photo_size(&too_big).map(|s| s.photo.id), Some(2));
        assert!(pick_photo_size(&[]).is_none());
    }

    #[test]
    fn photo_message_keeps_its_caption_as_text() {
        // "AQID" is base64 for the bytes 1, 2, 3.
        let mini = types::Minithumbnail { width: 40, height: 30, data: "AQID".into() };
        let photo = types::Photo {
            has_stickers: false,
            minithumbnail: Some(mini),
            sizes: vec![size(5, 320, 240), size(6, 1280, 960), size(7, 2560, 1920)],
        };
        let content = enums::MessageContent::MessagePhoto(types::MessagePhoto {
            photo,
            caption: caption("Look"),
            show_caption_above_media: false,
            has_spoiler: false,
            is_secret: false,
        });
        let message = with_content(content.clone());
        assert_eq!(message.text, "Look");
        assert_eq!(
            message.media,
            Some(Media::Photo(Photo {
                file_id: 6,
                width: 1280,
                height: 960,
                minithumbnail: Some(vec![1, 2, 3]),
                has_spoiler: false
            }))
        );
        // The chat list keeps the label.
        assert_eq!(content_text(&content), "[Photo] Look");
    }

    #[test]
    fn photo_without_sizes_stays_a_label() {
        let content = enums::MessageContent::MessagePhoto(types::MessagePhoto {
            photo: types::Photo::default(),
            caption: caption(""),
            show_caption_above_media: false,
            has_spoiler: false,
            is_secret: false,
        });
        let message = with_content(content);
        assert_eq!((message.text.as_str(), message.media), ("[Photo]", None));
    }

    fn animation_content(thumbnail: Option<types::Thumbnail>, minithumbnail: &str) -> enums::MessageContent {
        animation_with(thumbnail, minithumbnail, false, false)
    }

    fn animation_with(
        thumbnail: Option<types::Thumbnail>,
        minithumbnail: &str,
        has_spoiler: bool,
        is_secret: bool,
    ) -> enums::MessageContent {
        let mut animation = file(9);
        animation.remote.unique_id = "AgADxyz".into();
        enums::MessageContent::MessageAnimation(types::MessageAnimation {
            animation: types::Animation {
                duration: 3,
                width: 480,
                height: 270,
                file_name: "cat.mp4".into(),
                mime_type: "video/mp4".into(),
                has_stickers: false,
                minithumbnail: Some(types::Minithumbnail { width: 40, height: 22, data: minithumbnail.into() }),
                thumbnail,
                animation,
            },
            caption: caption(""),
            show_caption_above_media: false,
            has_spoiler,
            is_secret,
        })
    }

    #[test]
    fn animation_message_carries_its_files() {
        let thumbnail =
            types::Thumbnail { format: enums::ThumbnailFormat::Jpeg, width: 320, height: 180, file: file(10) };
        let message = with_content(animation_content(Some(thumbnail), "AQID"));
        assert_eq!(message.text, "");
        assert_eq!(
            message.media,
            Some(Media::Animation(Animation {
                file_id: 9,
                unique_id: "AgADxyz".into(),
                width: 480,
                height: 270,
                mime_type: "video/mp4".into(),
                thumbnail: Some(Thumbnail { file_id: 10, format: ThumbnailFormat::Jpeg }),
                minithumbnail: Some(vec![1, 2, 3]),
                has_spoiler: false,
            }))
        );
        assert_eq!(content_text(&animation_content(None, "")), "[GIF]");
    }

    fn photo_content(has_spoiler: bool, is_secret: bool, caption_text: &str) -> enums::MessageContent {
        enums::MessageContent::MessagePhoto(types::MessagePhoto {
            photo: types::Photo { has_stickers: false, minithumbnail: None, sizes: vec![size(5, 320, 240)] },
            caption: caption(caption_text),
            show_caption_above_media: false,
            has_spoiler,
            is_secret,
        })
    }

    #[test]
    fn spoilers_and_protection_are_carried() {
        let message = with_content(photo_content(true, false, ""));
        assert!(message.media.as_ref().is_some_and(Media::has_spoiler));
        assert!(message.can_be_saved);
        let message = with_content(animation_with(None, "", true, false));
        assert!(message.media.as_ref().is_some_and(Media::has_spoiler));
        let protected = Msg::from(types::Message { can_be_saved: false, ..message_with_photo() });
        assert!(!protected.can_be_saved);
        assert!(!with_content(photo_content(false, false, "")).media.unwrap().has_spoiler());
    }

    fn message_with_photo() -> types::Message {
        types::Message { content: photo_content(false, false, ""), ..message(2, None) }
    }

    #[test]
    fn self_destructing_media_is_a_label_only() {
        let message = with_content(photo_content(false, true, "Quick"));
        assert_eq!(message.media, None);
        assert_eq!(message.text, "[Self-destructing photo, open it in an official app] Quick");
        let message = with_content(animation_with(None, "", false, true));
        assert_eq!(message.media, None);
        assert_eq!(message.text, "[Self-destructing GIF, open it in an official app]");
        // The chat list shows the same label.
        let preview = preview(&types::Message { content: photo_content(false, true, ""), ..message_with_photo() });
        assert_eq!(preview, "[Self-destructing photo, open it in an official app]");
        // An edit can't smuggle it in either.
        let event = Event::from_update(enums::Update::MessageContent(types::UpdateMessageContent {
            chat_id: CHAT,
            message_id: 2,
            new_content: photo_content(false, true, ""),
        }));
        assert!(matches!(event, Some(Event::MessageContent { media: None, .. })));
    }

    #[test]
    fn broken_minithumbnail_is_left_out() {
        let message = with_content(animation_content(None, "not base64!"));
        let Some(Media::Animation(animation)) = message.media else { panic!("no animation") };
        assert_eq!((animation.minithumbnail, animation.thumbnail), (None, None));
    }

    #[test]
    fn edits_carry_the_new_media() {
        let event = Event::from_update(enums::Update::MessageContent(types::UpdateMessageContent {
            chat_id: CHAT,
            message_id: 2,
            new_content: animation_content(None, ""),
        }));
        let Some(Event::MessageContent { chat_id: CHAT, message_id: 2, text, media }) = event else {
            panic!("not a content change: {event:?}")
        };
        assert_eq!(text, "");
        assert!(matches!(media, Some(Media::Animation(Animation { file_id: 9, .. }))));
    }

    #[test]
    fn uploads_are_classified_by_extension() {
        for photo in ["a.jpg", "b.JPEG", "/x/c.png", "d.webp"] {
            assert_eq!(Upload::for_path(Path::new(photo)), Some(Upload::Photo), "{photo}");
        }
        assert_eq!(Upload::for_path(Path::new("e.GIF")), Some(Upload::Animation));
        for other in ["f.mp4", "g.heic", "h", ".png", "i.png.txt", "/dir.png/j"] {
            assert_eq!(Upload::for_path(Path::new(other)), None, "{other}");
        }
    }
}
