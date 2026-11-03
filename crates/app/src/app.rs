use std::{collections::HashMap, ops::Range};

use frillogram_core::{
    AuthStep, ChatSummary, Config, Event, Msg, SenderId, Telegram,
    threads::{self, Thread},
};
use gpui_kit::{
    component::{
        ActiveTheme, Theme,
        input::{InputEvent, InputState, TextareaState},
    },
    *,
};

/// How many recent messages to load when a chat is opened.
const HISTORY_LIMIT: usize = 100;
/// How many chats to ask TDLib for per request after login.
const CHAT_LIMIT: i32 = 200;
/// The composers grow with their text up to this many lines, then scroll.
const COMPOSER_MAX_ROWS: usize = 6;

pub struct FrillogramApp {
    pub(crate) tg: Option<Telegram>,
    /// Set when the app can't start at all, e.g. API credentials are missing.
    pub(crate) setup_error: Option<String>,
    pub(crate) auth: AuthStep,
    pub(crate) error: Option<String>,
    /// The chat `error` is about, if any. Switching chats hides only such
    /// errors, so e.g. a failed chat list load stays visible.
    error_chat: Option<i64>,
    pub(crate) busy: bool,

    pub(crate) phone_input: Entity<InputState>,
    pub(crate) code_input: Entity<InputState>,
    pub(crate) password_input: Entity<InputState>,

    pub(crate) chats: HashMap<i64, ChatSummary>,
    /// Ids of the chats shown in the chat list, in display order. Kept here
    /// so the list isn't filtered and sorted on every frame.
    pub(crate) visible_chats: Vec<i64>,
    /// `visible_chats` is out of date. Chat updates arrive by the thousand
    /// after login, so it's rebuilt once before the next render, not per update.
    chats_dirty: bool,
    pub(crate) user_names: HashMap<i64, String>,
    pub(crate) selected_chat: Option<i64>,
    /// Unsent composer text of chats other than the selected one.
    drafts: HashMap<i64, String>,
    /// Bumped by each `select_chat`, so a slower, older load can tell it's
    /// been superseded and leave the newer one's state alone.
    load_generation: u64,
    /// The selected chat's history failed to load, so picking it again retries.
    load_failed: bool,
    /// Messages of the selected chat, oldest first.
    pub(crate) messages: Vec<Msg>,
    /// Message changes seen while the selected chat's history loads, replayed
    /// onto it once it arrives. `None` when no history is loading.
    changes_while_loading: Option<Vec<Event>>,
    pub(crate) threads: Vec<Thread>,
    /// One key per entry of `threads`, for diffing them by content.
    thread_keys: Vec<RowKey>,
    /// Root message id of the thread shown in the side panel.
    pub(crate) open_thread: Option<i64>,
    /// Server ids of the selected chat's messages that were pending, by their
    /// temporary id, so a failed thread reply can still find its thread.
    remapped_ids: HashMap<i64, i64>,
    pub(crate) message_list: ListState,
    pub(crate) thread_scroll: ScrollHandle,
    pub(crate) composer: Entity<TextareaState>,
    pub(crate) thread_composer: Entity<TextareaState>,

    _subscriptions: Vec<Subscription>,
    _events: Option<Task<()>>,
}

