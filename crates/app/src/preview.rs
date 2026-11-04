//! Sample chats for `--preview`, which shows the app's look without a
//! Telegram account: made-up people and messages, nothing sent or stored.

use std::collections::HashMap;

use frillogram_core::{ChatSummary, Msg, SendState, SenderId};

/// The user's own id in the samples.
pub(crate) const ME: i64 = 1;

const DESIGN_TEAM: i64 = -100;
const ADA: i64 = 2;
const GRACE: i64 = 3;
const LINUS: i64 = 5;
const MARGARET: i64 = 8;

/// The chat list, in order.
pub(crate) fn chats() -> Vec<ChatSummary> {
    let chats = [
        (DESIGN_TEAM, "Design Team", "You: Feedback welcome before Thursday.", 3),
        (ADA, "Ada Lovelace", "Sounds good, see you at 3", 0),
        (-101, "Weekend Trip", "Linus: I booked the cabin", 12),
        (GRACE, "Grace Hopper", "Found the bug. It was a moth.", 0),
        (-102, "Rust Hackers", "Anyone tried the new borrow checker?", 128),
        (6, "Mom", "Call me when you land", 1),
        (LINUS, "Linus Torvalds", "Patch looks fine", 0),
        (-103, "Book Club", "Next month: Piranesi", 0),
        (MARGARET, "Margaret Hamilton", "The docs are in the shared folder", 0),
        (10, "Alan Turing", "Thanks!", 0),
    ];
    chats
        .into_iter()
        .enumerate()
        .map(|(i, (id, title, last, unread))| ChatSummary {
            id,
            title: title.into(),
            last_message: Some(last.into()),
            unread,
            order: (chats.len() - i) as i64,
            is_channel: false,
        })
        .collect()
}

/// Names of the people who send the sample messages.
pub(crate) fn user_names() -> HashMap<i64, String> {
    [
        (ME, "You"),
        (ADA, "Ada Lovelace"),
        (GRACE, "Grace Hopper"),
        (LINUS, "Linus Torvalds"),
        (MARGARET, "Margaret Hamilton"),
    ]
    .into_iter()
    .map(|(id, name)| (id, name.to_string()))
    .collect()
}

/// The messages of sample chat `chat_id`, oldest first, dated relative to
/// `now` (a Unix timestamp) so they always read as recent.
pub(crate) fn messages(chat_id: i64, now: i32) -> Vec<Msg> {
    const MINUTE: i32 = 60;
    const DAY: i32 = 24 * 60 * MINUTE;
    let msg = |id: i64, sender: i64, ago: i32, text: &str, reply_to: Option<i64>| Msg {
        id,
        chat_id,
        sender: SenderId::User(sender),
        date: now - ago,
        text: text.into(),
        media: None,
        reply_to,
        outgoing: sender == ME,
        send_state: SendState::Sent,
        can_be_saved: true,
    };
    match chat_id {
        // Grouped messages, two days, and two threads, so every part of the
        // message list shows up.
        DESIGN_TEAM => vec![
            msg(
                1,
                ADA,
                DAY + 120 * MINUTE,
                "Morning! Did everyone get a chance to look at the new onboarding flow?",
                None,
            ),
            msg(2, ADA, DAY + 118 * MINUTE, "I left comments in the doc.", None),
            msg(
                3,
                GRACE,
                DAY + 100 * MINUTE,
                "Yes, the second screen feels crowded. Could we ask for permissions after signup instead?",
                Some(1),
            ),
            msg(4, LINUS, DAY + 98 * MINUTE, "Agreed, and the progress dots are hard to see in dark mode.", Some(1)),
            msg(5, ME, DAY + 96 * MINUTE, "Good catches. I'll mock up an alternative this afternoon.", Some(1)),
            msg(6, MARGARET, 90 * MINUTE, "Reminder: design review moves to Thursday this week.", None),
            msg(
                7,
                GRACE,
                60 * MINUTE,
                "Shipping the new icons today 🎉 They have a 2px stroke now, so they line up with the text at \
                 every size. Tell me if any look off on Linux, I only checked macOS.",
                None,
            ),
            msg(8, ME, 58 * MINUTE, "These look great. The chat bubble one especially.", Some(7)),
            msg(9, ADA, 56 * MINUTE, "Love the settings gear!", Some(7)),
            msg(10, ME, 10 * MINUTE, "Pushed the onboarding mock to the shared folder.", None),
            msg(11, ME, 9 * MINUTE, "Feedback welcome before Thursday.", None),
        ],
        ADA => vec![
            msg(1, ADA, 50 * MINUTE, "Are we still on for coffee?", None),
            msg(2, ME, 45 * MINUTE, "Yes! 3pm at the usual place?", None),
            msg(3, ADA, 44 * MINUTE, "Sounds good, see you at 3", None),
        ],
        // The other chats show their last message, so none is empty.
        _ => {
            let last = chats().into_iter().find(|chat| chat.id == chat_id).and_then(|chat| chat.last_message);
            let text = last.as_deref().unwrap_or("Hello!");
            let (sender, text) = match text.split_once(": ") {
                Some(("You", text)) => (ME, text),
                Some((_, text)) => (LINUS, text),
                None => (if chat_id < 0 { MARGARET } else { chat_id }, text),
            };
            vec![msg(1, sender, 30 * MINUTE, text, None)]
        }
    }
}

#[cfg(test)]
mod tests {
    use frillogram_core::{SenderId, threads};

    use super::{chats, messages, user_names};

    #[test]
    fn every_sample_chat_has_messages_whose_senders_have_names() {
        let names = user_names();
        for chat in chats() {
            let messages = messages(chat.id, 1_800_000_000);
            assert!(!messages.is_empty(), "{}", chat.title);
            assert!(messages.windows(2).all(|pair| pair[0].id < pair[1].id), "{}", chat.title);
            for message in &messages {
                let SenderId::User(id) = message.sender else { panic!("{}", chat.title) };
                // A private chat's sender is that chat's person.
                assert!(names.contains_key(&id) || id == chat.id, "{}: {id}", chat.title);
            }
        }
    }

    #[test]
    fn the_first_chat_shows_threads() {
        let first = chats()[0].id;
        let messages = messages(first, 1_800_000_000);
        let with_replies = threads::group(&messages).iter().filter(|t| !t.replies.is_empty()).count();
        assert_eq!(with_replies, 2);
    }
}
