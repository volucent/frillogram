use std::collections::HashMap;

use frillogram_core::{
    AuthStep, ChatSummary, Config, Event, Msg, SenderId, Telegram,
    threads::{self, Thread},
};
use gpui_kit::{
    component::{
        ActiveTheme, Theme,
        input::{InputEvent, InputState},
    },
    *,
};

/// How many recent messages to load when a chat is opened.
const HISTORY_LIMIT: usize = 100;
/// How many chats to ask TDLib for after login.
const CHAT_LIMIT: i32 = 200;

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
    pub(crate) threads: Vec<Thread>,
    /// Root message id of the thread shown in the side panel.
    pub(crate) open_thread: Option<i64>,
    pub(crate) message_list: ListState,
    pub(crate) composer: Entity<InputState>,
    pub(crate) thread_composer: Entity<InputState>,

    _subscriptions: Vec<Subscription>,
    _events: Option<Task<()>>,
}

impl FrillogramApp {
    pub fn new(config: Result<Config, String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let phone_input = cx.new(|cx| InputState::new(window, cx).placeholder("+1 555 123 4567"));
        let code_input = cx.new(|cx| InputState::new(window, cx).placeholder("Login code"));
        let password_input = cx.new(|cx| InputState::new(window, cx).placeholder("Password").masked(true));
        let composer = cx.new(|cx| InputState::new(window, cx).placeholder("Message"));
        let thread_composer = cx.new(|cx| InputState::new(window, cx).placeholder("Reply in thread…"));

        Theme::sync_system_appearance(Some(window), cx);
        let mut subscriptions = vec![cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        })];
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
            threads: Vec::new(),
            open_thread: None,
            message_list: ListState::new(0, ListAlignment::Bottom, px(600.)),
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
                self.auth = step;
                self.busy = false;
                self.focus_login_input(window, cx);
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
                    if let Some(order) = order {
                        chat.order = order;
                    }
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
                    self.view_messages(message.chat_id, vec![message.id], cx);
                    self.messages.push(message);
                    self.rethread();
                }
            }
            Event::MessageSent { old_id, message } => {
                if let Some(pending) = self.messages.iter_mut().find(|m| m.id == old_id) {
                    *pending = message;
                    self.rethread();
                }
            }
            Event::MessageSendFailed { old_id, error } => {
                let count = self.messages.len();
                self.messages.retain(|m| m.id != old_id);
                if self.messages.len() != count {
                    self.rethread();
                }
                self.error = Some(error);
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
        let input = match self.auth {
            AuthStep::Phone => &self.phone_input,
            AuthStep::Code { .. } => &self.code_input,
            AuthStep::Password { .. } => &self.password_input,
            _ => return,
        };
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
            // "Not Found" only means every chat is already loaded.
            if let Err(error) = tg.load_chats(CHAT_LIMIT).await
                && error != "Not Found"
            {
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
        self.threads.clear();
        self.message_list.reset(0);
        self.open_thread = None;
        self.composer.update(cx, |input, cx| input.focus(window, cx));

        cx.spawn(async move |this, cx| {
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
            let _ = this.update(cx, |app, cx| {
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
                        app.rethread();
                        app.view_messages(chat_id, app.messages.iter().map(|m| m.id).collect(), cx);
                    }
                    Err(error) => app.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
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
            self.thread_composer.update(cx, |input, cx| input.focus(window, cx));
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

    fn rethread(&mut self) {
        // Server ids grow over time; sorting keeps parents before replies,
        // which `threads::group` relies on.
        self.messages.sort_by_key(|m| m.id);
        let threads = threads::group(&self.messages);
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
