//! App-level types. TDLib's generated types are huge and change with every
//! TDLib release, so the UI only ever sees these small, stable shapes.

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
    pub text: String,
    /// The message this one replies to, if it is in the same chat.
    pub reply_to: Option<i64>,
    pub outgoing: bool,
    pub send_state: SendState,
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
    /// A message's content changed, e.g. after an edit.
    MessageText {
        chat_id: i64,
        message_id: i64,
        text: String,
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
                Event::MessageText { chat_id: u.chat_id, message_id: u.message_id, text: content_text(&u.new_content) }
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
        Msg {
            id: message.id,
            chat_id: message.chat_id,
            sender: match &message.sender_id {
                enums::MessageSender::User(u) => SenderId::User(u.user_id),
                enums::MessageSender::Chat(c) => SenderId::Chat(c.chat_id),
            },
            date: message.date,
            text: content_text(&message.content),
            reply_to,
            outgoing: message.is_outgoing,
            send_state: match message.sending_state {
                None => SendState::Sent,
                Some(enums::MessageSendingState::Pending(_)) => SendState::Pending,
                Some(enums::MessageSendingState::Failed(_)) => SendState::Failed,
            },
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

/// Text for a message body. Media isn't rendered yet, so it becomes a label
/// followed by its caption.
fn content_text(content: &enums::MessageContent) -> String {
    use enums::MessageContent as C;
    let with_caption = |label: &str, caption: &types::FormattedText| {
        if caption.text.is_empty() { label.to_string() } else { format!("{label} {}", caption.text) }
    };
    match content {
        C::MessageText(m) => m.text.text.clone(),
        C::MessagePhoto(m) => with_caption("[Photo]", &m.caption),
        C::MessageVideo(m) => with_caption("[Video]", &m.caption),
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
}
