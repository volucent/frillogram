use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
};

use frillogram_core::{
    AuthStep, ChatSummary, Config, ConfigError, Event, Msg, SendState, SenderId, Settings, Telegram,
    config::ThemeChoice,
    threads::{self, Thread},
};
use gpui_kit::{
    component::{
        ActiveTheme,
        input::{InputEvent, InputState, TextareaState},
    },
    *,
};

use crate::{
    chat::{RowFacts, RowLook, row_looks},
    media::MediaState,
    theme,
};

/// How many chats to ask TDLib for per request after login.
const CHAT_LIMIT: i32 = 200;
/// The composers grow with their text up to this many lines, then scroll.
const COMPOSER_MAX_ROWS: usize = 6;

pub struct FrillogramApp {
    /// What `config.toml` set, or the defaults if it has an error.
    pub(crate) settings: Settings,
    pub(crate) tg: Option<Telegram>,
    /// Set when the app can't start at all, e.g. API credentials are missing
    /// or the config file has an error.
    pub(crate) setup_error: Option<ConfigError>,
    pub(crate) auth: AuthStep,
    pub(crate) error: ErrorBanner,
    pub(crate) busy: bool,

    pub(crate) phone_input: Entity<InputState>,
    pub(crate) code_input: Entity<InputState>,
    pub(crate) password_input: Entity<InputState>,
    /// Filters the chat list by title.
    pub(crate) chat_search: Entity<InputState>,

    pub(crate) chats: HashMap<i64, ChatSummary>,
    /// Ids of the chats shown in the chat list, in display order. Kept here
    /// so the list isn't filtered and sorted on every frame.
    pub(crate) visible_chats: Vec<i64>,
    /// `visible_chats` is out of date. Chat updates arrive by the thousand
    /// after login, so it's rebuilt once before the next render, not per update.
    chats_dirty: bool,
    /// What the chat search holds, lowercased and trimmed.
    chat_query: String,
    pub(crate) user_names: HashMap<i64, String>,
    pub(crate) selected_chat: Option<i64>,
    /// Unsent composer text of chats other than the selected one.
    pub(crate) drafts: HashMap<i64, String>,
    /// Errors about sends from chats that were no longer selected when they
    /// failed, shown when the user comes back to them.
    pending_errors: HashMap<i64, Vec<String>>,
    /// Bumped by each `select_chat`, so a slower, older load can tell it's
    /// been superseded and leave the newer one's state alone.
    load_generation: u64,
    /// The chat this client holds open on TDLib, which counts `openChat` per
    /// chat and sends its updates until as many `closeChat`s came in. Set
    /// once the current load's `open_chat` succeeds and closed by the next
    /// `select_chat`; a superseded load closes its own instead. So every
    /// successful `open_chat` is closed exactly once.
    opened_chat: Option<i64>,
    /// The selected chat's history failed to load, so picking it again retries.
    load_failed: bool,
    /// Messages of the selected chat, oldest first.
    pub(crate) messages: Vec<Msg>,
    /// Message changes seen while the selected chat's history loads, replayed
    /// onto it once it arrives. `None` when no history is loading.
    changes_while_loading: Option<Vec<Event>>,
    pub(crate) threads: Vec<Thread>,
    /// How each entry of `threads` groups with the one above it.
    pub(crate) row_looks: Vec<RowLook>,
    /// One key per entry of `threads`, for diffing them by content.
    thread_keys: Vec<RowKey>,
    /// Root message id of the thread shown in the side panel.
    pub(crate) open_thread: Option<i64>,
    pub(crate) message_list: ListState,
    pub(crate) thread_scroll: ScrollHandle,
    pub(crate) composer: Entity<TextareaState>,
    pub(crate) thread_composer: Entity<TextareaState>,
    /// The thread composer had focus last, not the main one, so dismissing
    /// an error gives focus back to it.
    pub(crate) thread_composer_focused_last: bool,
    /// Downloads and GIF conversions of inline media.
    pub(crate) media: MediaState,
    /// A header was pressed on macOS, so the next drag moves the window.
    pub(crate) moving_window: bool,

    _subscriptions: Vec<Subscription>,
    _events: Option<Task<()>>,
}