impl FrillogramApp {
    pub fn new(config: Result<Config, String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let phone_input = cx.new(|cx| InputState::new(window, cx).placeholder("+1 555 123 4567"));
        let code_input = cx.new(|cx| InputState::new(window, cx).placeholder("Login code"));
        let password_input = cx.new(|cx| InputState::new(window, cx).placeholder("Password").masked(true));
        // Enter sends, Shift+Enter starts a new line.
        let composer = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder(placeholder)
                    // Enter-to-send relies on this, which gpui-base 0.9 marks
                    // `#[doc(hidden)]`; recheck it when upgrading.
                    .submit_on_enter(true)
                    .auto_grow(1, COMPOSER_MAX_ROWS)
            })
        };
        let thread_composer = composer("Reply in thread…", window, cx);
        let composer = composer("Message", window, cx);

        Theme::sync_system_appearance(Some(window), cx);
        let mut subscriptions = vec![
            cx.observe_window_appearance(window, |_, window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
            }),
            // Messages that arrived while the window was in the background
            // are read once the user comes back to it.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.view_loaded_messages(cx);
                }
            }),
        ];
        for input in [&phone_input, &code_input, &password_input] {
            subscriptions.push(cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_login(window, cx);
                }
            }));
        }
        for (input, in_thread) in [(&composer, false), (&thread_composer, true)] {
            subscriptions.push(cx.subscribe_in(input, window, move |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                    this.send_from_composer(in_thread, window, cx);
                }
            }));
        }

        let (tg, setup_error, events) = match config {
            Ok(config) => {
                let (tg, events) = Telegram::start(config);
                (Some(tg), None, Some(events))
            }
            Err(error) => (None, Some(error), None),
        };
        let _events = events.map(|events| {
            cx.spawn_in(window, async move |this, cx| {
                while let Ok(event) = events.recv().await {
                    if this.update_in(cx, |app, window, cx| app.handle_event(event, window, cx)).is_err() {
                        break;
                    }
                }
            })
        });

        FrillogramApp {
            tg,
            setup_error,
            auth: AuthStep::Starting,
            error: None,
            error_chat: None,
            busy: false,
            phone_input,
            code_input,
            password_input,
            chats: HashMap::new(),
            visible_chats: Vec::new(),
            chats_dirty: false,
            user_names: HashMap::new(),
            selected_chat: None,
            drafts: HashMap::new(),
            load_generation: 0,
            load_failed: false,
            messages: Vec::new(),
            changes_while_loading: None,
            threads: Vec::new(),
            thread_keys: Vec::new(),
            open_thread: None,
            remapped_ids: HashMap::new(),
            message_list: ListState::new(0, ListAlignment::Bottom, px(600.)),
            thread_scroll: ScrollHandle::new(),
            composer,
            thread_composer,
            _subscriptions: subscriptions,
            _events,
        }
    }

    fn handle_event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Auth(step) => {
                if step == AuthStep::Ready && self.auth != AuthStep::Ready {
                    self.load_chats(cx);
                }
                let previous = std::mem::replace(&mut self.auth, step);
                // Clear an answered step's input, so the login code and
                // password don't stay in widget state.
                if std::mem::discriminant(&previous) != std::mem::discriminant(&self.auth)
                    && let Some(input) = self.login_input(&previous).cloned()
                {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
                self.busy = false;
                if self.auth == AuthStep::Ready {
                    // The login input is gone, and no chat is open yet to
                    // type into; picking one focuses the composer.
                    if std::mem::discriminant(&previous) != std::mem::discriminant(&self.auth) {
                        window.blur(cx);
                    }
                } else {
                    self.focus_login_input(window, cx);
                }
            }
            Event::Chat(chat) => {
                self.chats.insert(chat.id, chat);
                self.chats_dirty = true;
            }
            Event::ChatTitle { chat_id, title } => {
                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    chat.title = title;
                }
            }
            Event::ChatLastMessage { chat_id, preview, order } => {
                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    chat.last_message = preview;
                    chat.order = order;
                    self.chats_dirty = true;
                }
            }
            Event::ChatOrder { chat_id, order } => {
                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    chat.order = order;
                    self.chats_dirty = true;
                }
            }
            Event::ChatUnread { chat_id, unread } => {
                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    chat.unread = unread;
                }
            }
            Event::NewMessage(message) => {
                if Some(message.chat_id) == self.selected_chat && !self.messages.iter().any(|m| m.id == message.id) {
                    // Only mark it read if the user can see it; otherwise
                    // that happens when the window is activated.
                    if window.is_window_active() {
                        self.view_messages(message.chat_id, vec![message.id], cx);
                    }
                    self.messages.push(message);
                    self.rethread(window, cx);
                }
            }
            Event::MessageSent { .. }
            | Event::MessageSendFailed { .. }
            | Event::MessagesDeleted { .. }
            | Event::MessageText { .. } => {
                let chat_id = match &event {
                    Event::MessageSent { message, .. } | Event::MessageSendFailed { message, .. } => {
                        Some(message.chat_id)
                    }
                    Event::MessagesDeleted { chat_id, .. } | Event::MessageText { chat_id, .. } => Some(*chat_id),
                    _ => None,
                };
                let selected = chat_id.is_some() && chat_id == self.selected_chat;
                if let Event::MessageSendFailed { error, .. } = &event
                    && selected
                {
                    self.show_error(error.clone(), chat_id);
                }
                if self.apply_message_change(&event) {
                    self.rethread(window, cx);
                }
                if let Some(changes) = &mut self.changes_while_loading
                    && selected
                {
                    changes.push(event);
                }
            }
            Event::User { id, name } => {
                self.user_names.insert(id, name);
            }
            Event::Error(error) => self.show_error(error, None),
        }
        cx.notify();
    }

    /// Shows `error`, which is about `chat` if that's set.
    fn show_error(&mut self, error: String, chat: Option<i64>) {
        self.error = Some(error);
        self.error_chat = chat;
    }

    pub(crate) fn submit_login(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(tg) = self.tg else { return };
        let Some(input) = self.login_input(&self.auth) else { return };
        let value = input.read(cx).value();
        // Spaces can be part of a password, so only the phone and code are trimmed.
        let value = match self.auth {
            AuthStep::Password { .. } => value.to_string(),
            _ => value.trim().to_string(),
        };
        if value.is_empty() || self.busy {
            return;
        }
        let step = self.auth.clone();
        self.busy = true;
        self.error = None;
        cx.spawn(async move |this, cx| {
            let result = match step {
                AuthStep::Phone => tg.submit_phone(value).await,
                AuthStep::Code { .. } => tg.submit_code(value).await,
                _ => tg.submit_password(value).await,
            };
            // On success TDLib moves to the next step with an Auth event.
            if let Err(error) = result {
                let _ = this.update(cx, |app, cx| {
                    app.show_error(error, None);
                    app.busy = false;
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn load_chats(&mut self, cx: &mut Context<Self>) {
        let Some(tg) = self.tg else { return };
        cx.spawn(async move |this, cx| {
            if let Err(error) = tg.load_chats(CHAT_LIMIT).await {
                let _ = this.update(cx, |app, cx| {
                    app.show_error(error, None);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(crate) fn select_chat(&mut self, chat_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tg) = self.tg else { return };
        if self.selected_chat == Some(chat_id) && !self.load_failed {
            return;
        }
        let previous = self.selected_chat.replace(chat_id);
        // Each chat keeps its own draft, so Enter can't send one chat's
        // half-typed message to another.
        if let Some(previous) = previous {
            let draft = self.composer.read(cx).value().to_string();
            if !draft.is_empty() {
                self.drafts.insert(previous, draft);
            }
        }
        let draft = self.drafts.remove(&chat_id).unwrap_or_default();
        restore_text(&self.composer, draft, window, cx);
        self.load_generation += 1;
        let generation = self.load_generation;
        self.load_failed = false;
        self.messages.clear();
        self.changes_while_loading = Some(Vec::new());
        self.threads.clear();
        self.thread_keys.clear();
        self.message_list.reset(0);
        self.open_thread = None;
        self.remapped_ids.clear();
        self.thread_composer.update(cx, |input, cx| input.set_value("", window, cx));
        if self.error_chat.take().is_some() {
            self.error = None;
        }
        self.composer.update(cx, |input, cx| input.focus(window, cx));

        cx.spawn_in(window, async move |this, cx| {
            if let Some(previous) = previous {
                let _ = tg.close_chat(previous).await;
            }
            // Another click may have started a newer load meanwhile; that one
            // owns the chat state now, even if it's for this same chat.
            let is_current =
                |cx: &mut AsyncApp| this.read_with(cx, |app, _| app.load_generation == generation).unwrap_or(false);
            if !is_current(cx) {
                return;
            }
            let _ = tg.open_chat(chat_id).await;
            if !is_current(cx) {
                // Opening it now would leave it open, unless the newer load
                // picked this chat again and needs it open.
                let reselected = this.read_with(cx, |app, _| app.selected_chat == Some(chat_id)).unwrap_or(false);
                if !reselected {
                    let _ = tg.close_chat(chat_id).await;
                }
                return;
            }
            let history = tg.history(chat_id, HISTORY_LIMIT).await;
            let _ = this.update_in(cx, |app, window, cx| {
                if app.load_generation != generation {
                    return;
                }
                match history {
                    Ok(mut history) => {
                        // Keep messages that arrived while history was loading.
                        let arrived: Vec<Msg> =
                            app.messages.drain(..).filter(|m| !history.iter().any(|h| h.id == m.id)).collect();
                        history.extend(arrived);
                        app.messages = history;
                        // Changes that came in after the fetch may not be in it.
                        for change in app.changes_while_loading.take().unwrap_or_default() {
                            app.apply_message_change(&change);
                        }
                        app.rethread(window, cx);
                        if window.is_window_active() {
                            app.view_loaded_messages(cx);
                        }
                    }
                    Err(error) => {
                        app.changes_while_loading = None;
                        app.load_failed = true;
                        app.show_error(error, Some(chat_id));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Marks all loaded messages of the selected chat read on Telegram.
    fn view_loaded_messages(&self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.selected_chat else { return };
        if !self.messages.is_empty() {
            self.view_messages(chat_id, self.messages.iter().map(|m| m.id).collect(), cx);
        }
    }

    /// Marks messages of the open chat read on Telegram.
    fn view_messages(&self, chat_id: i64, message_ids: Vec<i64>, cx: &mut Context<Self>) {
        let Some(tg) = self.tg else { return };
        cx.background_spawn(async move {
            let _ = tg.view_messages(chat_id, message_ids).await;
        })
        .detach();
    }

    pub(crate) fn open_thread(&mut self, root_id: Option<i64>, window: &mut Window, cx: &mut Context<Self>) {
        // A draft for one thread mustn't be sent to another by Enter.
        if self.open_thread != root_id {
            self.thread_composer.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.open_thread = root_id;
        if root_id.is_some() {
            self.thread_scroll.scroll_to_bottom();
            self.thread_composer.update(cx, |input, cx| input.focus(window, cx));
        } else {
            // The thread composer is no longer rendered, so it can't keep focus.
            self.composer.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn send_from_composer(&mut self, in_thread: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(tg), Some(chat_id)) = (self.tg, self.selected_chat) else { return };
        let input = if in_thread { self.thread_composer.clone() } else { self.composer.clone() };
        let text = input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        // A thread reply is a plain Telegram reply to the thread's root.
        let reply_to = if in_thread { self.open_thread } else { None };
        input.update(cx, |input, cx| input.set_value("", window, cx));

        cx.spawn_in(window, async move |this, cx| {
            if let Err(error) = tg.send_text(chat_id, text.clone(), reply_to).await {
                let _ = this.update_in(cx, |app, window, cx| {
                    let selected = app.selected_chat == Some(chat_id);
                    let thread_open = thread_still_open(app.open_thread, reply_to, &app.remapped_ids);
                    match restore_target(selected, in_thread, thread_open) {
                        // Only if nothing new was typed there meanwhile.
                        Restore::Composer if input.read(cx).value().is_empty() => {
                            restore_text(&input, text, window, cx);
                        }
                        Restore::Composer | Restore::Drop => {}
                        Restore::Draft => {
                            app.drafts.entry(chat_id).or_insert(text);
                        }
                    }
                    if selected {
                        app.show_error(error, Some(chat_id));
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Applies a sent, failed, deleted or edited message to the selected
    /// chat's messages, and returns whether they changed. Applying the same
    /// change twice is harmless, which replaying after a history load relies on.
    fn apply_message_change(&mut self, event: &Event) -> bool {
        let selected = |chat_id: i64| self.selected_chat == Some(chat_id);
        match event {
            Event::MessageSent { old_id, message } | Event::MessageSendFailed { old_id, message, .. }
                if selected(message.chat_id) =>
            {
                // The thread panel and replies may still point at the
                // temporary id of a message that was pending.
                let mut changed = false;
                self.remapped_ids.insert(*old_id, message.id);
                if self.open_thread == Some(*old_id) {
                    self.open_thread = Some(message.id);
                    changed = true;
                }
                for reply in self.messages.iter_mut().filter(|m| m.reply_to == Some(*old_id)) {
                    reply.reply_to = Some(message.id);
                    changed = true;
                }
                let Some(index) = self.messages.iter().position(|m| m.id == *old_id) else { return changed };
                if self.messages.iter().any(|m| m.id == message.id) {
                    self.messages.remove(index);
                } else {
                    self.messages[index] = message.clone();
                }
                true
            }
            Event::MessagesDeleted { chat_id, message_ids } if selected(*chat_id) => {
                let count = self.messages.len();
                self.messages.retain(|m| !message_ids.contains(&m.id));
                self.messages.len() != count
            }
            Event::MessageText { chat_id, message_id, text } if selected(*chat_id) => {
                match self.messages.iter_mut().find(|m| m.id == *message_id) {
                    Some(message) if message.text != *text => {
                        message.text = text.clone();
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn rethread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Server ids grow over time, so this shows messages oldest first.
        self.messages.sort_by_key(|m| m.id);
        let threads = threads::group(&self.messages);
        // Close the thread panel if its root is gone, so replies can't go to
        // a deleted message.
        if let Some(root_id) = self.open_thread
            && !self.messages.iter().any(|m| m.id == root_id)
        {
            self.open_thread = None;
            if self.thread_composer.read(cx).focus_handle(cx).is_focused(window) {
                self.composer.update(cx, |input, cx| input.focus(window, cx));
            }
            self.thread_composer.update(cx, |input, cx| input.set_value("", window, cx));
        }
        // Keep the thread panel at its newest reply, unless the user scrolled up.
        let (offset, max) = (self.thread_scroll.offset(), self.thread_scroll.max_offset());
        if self.open_thread.is_some() && offset.y <= -max.y + px(1.) {
            self.thread_scroll.scroll_to_bottom();
        }
        // Only replace the rows that changed, so the list keeps its scroll
        // position when a message arrives. `reset` is for switching chats.
        let keys = row_keys(&threads, &self.messages);
        let (old_rows, new_count) = changed_rows(&self.thread_keys, &keys);
        self.message_list.splice(old_rows, new_count);
        self.threads = threads;
        self.thread_keys = keys;
    }

    pub(crate) fn sender_name(&self, sender: SenderId) -> String {
        match sender {
            SenderId::User(id) => self.user_names.get(&id).cloned(),
            SenderId::Chat(id) => self.chats.get(&id).map(|c| c.title.clone()),
        }
        .unwrap_or_else(|| "Unknown".into())
    }
}

/// Puts `text` into a composer with the caret at its end, where typing
/// continues. `set_value` leaves a multi-line input's caret at the start.
fn restore_text(input: &Entity<TextareaState>, text: String, window: &mut Window, cx: &mut App) {
    input.update(cx, |input, cx| {
        input.set_value(text, window, cx);
        let end = input.text().len();
        // Not `set_cursor_position`, which also focuses the input.
        input.set_selected_range(end..end, cx);
    });
}

/// Where the text of a failed send goes back to.
#[derive(Debug, PartialEq)]
enum Restore {
    /// The composer it was sent from.
    Composer,
    /// Its chat's draft, for when that chat is no longer selected.
    Draft,
    Drop,
}

fn restore_target(chat_selected: bool, in_thread: bool, thread_open: bool) -> Restore {
    if !chat_selected {
        // No error can show there, so the text is kept for when the user
        // comes back, even a thread reply's: nothing sends it unseen.
        Restore::Draft
    } else if !in_thread || thread_open {
        Restore::Composer
    } else {
        // Its thread is closed. In the main composer Enter would post it to
        // the whole chat, so it's dropped and the error says it wasn't sent.
        Restore::Drop
    }
}

/// Whether the thread panel still shows the thread rooted at `root`, which
/// may have been a pending message that got its server id since.
fn thread_still_open(open_thread: Option<i64>, root: Option<i64>, remapped_ids: &HashMap<i64, i64>) -> bool {
    match (open_thread, root) {
        (Some(open), Some(root)) => open == root || remapped_ids.get(&root) == Some(&open),
        _ => false,
    }
}

/// Identifies a message list row by content: its root id, reply ids and
/// whether the root is a reply. `Thread` holds indices into `messages`, which
/// shift whenever a message above is added or removed.
type RowKey = (i64, Vec<i64>, bool);

fn row_keys(threads: &[Thread], messages: &[Msg]) -> Vec<RowKey> {
    threads
        .iter()
        .map(|t| (messages[t.root].id, t.replies.iter().map(|&i| messages[i].id).collect(), t.root_is_reply))
        .collect()
}

/// The range of `old` rows that differ from `new`, and how many `new` rows
/// replace them: everything between their common prefix and common suffix.
fn changed_rows<T: PartialEq>(old: &[T], new: &[T]) -> (Range<usize>, usize) {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
    (prefix..old.len() - suffix, new.len() - prefix - suffix)
}

/// Ids of the chats to list, in Telegram's order.
fn visible_chats(chats: &HashMap<i64, ChatSummary>) -> Vec<i64> {
    // Channels stay hidden until sponsored messages are shown, which
    // Telegram's API terms require for any client that displays channels.
    let mut visible: Vec<&ChatSummary> = chats.values().filter(|c| c.order > 0 && !c.is_channel).collect();
    visible.sort_by_key(|c| std::cmp::Reverse((c.order, c.id)));
    visible.into_iter().map(|c| c.id).collect()
}

impl Render for FrillogramApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.chats_dirty) {
            self.visible_chats = visible_chats(&self.chats);
        }
        let body = if let Some(error) = &self.setup_error {
            self.render_setup(error.clone(), cx).into_any_element()
        } else if self.auth == AuthStep::Ready {
            self.render_main(window, cx).into_any_element()
        } else {
            self.render_login(cx).into_any_element()
        };
        div().size_full().bg(cx.theme().background).text_color(cx.theme().foreground).child(body)
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's glob import brings its own `test` attribute.
    use std::collections::HashMap;

    use frillogram_core::{ChatSummary, Msg, SendState, SenderId, threads};

    use super::{Restore, changed_rows, restore_target, row_keys, thread_still_open, visible_chats};

    #[test]
    fn identical_rows_change_nothing() {
        assert_eq!(changed_rows(&[1, 2, 3], &[1, 2, 3]), (3..3, 0));
    }

    #[test]
    fn appended_row_is_inserted_at_the_end() {
        assert_eq!(changed_rows(&[1, 2], &[1, 2, 3]), (2..2, 1));
    }

    #[test]
    fn deleted_row_above_keeps_the_rest() {
        assert_eq!(changed_rows(&[1, 2, 3, 4], &[1, 3, 4]), (1..2, 0));
        assert_eq!(changed_rows(&[1, 2, 3], &[2, 3]), (0..1, 0));
    }

    #[test]
    fn repeated_rows_never_underflow() {
        assert_eq!(changed_rows(&[1, 1], &[1, 1, 1]), (2..2, 1));
        assert_eq!(changed_rows(&[1, 1, 1], &[1]), (1..3, 0));
        assert_eq!(changed_rows::<i32>(&[], &[]), (0..0, 0));
    }

    #[test]
    fn row_keys_survive_index_shifts() {
        let msg = |id: i64, reply_to: Option<i64>| Msg {
            id,
            chat_id: 1,
            sender: SenderId::User(1),
            date: 0,
            text: String::new(),
            reply_to,
            outgoing: false,
            send_state: SendState::Sent,
        };
        let before = [msg(1, None), msg(2, None), msg(3, Some(2)), msg(4, None)];
        let after = &before[1..];
        let old = row_keys(&threads::group(&before), &before);
        let new = row_keys(&threads::group(after), after);
        // Deleting message 1 removes only its row, although every index moved.
        assert_eq!(changed_rows(&old, &new), (0..1, 0));
        assert_eq!(new, vec![(2, vec![3], false), (4, vec![], false)]);
    }

    #[test]
    fn visible_chats_skip_channels_and_unlisted_and_sort_by_order() {
        let chat = |id: i64, order: i64, is_channel: bool| {
            (id, ChatSummary { id, title: String::new(), last_message: None, unread: 0, order, is_channel })
        };
        let chats = HashMap::from([chat(1, 5, false), chat(2, 9, false), chat(3, 7, true), chat(4, 0, false)]);
        assert_eq!(visible_chats(&chats), vec![2, 1]);
    }

    #[test]
    fn failed_send_text_goes_back_where_it_can_be_sent_again() {
        assert_eq!(restore_target(true, false, false), Restore::Composer);
        assert_eq!(restore_target(true, true, true), Restore::Composer);
        assert_eq!(restore_target(true, true, false), Restore::Drop);
        assert_eq!(restore_target(false, false, false), Restore::Draft);
        assert_eq!(restore_target(false, true, true), Restore::Draft);
    }

    #[test]
    fn thread_stays_open_across_its_root_getting_a_server_id() {
        let remapped = HashMap::from([(-5, 42)]);
        assert!(thread_still_open(Some(7), Some(7), &remapped));
        assert!(thread_still_open(Some(42), Some(-5), &remapped));
        assert!(!thread_still_open(Some(8), Some(7), &remapped));
        assert!(!thread_still_open(Some(42), Some(-6), &remapped));
        assert!(!thread_still_open(None, Some(7), &remapped));
        assert!(!thread_still_open(None, None, &remapped));
    }
}
