use std::ops::Range;

use chrono::{DateTime, Local, NaiveDate};
use frillogram_core::{Msg, SendState, SenderId};
use gpui_kit::{
    component::{
        ActiveTheme,
        button::{Button, ButtonVariants},
        h_flex,
        input::Textarea,
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};

use crate::app::FrillogramApp;

const SIDEBAR_WIDTH: f32 = 300.;
const THREAD_PANEL_WIDTH: f32 = 400.;

impl FrillogramApp {
    pub(crate) fn render_main(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Read the clock once per frame, not once per message.
        let today = Local::now().date_naive();
        let mut root = h_flex().size_full().child(self.render_chat_list(cx)).child(self.render_chat(today, cx));
        if let Some(root_id) = self.open_thread {
            root = root.child(self.render_thread_panel(root_id, today, cx));
        }
        root
    }

    fn render_chat_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Only the rows in view are laid out, since the list can hold
        // thousands of chats.
        let rows = uniform_list(
            "chat-list",
            self.visible_chats.len(),
            cx.processor(|this, range: Range<usize>, _window, cx| {
                range.map(|ix| this.render_chat_row(ix, cx)).collect::<Vec<_>>()
            }),
        )
        .flex_1();

        v_flex()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(div().px_3().py_2().text_lg().font_weight(FontWeight::BOLD).child("Chats"))
            .child(rows)
    }

    fn render_chat_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(chat) = self.visible_chats.get(ix).and_then(|id| self.chats.get(id)) else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let (active, hover, muted, primary, primary_fg) =
            (theme.list_active, theme.list_hover, theme.muted_foreground, theme.primary, theme.primary_foreground);

        let chat_id = chat.id;
        let selected = self.selected_chat == Some(chat_id);
        let mut title_row = h_flex()
            .gap_2()
            .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(chat.title.clone()));
        if chat.unread > 0 {
            title_row = title_row.child(
                div()
                    .px_1p5()
                    .rounded_full()
                    .bg(primary)
                    .text_color(primary_fg)
                    .text_xs()
                    .child(chat.unread.to_string()),
            );
        }
        h_flex()
            // The cast wraps negative (group) ids, which keeps them distinct.
            .id(("chat", chat_id as u64))
            // A list item is laid out on its own, so it has to ask for the full width.
            .w_full()
            .gap_3()
            .px_3()
            .py_2()
            .when(selected, |row| row.bg(active))
            // The hover color would hide the selection.
            .when(!selected, |row| row.hover(move |row| row.bg(hover)))
            .on_click(cx.listener(move |this, _, window, cx| this.select_chat(chat_id, window, cx)))
            .child(avatar(&chat.title, chat_id))
            .child(v_flex().flex_1().min_w_0().child(title_row).child(
                div().truncate().text_sm().text_color(muted).child(chat.last_message.clone().unwrap_or_default()),
            ))
            .into_any_element()
    }

    fn render_chat(&self, today: NaiveDate, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let Some(chat) = self.selected_chat.and_then(|id| self.chats.get(&id)) else {
            return v_flex()
                .flex_1()
                .h_full()
                .items_center()
                .justify_center()
                .child(div().text_color(cx.theme().muted_foreground).child("Pick a chat"))
                .children(self.render_error(cx));
        };

        let list = list(
            self.message_list.clone(),
            cx.processor(move |this, ix: usize, _window, cx| this.render_thread_row(ix, today, cx).into_any_element()),
        )
        .size_full();

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(border)
                    .font_weight(FontWeight::BOLD)
                    .child(chat.title.clone()),
            )
            .child(div().flex_1().min_h_0().child(list))
            .children(self.render_error(cx))
            .child(div().p_3().border_t_1().border_color(border).child(Textarea::new(&self.composer)))
    }

    /// The last error, if any, with a button to dismiss it.
    fn render_error(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let error = self.error.text()?;
        Some(
            h_flex()
                .px_4()
                .py_1()
                .gap_2()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(div().flex_1().min_w_0().child(error))
                .child(Button::new("dismiss-error").ghost().label("Dismiss").on_click(cx.listener(
                    |this, _, window, cx| {
                        this.error.clear();
                        // Clicking moved focus to this button, which is now
                        // gone. The thread composer only shows for a sent root.
                        if this.thread_composer_focused_last && this.thread_root_state() == Some(SendState::Sent) {
                            this.thread_composer.update(cx, |input, cx| input.focus(window, cx));
                        } else if this.selected_chat.is_some() {
                            this.composer.update(cx, |input, cx| input.focus(window, cx));
                        }
                        cx.notify();
                    },
                ))),
        )
    }

    /// One row of the main list: a thread's root message and a summary of its replies.
    fn render_thread_row(&self, ix: usize, today: NaiveDate, cx: &mut Context<Self>) -> Div {
        let Some(thread) = self.threads.get(ix) else { return div() };
        let root = &self.messages[thread.root];
        let muted = cx.theme().muted_foreground;
        let link = cx.theme().primary;

        let root_id = root.id;
        let mut row = v_flex().group("thread-row").relative().px_4().py_1p5();
        if thread.root_is_reply {
            row = row.child(div().pl(px(44.)).text_xs().text_color(muted).child("↪ reply to an earlier message"));
        }
        row = row.child(self.render_message(root, today, cx));

        if let Some(&last) = thread.replies.last() {
            let count = thread.replies.len();
            let label = if count == 1 { "1 reply".to_string() } else { format!("{count} replies") };
            row = row.child(
                h_flex()
                    .id(("thread", root_id as u64))
                    .ml(px(44.))
                    .gap_2()
                    .text_sm()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| this.open_thread(Some(root_id), window, cx)))
                    .child(div().text_color(link).font_weight(FontWeight::SEMIBOLD).child(label))
                    .child(
                        div()
                            .text_color(muted)
                            .child(format!("Last reply {}", format_time(self.messages[last].date, today))),
                    ),
            );
        } else if root.send_state == SendState::Sent {
            // A message without replies starts a thread from a link shown on
            // hover. TDLib can't reply to a message that isn't on the server yet.
            row = row.child(
                div()
                    .id(("reply-in-thread", root_id as u64))
                    .absolute()
                    .top_1p5()
                    .right_4()
                    .invisible()
                    .group_hover("thread-row", |link| link.visible())
                    .text_sm()
                    .text_color(link)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| this.open_thread(Some(root_id), window, cx)))
                    .child("Reply in thread"),
            );
        }
        row
    }

    fn render_thread_panel(&self, root_id: i64, today: NaiveDate, cx: &mut Context<Self>) -> impl IntoElement {
        let (border, muted) = (cx.theme().border, cx.theme().muted_foreground);
        let thread = self.threads.iter().find(|t| self.messages[t.root].id == root_id);
        let messages: Vec<&Msg> = thread
            .map(|t| std::iter::once(t.root).chain(t.replies.iter().copied()).map(|i| &self.messages[i]).collect())
            .unwrap_or_default();

        v_flex()
            .w(px(THREAD_PANEL_WIDTH))
            .h_full()
            .flex_none()
            .border_l_1()
            .border_color(border)
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(border)
                    .justify_between()
                    .child(div().font_weight(FontWeight::BOLD).child("Thread"))
                    .child(
                        Button::new("close-thread")
                            .ghost()
                            .label("Close")
                            .on_click(cx.listener(|this, _, window, cx| this.open_thread(None, window, cx))),
                    ),
            )
            .child(
                v_flex()
                    .id("thread-messages")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.thread_scroll)
                    .px_4()
                    .py_2()
                    .gap_2()
                    .children(messages.into_iter().map(|m| self.render_message(m, today, cx))),
            )
            .child(div().p_3().border_t_1().border_color(border).map(|footer| {
                // A reply to a root that isn't on the server yet would go to
                // the whole chat, so there's no composer until it is.
                match self.thread_root_state() {
                    Some(SendState::Sent) => footer.child(Textarea::new(&self.thread_composer)),
                    Some(SendState::Pending) => footer.text_sm().text_color(muted).child("Sending…"),
                    Some(SendState::Failed) | None => footer.text_sm().text_color(muted).child("Not sent"),
                }
            }))
    }

    fn render_message(&self, message: &Msg, today: NaiveDate, cx: &App) -> Div {
        let name = self.sender_name(message.sender);
        let sender_id = match message.sender {
            SenderId::User(id) | SenderId::Chat(id) => id,
        };
        let (muted, danger) = (cx.theme().muted_foreground, cx.theme().danger);
        h_flex().items_start().gap_3().py_1().child(avatar(&name, sender_id)).child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().font_weight(FontWeight::BOLD).child(name))
                        .child(div().text_xs().text_color(muted).child(format_time(message.date, today)))
                        .when(message.send_state == SendState::Failed, |header| {
                            header.child(div().text_xs().text_color(danger).child("Not sent"))
                        }),
                )
                .child(
                    div()
                        .when(message.send_state == SendState::Pending, |text| text.text_color(muted))
                        .child(message.text.clone()),
                ),
        )
    }
}

/// A colored circle with the first letter of `name`; the color is stable per id.
fn avatar(name: &str, id: i64) -> impl IntoElement {
    let hue = (id.unsigned_abs() % 360) as f32 / 360.;
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    div()
        .size(px(32.))
        .flex_none()
        .rounded_full()
        .bg(hsla(hue, 0.55, 0.5, 1.))
        .text_color(white())
        .font_weight(FontWeight::BOLD)
        .flex()
        .items_center()
        .justify_center()
        .child(initial)
}

fn format_time(timestamp: i32, today: NaiveDate) -> String {
    let Some(time) = DateTime::from_timestamp(timestamp.into(), 0) else { return String::new() };
    let time = time.with_timezone(&Local);
    if time.date_naive() == today { time.format("%H:%M").to_string() } else { time.format("%b %-d, %H:%M").to_string() }
}