impl FrillogramApp {
    pub fn new(
        settings: Settings,
        config: Result<Config, ConfigError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let phone_input = cx.new(|cx| InputState::new(window, cx).placeholder("+1 555 123 4567"));
        let code_input = cx.new(|cx| InputState::new(window, cx).placeholder("Login code"));
        let password_input = cx.new(|cx| InputState::new(window, cx).placeholder("Password").masked(true));
        let chat_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search chats"));
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

        theme::init(&settings.appearance, &settings.colors, cx);
        theme::apply(&settings.appearance, window, cx);
        let mut subscriptions = vec![
            cx.observe_window_appearance(window, |this, window, cx| {
                if this.settings.appearance.theme == ThemeChoice::System {
                    theme::apply(&this.settings.appearance, window, cx);
                }
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
        subscriptions.push(cx.subscribe_in(&chat_search, window, |this, input, event: &InputEvent, window, cx| {
            match event {
                InputEvent::Change => {
                    this.chat_query = input.read(cx).value().trim().to_lowercase();
                    this.chats_dirty = true;
                    cx.notify();
                }
                // Enter opens the first match, so a chat is a few keys away.
                InputEvent::PressEnter { .. } => {
                    if let Some(&chat_id) = this.visible_chats.first() {
                        this.select_chat(chat_id, window, cx);
                    }
                }
                _ => {}
            }
        }));
        for (input, in_thread) in [(&composer, false), (&thread_composer, true)] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { shift: false, .. } => this.send_from_composer(in_thread, window, cx),
                    InputEvent::Focus => this.thread_composer_focused_last = in_thread,
                    // The send button lights up once there's text.
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ));
        }

        let media = MediaState::new(config.as_ref().ok().map(|config| config.data_dir.clone()));
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
            settings,
            tg,
            setup_error,
            auth: AuthStep::Starting,
            error: ErrorBanner::default(),
            busy: false,
            phone_input,
            code_input,
            password_input,
            chat_search,
            chats: HashMap::new(),
            visible_chats: Vec::new(),
            chats_dirty: false,
            chat_query: String::new(),
            user_names: HashMap::new(),
            selected_chat: None,
            drafts: HashMap::new(),
            pending_errors: HashMap::new(),
            load_generation: 0,
            opened_chat: None,
            load_failed: false,
            messages: Vec::new(),
            changes_while_loading: None,
            threads: Vec::new(),
            row_looks: Vec::new(),
            thread_keys: Vec::new(),
            open_thread: None,
            message_list: ListState::new(0, ListAlignment::Bottom, px(600.)),
            thread_scroll: ScrollHandle::new(),
            composer,
            thread_composer,
            thread_composer_focused_last: false,
            media,
            moving_window: false,
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
                if self.selected_chat == Some(chat_id) {
                    self.name_composer(window, cx);
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
            | Event::MessageContent { .. } => {
                let chat_id = match &event {
                    Event::MessageSent { message, .. } | Event::MessageSendFailed { message, .. } => {
                        Some(message.chat_id)
                    }
                    Event::MessagesDeleted { chat_id, .. } | Event::MessageContent { chat_id, .. } => Some(*chat_id),
                    _ => None,
                };
                let selected = chat_id.is_some() && chat_id == self.selected_chat;
                // `send_text` returns once TDLib queued the message, so most
                // failures arrive here, often after the user moved on.
                if let Event::MessageSendFailed { error, message, .. } = &event {
                    if selected {
                        self.show_error(error.clone(), chat_id);
                    } else {
                        push_distinct(self.pending_errors.entry(message.chat_id).or_default(), error.clone());
                    }
                }
                // A pasted image's file has done its job once its message settled.
                if let Event::MessageSent { old_id, message } | Event::MessageSendFailed { old_id, message, .. } =
                    &event
                {
                    self.paste_settled(message.chat_id, *old_id, cx);
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

    /// Shows `error`, which is about `chat` if that's set, after any errors
    /// shown already.
    pub(crate) fn show_error(&mut self, error: String, chat: Option<i64>) {
        self.error.push(error, chat);
    }

    /// Shows an error about `chat_id`, or keeps it for when the user comes
    /// back to that chat.
    pub(crate) fn chat_error(&mut self, chat_id: i64, error: String) {
        if self.selected_chat == Some(chat_id) {
            self.show_error(error, Some(chat_id));
        } else {
            push_distinct(self.pending_errors.entry(chat_id).or_default(), error);
        }
    }

    pub(crate) fn submit_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.error.clear();
        cx.spawn_in(window, async move |this, cx| {
            let result = match step {
                AuthStep::Phone => tg.submit_phone(value).await,
                AuthStep::Code { .. } => tg.submit_code(value).await,
                _ => tg.submit_password(value).await,
            };
            // On success TDLib moves to the next step with an Auth event.
            if let Err(error) = result {
                let _ = this.update_in(cx, |app, window, cx| {
                    app.show_error(error, None);
                    app.busy = false;
                    // Clicking Next moved focus to it; the user retypes in the input.
                    app.focus_login_input(window, cx);
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
        // half-typed message to another. A retry of a failed load stays in
        // the same chat, so it leaves the composer, caret included, alone.
        if previous != Some(chat_id) {
            if let Some(previous) = previous {
                let draft = self.composer.read(cx).value().to_string();
                if !draft.is_empty() {
                    self.drafts.insert(previous, draft);
                }
            }
            let draft = self.drafts.remove(&chat_id).unwrap_or_default();
            restore_text(&self.composer, draft, window, cx);
            self.name_composer(window, cx);
        }
        self.load_generation += 1;
        let generation = self.load_generation;
        self.load_failed = false;
        self.messages.clear();
        self.changes_while_loading = Some(Vec::new());
        self.threads.clear();
        self.row_looks.clear();
        self.thread_keys.clear();
        self.message_list.reset(0);
        self.open_thread = None;
        self.thread_composer.update(cx, |input, cx| input.set_value("", window, cx));
        self.error.clear_chat();
        for error in self.pending_errors.remove(&chat_id).unwrap_or_default() {
            self.show_error(error, Some(chat_id));
        }
        self.composer.update(cx, |input, cx| input.focus(window, cx));
        // Only a chat that was actually opened is closed, so a load that
        // never got to open its chat can't close one opened elsewhere.
        let to_close = self.opened_chat.take();
        let history_limit = self.settings.chats.history_limit as usize;

        cx.spawn_in(window, async move |this, cx| {
            if let Some(to_close) = to_close {
                let _ = tg.close_chat(to_close).await;
            }
            // Another click may have started a newer load meanwhile; that one
            // owns the chat state now, even if it's for this same chat.
            let is_current =
                |cx: &mut AsyncApp| this.read_with(cx, |app, _| app.load_generation == generation).unwrap_or(false);
            if !is_current(cx) {
                return;
            }
            // TDLib doesn't count a failed open, so there's nothing to close then.
            let opened = tg.open_chat(chat_id).await.is_ok();
            let (current, to_close) = this
                .update(cx, |app, _| {
                    let current = app.load_generation == generation;
                    let to_close = opened.then(|| settle_open(&mut app.opened_chat, chat_id, current)).flatten();
                    (current, to_close)
                })
                .unwrap_or((false, opened.then_some(chat_id)));
            if let Some(to_close) = to_close {
                let _ = tg.close_chat(to_close).await;
            }
            if !current {
                return;
            }
            let history = tg.history(chat_id, history_limit).await;
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
        if self.open_thread.is_some() != root_id.is_some() {
            self.panel_toggled();
        }
        self.open_thread = root_id;
        if root_id.is_some() {
            self.thread_scroll.scroll_to_bottom();
        }
        if self.thread_root_state() == Some(SendState::Sent) {
            self.thread_composer.update(cx, |input, cx| input.focus(window, cx));
        } else {
            // The thread composer isn't rendered, so it can't take focus.
            self.composer.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    /// The thread panel opens or closes, which changes the main list's width.
    /// The list then measures every row again; this makes it keep the row at
    /// its top at the same fraction of its height, which its new height may
    /// not leave room for in pixels. `open_thread_from_row` first makes the
    /// row that opens the panel the top one, at fraction 0.
    fn panel_toggled(&self) {
        self.message_list.remeasure();
    }

    /// Shows the selected chat's name in the empty composer.
    fn name_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.selected_chat.and_then(|id| self.chats.get(&id)) else { return };
        let placeholder = format!("Message {}", chat.title);
        self.composer.update(cx, |input, cx| input.set_placeholder(placeholder, window, cx));
    }

    /// The send state of the open thread's root, if it's loaded.
    pub(crate) fn thread_root_state(&self) -> Option<SendState> {
        let root_id = self.open_thread?;
        self.messages.iter().find(|m| m.id == root_id).map(|m| m.send_state)
    }

    pub(crate) fn send_from_composer(&mut self, in_thread: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(tg), Some(chat_id)) = (self.tg, self.selected_chat) else { return };
        let input = if in_thread { self.thread_composer.clone() } else { self.composer.clone() };
        let text = input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(reply_to) = self.reply_target(in_thread, chat_id, cx) else { return };
        input.update(cx, |input, cx| input.set_value("", window, cx));

        cx.spawn_in(window, async move |this, cx| {
            if let Err(error) = tg.send_text(chat_id, text.clone(), reply_to).await {
                let _ = this.update_in(cx, |app, window, cx| {
                    let selected = app.selected_chat == Some(chat_id);
                    let thread_open = reply_thread_open(reply_to, app.open_thread, &app.threads, &app.messages);
                    match restore_target(selected, in_thread, thread_open) {
                        Restore::Composer => restore_failed_text(&input, text, window, cx),
                        Restore::Drop => {}
                        Restore::Draft => park_draft(&mut app.drafts, chat_id, &text),
                    }
                    app.chat_error(chat_id, error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// What a send from the main or thread composer replies to: nothing, or
    /// the open thread's root. `None` refuses the send, with the error shown.
    pub(crate) fn reply_target(
        &mut self,
        in_thread: bool,
        chat_id: i64,
        cx: &mut Context<Self>,
    ) -> Option<Option<i64>> {
        if !in_thread {
            return Some(None);
        }
        // A thread reply is a plain Telegram reply to the thread's root.
        // TDLib silently drops a reply to a message that hasn't reached the
        // server and posts it to the whole chat, so the text stays here.
        let refusal = match self.thread_root_state() {
            Some(SendState::Sent) => None,
            Some(SendState::Failed) => Some("This message wasn't sent, so it can't be replied to."),
            Some(SendState::Pending) | None => Some("Wait until the message is sent before replying in its thread."),
        };
        if let Some(refusal) = refusal {
            self.show_error(refusal.into(), Some(chat_id));
            cx.notify();
            return None;
        }
        Some(self.open_thread)
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
                // The thread panel follows its root to the server id, so it
                // stays open. No reply can point at the temporary id: thread
                // replies wait for their root to be sent, and TDLib drops any
                // other reply to an unsent message.
                let mut changed = false;
                if self.open_thread == Some(*old_id) {
                    self.open_thread = Some(message.id);
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
            Event::MessageContent { chat_id, message_id, text, media } if selected(*chat_id) => {
                match self.messages.iter_mut().find(|m| m.id == *message_id) {
                    Some(message) if message.text != *text || message.media != *media => {
                        message.text = text.clone();
                        message.media = media.clone();
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
        // The thread panel follows its root into the thread it joined, e.g.
        // when history brings in the message it replies to, and closes if
        // the root is gone, so replies can't go to a deleted message.
        if let Some(root_id) = self.open_thread {
            self.open_thread = thread_root_of(root_id, &threads, &self.messages);
            if self.open_thread.is_none() {
                self.panel_toggled();
                if self.thread_composer.read(cx).focus_handle(cx).is_focused(window) {
                    self.composer.update(cx, |input, cx| input.focus(window, cx));
                }
                self.thread_composer.update(cx, |input, cx| input.set_value("", window, cx));
            }
        }
        // Keep the thread panel at its newest reply, unless the user scrolled up.
        let (offset, max) = (self.thread_scroll.offset(), self.thread_scroll.max_offset());
        if self.open_thread.is_some() && offset.y <= -max.y + px(1.) {
            self.thread_scroll.scroll_to_bottom();
        }
        let facts: Vec<RowFacts> = threads
            .iter()
            .map(|t| RowFacts::of(&self.messages[t.root], !t.replies.is_empty(), t.root_is_reply))
            .collect();
        let looks = row_looks(&facts, self.settings.chats.group_minutes);
        // Only replace the rows that changed, so the list keeps its scroll
        // position when a message arrives. `reset` is for switching chats.
        let keys = row_keys(&threads, &self.messages, &looks);
        let (old_rows, new_count) = changed_rows(&self.thread_keys, &keys);
        if old_rows.len() == new_count {
            // E.g. an edit. Unlike `splice`, this keeps the top row's offset
            // when that's the one that changed, so the view doesn't jump.
            if new_count > 0 {
                self.message_list.remeasure_items(old_rows);
            }
        } else {
            self.message_list.splice(old_rows, new_count);
        }
        self.threads = threads;
        self.row_looks = looks;
        self.thread_keys = keys;
        self.fetch_media(cx);
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

/// Puts the text of a failed send back into the composer it came from. Text
/// typed there meanwhile comes first, as in `park_draft`, and keeps its caret
/// and selection.
pub(crate) fn restore_failed_text(input: &Entity<TextareaState>, text: String, window: &mut Window, cx: &mut App) {
    let current = input.read(cx).value().to_string();
    if current.is_empty() {
        return restore_text(input, text, window, cx);
    }
    input.update(cx, |input, cx| {
        let (selection, reversed) = (input.selected_range(), input.is_selection_reversed());
        let end = input.text().len();
        input.set_selected_range(end..end, cx);
        // Inserted rather than set, so it can be undone and the view doesn't
        // jump back to the top.
        input.insert(appendix(&current, &text), window, cx);
        input.set_selected_range_with_direction(selection, reversed, cx);
    });
}

/// Where the text of a failed send goes back to. Its error shows either way,
/// right away or when the user returns to the chat.
#[derive(Debug, PartialEq)]
pub(crate) enum Restore {
    /// The composer it was sent from, after anything typed there meanwhile.
    Composer,
    /// Its chat's draft, for when that chat is no longer selected.
    Draft,
    /// Nowhere it can be sent from as it was meant; the error says it wasn't sent.
    Drop,
}

pub(crate) fn restore_target(chat_selected: bool, in_thread: bool, thread_open: bool) -> Restore {
    if in_thread {
        // A thread reply only goes back to its own thread's composer. In the
        // main composer or draft, Enter would post it to the whole chat.
        if chat_selected && thread_open { Restore::Composer } else { Restore::Drop }
    } else if chat_selected {
        Restore::Composer
    } else {
        Restore::Draft
    }
}

/// Whether the thread holding the reply target `reply_to` is still the open one.
/// The panel may have followed its root into an older thread since the send,
/// so this compares thread roots. A target that's no longer loaded is in none.
pub(crate) fn reply_thread_open(
    reply_to: Option<i64>,
    open_thread: Option<i64>,
    threads: &[Thread],
    messages: &[Msg],
) -> bool {
    reply_to.and_then(|id| thread_root_of(id, threads, messages)).is_some_and(|root| open_thread == Some(root))
}

/// Adds `text` to `chat_id`'s draft, after whatever was typed there since.
pub(crate) fn park_draft(drafts: &mut HashMap<i64, String>, chat_id: i64, text: &str) {
    let draft = drafts.entry(chat_id).or_default();
    let appendix = appendix(draft, text);
    draft.push_str(&appendix);
}

/// What to add to `existing` so that `text` follows it on a line of its own.
fn appendix(existing: &str, text: &str) -> String {
    if existing.is_empty() { text.to_string() } else { format!("\n{text}") }
}

/// Settles a load's successful `open_chat`. The current load's chat becomes
/// `opened_chat`, for the next `select_chat` to close; a superseded load's
/// chat is returned, to be closed right away.
fn settle_open(opened_chat: &mut Option<i64>, chat_id: i64, current: bool) -> Option<i64> {
    if !current {
        return Some(chat_id);
    }
    // `select_chat` took the previous one, and only one load is current.
    debug_assert!(opened_chat.is_none());
    *opened_chat = Some(chat_id);
    None
}

/// The error banner. Errors about a chat hide when the user switches away
/// from it; the others, e.g. a failed chat list load, stay until dismissed.
#[derive(Default)]
pub(crate) struct ErrorBanner {
    global: Vec<String>,
    /// Errors about the selected chat, and its id.
    chat: Option<(i64, Vec<String>)>,
}

impl ErrorBanner {
    /// Adds `error`, about `chat` if that's set, after those shown already.
    fn push(&mut self, error: String, chat: Option<i64>) {
        let errors = match chat {
            None => &mut self.global,
            Some(chat_id) => {
                let chat = self.chat.get_or_insert_with(|| (chat_id, Vec::new()));
                // Errors about a chat that isn't selected anymore are stale.
                if chat.0 != chat_id {
                    *chat = (chat_id, Vec::new());
                }
                &mut chat.1
            }
        };
        push_distinct(errors, error);
    }

    fn clear_chat(&mut self) {
        self.chat = None;
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// The banner's text, global errors first, if there are any.
    pub(crate) fn text(&self) -> Option<String> {
        let mut errors: Vec<&str> = Vec::new();
        for error in self.global.iter().chain(self.chat.iter().flat_map(|(_, errors)| errors)) {
            if !errors.contains(&error.as_str()) {
                errors.push(error);
            }
        }
        (!errors.is_empty()).then(|| errors.join("; "))
    }
}

/// Adds `error` unless it's there already, so a repeated failure shows once.
fn push_distinct(errors: &mut Vec<String>, error: String) {
    if !errors.contains(&error) {
        errors.push(error);
    }
}

/// Identifies a message list row by content, so the list knows which rows
/// to measure again. `Thread` holds indices into `messages`, which shift
/// whenever a message above is added or removed, so this holds ids. Besides
/// identity it holds whatever changes the row's height.
#[derive(Debug, PartialEq)]
struct RowKey {
    root: i64,
    replies: Vec<i64>,
    root_is_reply: bool,
    /// How it groups with the row above, e.g. after the row above is deleted.
    look: RowLook,
    /// The root's text and media size, which an edit changes.
    content: u64,
}

fn row_keys(threads: &[Thread], messages: &[Msg], looks: &[RowLook]) -> Vec<RowKey> {
    threads
        .iter()
        .zip(looks)
        .map(|(t, &look)| {
            let root = &messages[t.root];
            RowKey {
                root: root.id,
                replies: t.replies.iter().map(|&i| messages[i].id).collect(),
                root_is_reply: t.root_is_reply,
                look,
                content: content_revision(root),
            }
        })
        .collect()
}

/// A hash of what an edit can change about a message's height: its text and
/// whether it has media, and of what size. A hash, so keys don't copy every text.
fn content_revision(message: &Msg) -> u64 {
    let mut hasher = DefaultHasher::new();
    message.text.hash(&mut hasher);
    message.media.as_ref().map(|media| media.size()).hash(&mut hasher);
    hasher.finish()
}

/// The root id of the thread holding message `id`, if that's loaded.
fn thread_root_of(id: i64, threads: &[Thread], messages: &[Msg]) -> Option<i64> {
    threads
        .iter()
        .find(|t| std::iter::once(t.root).chain(t.replies.iter().copied()).any(|i| messages[i].id == id))
        .map(|t| messages[t.root].id)
}

/// The range of `old` rows that differ from `new`, and how many `new` rows
/// replace them: everything between their common prefix and common suffix.
fn changed_rows<T: PartialEq>(old: &[T], new: &[T]) -> (Range<usize>, usize) {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
    (prefix..old.len() - suffix, new.len() - prefix - suffix)
}

/// Ids of the chats to list, in Telegram's order, whose titles contain
/// `query` (lowercase; "" lists all).
fn visible_chats(chats: &HashMap<i64, ChatSummary>, query: &str) -> Vec<i64> {
    // Channels stay hidden until sponsored messages are shown, which
    // Telegram's API terms require for any client that displays channels.
    let mut visible: Vec<&ChatSummary> = chats
        .values()
        .filter(|c| c.order > 0 && !c.is_channel)
        .filter(|c| query.is_empty() || c.title.to_lowercase().contains(query))
        .collect();
    visible.sort_by_key(|c| std::cmp::Reverse((c.order, c.id)));
    visible.into_iter().map(|c| c.id).collect()
}

impl Render for FrillogramApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.chats_dirty) {
            self.visible_chats = visible_chats(&self.chats, &self.chat_query);
        }
        let body = if let Some(error) = &self.setup_error {
            self.render_setup(error, cx).into_any_element()
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

    use frillogram_core::{ChatSummary, Media, Msg, Photo, SendState, SenderId, threads};

    use super::{
        ErrorBanner, Restore, appendix, changed_rows, park_draft, reply_thread_open, restore_target, row_keys,
        settle_open, thread_root_of, visible_chats,
    };
    use crate::chat::RowLook;

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
            media: None,
            reply_to,
            outgoing: false,
            send_state: SendState::Sent,
            can_be_saved: true,
        };
        let before = [msg(1, None), msg(2, None), msg(3, Some(2)), msg(4, None)];
        let after = &before[1..];
        let looks = [RowLook::default(); 3];
        let old = row_keys(&threads::group(&before), &before, &looks);
        let new = row_keys(&threads::group(after), after, &looks);
        // Deleting message 1 removes only its row, although every index moved.
        assert_eq!(changed_rows(&old, &new), (0..1, 0));
        let ids: Vec<(i64, Vec<i64>)> = new.into_iter().map(|key| (key.root, key.replies)).collect();
        assert_eq!(ids, vec![(2, vec![3]), (4, vec![])]);
    }

    #[test]
    fn edited_roots_change_only_their_row() {
        let msg = |id: i64, text: &str, media: Option<Media>| Msg {
            id,
            chat_id: 1,
            sender: SenderId::User(1),
            date: 0,
            text: text.into(),
            media,
            reply_to: None,
            outgoing: false,
            send_state: SendState::Sent,
            can_be_saved: true,
        };
        let photo = |file_id: i32, width: u32| {
            Some(Media::Photo(Photo { file_id, width, height: 100, minithumbnail: None, has_spoiler: false }))
        };
        let keys = |messages: &[Msg]| row_keys(&threads::group(messages), messages, &[RowLook::default(); 3]);
        let before = [msg(1, "a", None), msg(2, "b", photo(7, 100)), msg(3, "c", None)];
        let old = keys(&before);
        let edited = |i: usize, edit: Msg| {
            let mut messages = before.clone();
            messages[i] = edit;
            changed_rows(&old, &keys(&messages))
        };
        assert_eq!(edited(1, msg(2, "b, edited", photo(7, 100))), (1..2, 1));
        assert_eq!(edited(1, msg(2, "b", photo(7, 200))), (1..2, 1));
        assert_eq!(edited(1, msg(2, "b", None)), (1..2, 1));
        assert_eq!(edited(0, msg(1, "", None)), (0..1, 1));
        // A new file of the same size, e.g. a re-upload, doesn't change the height.
        assert_eq!(edited(1, msg(2, "b", photo(8, 100))), (3..3, 0));
    }

    #[test]
    fn open_thread_follows_its_root_into_the_thread_it_joined() {
        let msg = |id: i64, reply_to: Option<i64>| Msg {
            id,
            chat_id: 1,
            sender: SenderId::User(1),
            date: 0,
            text: String::new(),
            media: None,
            reply_to,
            outgoing: false,
            send_state: SendState::Sent,
            can_be_saved: true,
        };
        // 5 replies to 2, which isn't loaded yet, so it roots its own thread.
        let before = [msg(5, Some(2)), msg(6, Some(5))];
        assert_eq!(thread_root_of(5, &threads::group(&before), &before), Some(5));
        // History brings in 2, and 5 becomes a reply in 2's thread.
        let after = [msg(1, None), msg(2, None), msg(5, Some(2)), msg(6, Some(5))];
        let threads = threads::group(&after);
        assert_eq!(thread_root_of(5, &threads, &after), Some(2));
        assert_eq!(thread_root_of(1, &threads, &after), Some(1));
        // A deleted root closes the panel.
        assert_eq!(thread_root_of(3, &threads, &after), None);
    }

    #[test]
    fn failed_reply_finds_its_thread_after_the_root_moved() {
        let msg = |id: i64, reply_to: Option<i64>| Msg {
            id,
            chat_id: 1,
            sender: SenderId::User(1),
            date: 0,
            text: String::new(),
            media: None,
            reply_to,
            outgoing: false,
            send_state: SendState::Sent,
            can_be_saved: true,
        };
        // The reply went to 5, then history brought in 2 and moved the thread to root 2.
        let messages = [msg(1, None), msg(2, None), msg(5, Some(2)), msg(6, Some(5))];
        let threads = threads::group(&messages);
        assert!(reply_thread_open(Some(5), Some(2), &threads, &messages));
        // Another thread is open, or none, or the reply target is gone.
        assert!(!reply_thread_open(Some(5), Some(1), &threads, &messages));
        assert!(!reply_thread_open(Some(5), None, &threads, &messages));
        assert!(!reply_thread_open(Some(3), Some(2), &threads, &messages));
        assert!(!reply_thread_open(None, Some(2), &threads, &messages));
    }

    #[test]
    fn visible_chats_skip_channels_and_unlisted_and_sort_by_order() {
        let chat = |id: i64, order: i64, is_channel: bool| {
            (id, ChatSummary { id, title: String::new(), last_message: None, unread: 0, order, is_channel })
        };
        let chats = HashMap::from([chat(1, 5, false), chat(2, 9, false), chat(3, 7, true), chat(4, 0, false)]);
        assert_eq!(visible_chats(&chats, ""), vec![2, 1]);
    }

    #[test]
    fn chat_search_matches_titles_ignoring_case() {
        let chat = |id: i64, title: &str| {
            let summary =
                ChatSummary { id, title: title.into(), last_message: None, unread: 0, order: id, is_channel: false };
            (id, summary)
        };
        let chats = HashMap::from([chat(1, "Design Team"), chat(2, "Ada Lovelace"), chat(3, "Team Ünal")]);
        assert_eq!(visible_chats(&chats, "team"), vec![3, 1]);
        assert_eq!(visible_chats(&chats, "ün"), vec![3]);
        assert!(visible_chats(&chats, "zzz").is_empty());
    }

    #[test]
    fn failed_send_text_goes_back_where_it_can_be_sent_again() {
        assert_eq!(restore_target(true, false, false), Restore::Composer);
        assert_eq!(restore_target(true, true, true), Restore::Composer);
        assert_eq!(restore_target(false, false, false), Restore::Draft);
    }

    #[test]
    fn failed_thread_reply_never_lands_in_the_main_draft() {
        assert_eq!(restore_target(true, true, false), Restore::Drop);
        assert_eq!(restore_target(false, true, true), Restore::Drop);
        assert_eq!(restore_target(false, true, false), Restore::Drop);
    }

    #[test]
    fn parked_text_is_appended_to_an_existing_draft() {
        let mut drafts = HashMap::new();
        park_draft(&mut drafts, 1, "first");
        assert_eq!(drafts[&1], "first");
        park_draft(&mut drafts, 1, "second");
        assert_eq!(drafts[&1], "first\nsecond");
    }

    #[test]
    fn appended_text_goes_on_its_own_line() {
        assert_eq!(appendix("", "failed"), "failed");
        assert_eq!(appendix("typed", "failed"), "\nfailed");
    }

    /// `select_chat`'s open and close calls against TDLib's per-chat open
    /// count, with each load's steps run when a test says so.
    #[derive(Default)]
    struct OpenCounts {
        opened_chat: Option<i64>,
        generation: u64,
        counts: HashMap<i64, i32>,
    }

    impl OpenCounts {
        /// `select_chat`, up to and including its close of the previous chat.
        fn select(&mut self, chat_id: i64) -> (u64, i64) {
            self.generation += 1;
            if let Some(chat) = self.opened_chat.take() {
                self.close(chat);
            }
            (self.generation, chat_id)
        }

        /// The load's `open_chat` and what follows it, or nothing if it was
        /// superseded before getting there.
        fn open(&mut self, (generation, chat_id): (u64, i64)) {
            if generation != self.generation {
                return;
            }
            *self.counts.entry(chat_id).or_default() += 1;
            self.settle((generation, chat_id));
        }

        /// Like `open`, but the open resolves only after `later` ran.
        fn open_around(&mut self, load: (u64, i64), later: impl FnOnce(&mut Self)) {
            if load.0 != self.generation {
                return;
            }
            *self.counts.entry(load.1).or_default() += 1;
            later(self);
            self.settle(load);
        }

        fn settle(&mut self, (generation, chat_id): (u64, i64)) {
            let current = generation == self.generation;
            if let Some(chat) = settle_open(&mut self.opened_chat, chat_id, current) {
                self.close(chat);
            }
        }

        fn close(&mut self, chat_id: i64) {
            let count = self.counts.entry(chat_id).or_default();
            // TDLib ignores a close at 0, which would hide an unbalanced one.
            assert!(*count > 0, "closed chat {chat_id} more often than it was opened");
            *count -= 1;
        }

        /// Only `opened_chat` is still open on TDLib.
        fn assert_balanced(&self) {
            for (&chat_id, &count) in &self.counts {
                assert_eq!(count, i32::from(self.opened_chat == Some(chat_id)), "chat {chat_id}");
            }
        }
    }

    #[test]
    fn switching_chats_closes_the_previous_one() {
        let mut tdlib = OpenCounts::default();
        let a = tdlib.select(1);
        tdlib.open(a);
        let b = tdlib.select(2);
        tdlib.open(b);
        tdlib.assert_balanced();
        assert_eq!(tdlib.opened_chat, Some(2));
    }

    #[test]
    fn superseded_open_is_closed_even_if_its_chat_is_selected_again() {
        let mut tdlib = OpenCounts::default();
        let first = tdlib.select(1);
        tdlib.open_around(first, |tdlib| {
            let b = tdlib.select(2);
            let again = tdlib.select(1);
            tdlib.open(b);
            tdlib.open(again);
        });
        tdlib.assert_balanced();
        assert_eq!(tdlib.opened_chat, Some(1));
        let c = tdlib.select(3);
        tdlib.open(c);
        tdlib.assert_balanced();
    }

    #[test]
    fn load_superseded_before_opening_closes_nothing() {
        let mut tdlib = OpenCounts::default();
        let a = tdlib.select(1);
        let b = tdlib.select(2);
        tdlib.open(a);
        tdlib.open(b);
        // The chat selected before 2 was never opened, so only 2 is closed.
        let c = tdlib.select(3);
        tdlib.open(c);
        tdlib.assert_balanced();
        assert_eq!(tdlib.counts.get(&1), None);
    }

    #[test]
    fn retrying_a_failed_load_reopens_the_same_chat_once() {
        let mut tdlib = OpenCounts::default();
        let a = tdlib.select(1);
        tdlib.open(a);
        let retry = tdlib.select(1);
        tdlib.open(retry);
        tdlib.assert_balanced();
        assert_eq!(tdlib.counts[&1], 1);
    }

    #[test]
    fn errors_are_joined_once_each() {
        let mut banner = ErrorBanner::default();
        assert_eq!(banner.text(), None);
        banner.push("a".into(), Some(1));
        banner.push("b".into(), Some(1));
        banner.push("a".into(), Some(1));
        assert_eq!(banner.text().as_deref(), Some("a; b"));
    }

    #[test]
    fn chat_errors_never_replace_global_ones() {
        let mut banner = ErrorBanner::default();
        banner.push("chats failed".into(), None);
        banner.push("send failed".into(), Some(1));
        assert_eq!(banner.text().as_deref(), Some("chats failed; send failed"));
        // Switching chats hides only the chat's errors.
        banner.clear_chat();
        assert_eq!(banner.text().as_deref(), Some("chats failed"));
        banner.clear();
        assert_eq!(banner.text(), None);
    }
}
