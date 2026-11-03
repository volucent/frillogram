use std::collections::HashMap;

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
    pub(crate) busy: bool,

    pub(crate) phone_input: Entity<InputState>,
    pub(crate) code_input: Entity<InputState>,
    pub(crate) password_input: Entity<InputState>,

    pub(crate) chats: HashMap<i64, ChatSummary>,
    pub(crate) user_names: HashMap<i64, String>,
    pub(crate) selected_chat: Option<i64>,
    /// Messages of the selected chat, oldest first.
    pub(crate) messages: Vec<Msg>,
    /// Message changes seen while the selected chat's history loads, replayed
    /// onto it once it arrives. `None` when no history is loading.
    changes_while_loading: Option<Vec<Event>>,
    pub(crate) threads: Vec<Thread>,
    /// Root message id of the thread shown in the side panel.
    pub(crate) open_thread: Option<i64>,
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
            busy: false,
            phone_input,
            code_input,
            password_input,
            chats: HashMap::new(),
            user_names: HashMap::new(),
            selected_chat: None,
            messages: Vec::new(),
            changes_while_loading: None,
            threads: Vec::new(),
            open_thread: None,
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
                }
            }
            Event::ChatOrder { chat_id, order } => {
                if let Some(chat) = self.chats.get_mut(&chat_id) {
                    chat.order = order;
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
                if let Event::MessageSendFailed { error, .. } = &event {
                    self.error = Some(error.clone());
                }
                if self.apply_message_change(&event) {
                    self.rethread(window, cx);
                }
                if let Some(changes) = &mut self.changes_while_loading {
                    changes.push(event);
                }
            }
            Event::User { id, name } => {
                self.user_names.insert(id, name);
            }
            Event::Error(error) => self.error = Some(error),
        }
        cx.notify();
    }

    pub(crate) fn submit_login(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(tg) = self.tg else { return };
        let Some(input) = self.login_input(&self.auth) else { return };
        let value = input.read(cx).value().trim().to_string();
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
                    app.error = Some(error);
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
                    app.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(crate) fn select_chat(&mut self, chat_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tg) = self.tg else { return };
        if self.selected_chat == Some(chat_id) {
            return;
        }
        let previous = self.selected_chat.replace(chat_id);
        self.messages.clear();
        self.changes_while_loading = Some(Vec::new());
        self.threads.clear();
        self.message_list.reset(0);
        self.open_thread = None;
        self.error = None;
        self.composer.update(cx, |input, cx| input.focus(window, cx));

        cx.spawn_in(window, async move |this, cx| {
            if let Some(previous) = previous {
                let _ = tg.close_chat(previous).await;
            }
            // Another click may have picked a different chat while this one
            // was closing the previous; opening it now would leave it open.
            let still_selected =
                |cx: &mut AsyncApp| this.read_with(cx, |app, _| app.selected_chat == Some(chat_id)).unwrap_or(false);
            if !still_selected(cx) {
                return;
            }
            let _ = tg.open_chat(chat_id).await;
            if !still_selected(cx) {
                let _ = tg.close_chat(chat_id).await;
                return;
            }
            let history = tg.history(chat_id, HISTORY_LIMIT).await;
            let _ = this.update_in(cx, |app, window, cx| {
                if app.selected_chat != Some(chat_id) {
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
                        app.error = Some(error);
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
                    app.error = Some(error);
                    // Give the text back unless something new was typed meanwhile.
                    if input.read(cx).value().is_empty() {
                        input.update(cx, |input, cx| input.set_value(text, window, cx));
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
                let Some(index) = self.messages.iter().position(|m| m.id == *old_id) else { return false };
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
        // Server ids grow over time; sorting keeps parents before replies,
        // which `threads::group` relies on.
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
        }
        // Keep the thread panel at its newest reply, unless the user scrolled up.
        let (offset, max) = (self.thread_scroll.offset(), self.thread_scroll.max_offset());
        if self.open_thread.is_some() && offset.y <= -max.y + px(1.) {
            self.thread_scroll.scroll_to_bottom();
        }
        // Only replace the rows that changed, so the list keeps its scroll
        // position when a message arrives. `reset` is for switching chats.
        let old = &self.threads;
        let prefix = old.iter().zip(&threads).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..].iter().rev().zip(threads[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
        self.message_list.splice(prefix..old.len() - suffix, threads.len() - prefix - suffix);
        self.threads = threads;
    }

    pub(crate) fn sender_name(&self, sender: SenderId) -> String {
        match sender {
            SenderId::User(id) => self.user_names.get(&id).cloned(),
            SenderId::Chat(id) => self.chats.get(&id).map(|c| c.title.clone()),
        }
        .unwrap_or_else(|| "Unknown".into())
    }
}

impl Render for FrillogramApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
