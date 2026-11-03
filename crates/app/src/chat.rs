//! The signed-in window: chat list, message list, thread panel and their
//! headers. Which rows group together, what day separators say and how
//! avatars look is decided by the pure functions at the bottom; the
//! `render_*` methods draw it.

use std::ops::Range;

use chrono::{DateTime, Datelike, Local, NaiveDate};
use frillogram_core::{
    Msg, SendState, SenderId,
    config::{Appearance, Density, Time},
    threads::Thread,
};
use gpui_kit::{
    assets::IconName as Lucide,
    component::{
        ActiveTheme, Icon, IconName, Sizable, ThemeStyled,
        button::{Button, ButtonVariants},
        h_flex, v_flex,
    },
    prelude::FluentBuilder,
    *,
};

use crate::app::FrillogramApp;

/// The hover group of a main list row, whose action bar shows while it's hovered.
const ROW_GROUP: &str = "message-row";
/// The hover group of a message in the thread panel.
const PANEL_GROUP: &str = "thread-message";
/// The hover group of a thread summary, whose chevron shows while it's hovered.
const SUMMARY_GROUP: &str = "thread-summary";
/// How many of the latest repliers a thread summary shows.
const SUMMARY_AVATARS: usize = 3;
/// Message text and the time beside a continued message share this line
/// height, so the time sits on the text's first line.
const BODY_LINE_HEIGHT: f32 = 1.45;
/// Line height of a chat list row's title and preview.
const CHAT_LINE_HEIGHT: f32 = 1.3;
/// Line height of the header titles, GPUI's default.
const HEADER_LINE_HEIGHT: f32 = 1.618;
/// Font sizes of `text_sm` and `text_lg`, in rems.
const TEXT_SM: f32 = 0.875;
const TEXT_LG: f32 = 1.125;
/// Height of the unread badge in the chat list.
const BADGE_HEIGHT: Pixels = px(20.);

/// Sizes that `[appearance]` decides.
#[derive(Clone, Copy)]
struct Metrics {
    avatar: Pixels,
    /// Vertical padding of a message row that shows its sender, and of one
    /// that continues the row above.
    row_y: Pixels,
    continued_y: Pixels,
    /// Vertical padding of a chat list row.
    chat_y: Pixels,
    /// Heights of a chat list row's title line, which the unread badge fits
    /// in, and preview line. `uniform_list` measures only the first row, so
    /// no row's content may change its height.
    chat_title: Pixels,
    chat_preview: Pixels,
    /// Height of the chat list, chat and thread headers, which line up. Each
    /// fits its title and the chat header's avatar.
    header: Pixels,
    /// Side of the buttons in a row's action bar.
    action: Pixels,
}

impl Metrics {
    fn new(appearance: &Appearance) -> Self {
        let avatar = px(appearance.avatar_size as f32);
        // The root view sizes rems from the font size.
        let rem = px(appearance.font_size as f32);
        let chat_title = (rem * CHAT_LINE_HEIGHT).max(BADGE_HEIGHT);
        let chat_preview = rem * (TEXT_SM * CHAT_LINE_HEIGHT);
        // The thread header's close button is 1.5rem, shorter than a title.
        let header_content = avatar.max(rem * (TEXT_LG * HEADER_LINE_HEIGHT));
        match appearance.density {
            Density::Comfortable => Metrics {
                avatar,
                row_y: px(6.),
                continued_y: px(2.),
                chat_y: px(8.),
                chat_title,
                chat_preview,
                header: header_content + px(20.),
                action: px(24.),
            },
            Density::Compact => Metrics {
                avatar,
                row_y: px(3.),
                continued_y: px(1.),
                chat_y: px(4.),
                chat_title,
                chat_preview,
                header: header_content + px(12.),
                action: px(20.),
            },
        }
    }

    /// The action bar's padding around its buttons, and its inset from the
    /// row's top edge.
    const BAR_PADDING: Pixels = px(2.);
    const BAR_INSET: Pixels = px(2.);

    /// A message row is at least as tall as its action bar and insets, so the
    /// bar never sticks out of its row: hovering the part outside would be
    /// hovering the next row, which hides the bar under the pointer.
    fn row_min_height(&self) -> Pixels {
        self.action + Self::BAR_PADDING * 2. + Self::BAR_INSET * 2.
    }
}

impl FrillogramApp {
    pub(crate) fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Read the clock once per frame, not once per message.
        let today = Local::now().date_naive();
        let metrics = Metrics::new(&self.settings.appearance);
        let mut root = h_flex()
            .size_full()
            .child(self.render_chat_list(metrics, cx))
            .child(self.render_chat(today, metrics, window, cx));
        if let Some(root_id) = self.open_thread {
            root = root.child(self.render_thread_panel(root_id, today, metrics, window, cx));
        }
        root
    }

    fn render_chat_list(&self, metrics: Metrics, cx: &mut Context<Self>) -> impl IntoElement {
        // Only the rows in view are laid out, since the list can hold
        // thousands of chats. They all have the same height, which
        // `uniform_list` relies on.
        let rows = uniform_list(
            "chat-list",
            self.visible_chats.len(),
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                range.map(|ix| this.render_chat_row(ix, metrics, cx)).collect::<Vec<_>>()
            }),
        )
        .flex_1();

        let theme = cx.theme();
        v_flex()
            .w(px(self.settings.layout.sidebar_width as f32))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(theme.sidebar_border)
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .child(
                h_flex()
                    .h(metrics.header)
                    .flex_none()
                    .px_4()
                    .border_b_1()
                    .border_color(theme.sidebar_border)
                    .child(header_title().font_weight(FontWeight::BOLD).child("Chats")),
            )
            .child(rows)
    }

    fn render_chat_row(&self, ix: usize, metrics: Metrics, cx: &mut Context<Self>) -> AnyElement {
        let Some(chat) = self.visible_chats.get(ix).and_then(|id| self.chats.get(id)) else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        // Tints of the accent, which are greys when no accent is set.
        let (selected_bg, hover_bg) = (theme.primary.opacity(0.12), theme.primary.opacity(0.06));
        let (muted, primary, primary_fg, radius) =
            (theme.muted_foreground, theme.primary, theme.primary_foreground, theme.radius_lg);

        let chat_id = chat.id;
        let selected = self.selected_chat == Some(chat_id);
        let mut title_row = h_flex().h(metrics.chat_title).gap_2().child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .line_height(relative(CHAT_LINE_HEIGHT))
                .child(chat.title.clone()),
        );
        if chat.unread > 0 {
            title_row = title_row.child(
                div()
                    .flex_none()
                    .min_w(BADGE_HEIGHT)
                    .h(BADGE_HEIGHT)
                    .px_1p5()
                    .rounded_full()
                    .bg(primary)
                    .text_color(primary_fg)
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(unread_label(chat.unread)),
            );
        }
        // A list item is laid out on its own, so it has to ask for the full
        // width. The padding insets the rounded highlight from the edges.
        div()
            .w_full()
            .px_2()
            .py(px(1.))
            .child(
                h_flex()
                    // The cast wraps negative (group) ids, which keeps them distinct.
                    .id(("chat", chat_id as u64))
                    .w_full()
                    .gap_3()
                    .px_2()
                    .py(metrics.chat_y)
                    .rounded(radius)
                    .when(selected, |row| row.bg(selected_bg))
                    // The hover color would hide the selection.
                    .when(!selected, |row| row.hover(move |row| row.bg(hover_bg)))
                    .on_click(cx.listener(move |this, _, window, cx| this.select_chat(chat_id, window, cx)))
                    .child(avatar(&chat.title, chat_id, metrics.avatar))
                    .child(
                        v_flex().flex_1().min_w_0().gap_0p5().child(title_row).child(
                            div()
                                .h(metrics.chat_preview)
                                .truncate()
                                .text_sm()
                                .line_height(relative(CHAT_LINE_HEIGHT))
                                .text_color(muted)
                                .child(chat.last_message.clone().unwrap_or_default()),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_chat(
        &self,
        today: NaiveDate,
        metrics: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(chat) = self.selected_chat.and_then(|id| self.chats.get(&id)) else {
            return self.render_no_chat(cx).into_any_element();
        };
        let border = cx.theme().border;

        let list = list(
            self.message_list.clone(),
            cx.processor(move |this, ix: usize, _window, cx| this.render_thread_row(ix, today, metrics, cx)),
        )
        .size_full();

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            // GPUI redraws on every move of a drag, so this follows it.
            .drag_over::<ExternalPaths>(|style, _, _, cx| style.bg(cx.theme().drop_target))
            .child(
                h_flex()
                    .h(metrics.header)
                    .flex_none()
                    .px_4()
                    .gap_3()
                    .border_b_1()
                    .border_color(border)
                    .child(avatar(&chat.title, chat.id, metrics.avatar))
                    .child(
                        header_title()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(chat.title.clone()),
                    ),
            )
            .child(div().flex_1().min_h_0().child(list))
            .children(self.render_error(cx))
            .child(div().flex_none().px_4().pt_1().pb_3().child(self.render_composer(false, window, cx)))
            // Image files dropped on the chat are sent like attached ones.
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.send_files(paths.paths().to_vec(), false, window, cx)
            }))
            .into_any_element()
    }

    /// What the chat area shows before a chat is picked.
    fn render_no_chat(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .size_16()
                    .rounded_full()
                    .bg(theme.muted)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(Lucide::MessagesSquare).with_size(px(28.)).text_color(theme.muted_foreground)),
            )
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("No chat open"))
            .child(div().text_sm().text_color(theme.muted_foreground).child("Pick a chat from the list to read it."))
            .children(self.render_error(cx))
    }

    /// The last error, if any, with a button to dismiss it.
    fn render_error(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let error = self.error.text()?;
        let (danger, radius) = (cx.theme().danger, cx.theme().radius);
        Some(
            div().flex_none().px_4().pb_2().child(
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .rounded(radius)
                    .border_1()
                    .border_color(danger.opacity(0.35))
                    .bg(danger.opacity(0.08))
                    .text_sm()
                    .child(Icon::new(IconName::CircleAlert).small().text_color(danger))
                    .child(div().flex_1().min_w_0().text_color(danger).child(error))
                    .child(Button::new("dismiss-error").ghost().small().label("Dismiss").on_click(cx.listener(
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
            ),
        )
    }

    /// One row of the main list: a day separator if a new day starts here,
    /// then a thread's root message and a summary of its replies.
    ///
    /// Hover only changes colors and visibility, never sizes, so the list's
    /// measured row heights stay valid.
    fn render_thread_row(&self, ix: usize, today: NaiveDate, m: Metrics, cx: &mut Context<Self>) -> AnyElement {
        let (Some(thread), Some(&look)) = (self.threads.get(ix), self.row_looks.get(ix)) else {
            return div().into_any_element();
        };
        let root = &self.messages[thread.root];
        let root_id = root.id;
        let (hover_bg, muted) = (cx.theme().list_hover, cx.theme().muted_foreground);

        // The row has an id, so GPUI keeps its hover state and repaints it as
        // the pointer moves; without one, `hover` would only update when
        // something else redraws the window.
        let mut row = v_flex()
            .id(("row", root_id as u64))
            .group(ROW_GROUP)
            .relative()
            .w_full()
            .min_h(m.row_min_height())
            .px_4()
            .py(if look.continued { m.continued_y } else { m.row_y })
            // The open thread's root keeps the hover color, so it's easy to find.
            .when(self.open_thread == Some(root_id), |row| row.bg(hover_bg))
            .hover(move |row| row.bg(hover_bg));
        if thread.root_is_reply {
            row = row
                .child(div().pl(m.avatar + px(12.)).text_xs().text_color(muted).child("↪ reply to an earlier message"));
        }
        let summary = (!thread.replies.is_empty())
            .then(|| self.render_thread_summary(thread, root_id, today, cx).into_any_element());
        row = row.child(self.render_message(root, look.continued, ROW_GROUP, today, m, summary, cx));

        // A message without replies starts a thread from its action bar. TDLib
        // can't reply to a message that isn't on the server yet.
        if !thread.replies.is_empty() || root.send_state == SendState::Sent {
            let tooltip = if thread.replies.is_empty() { "Reply in thread" } else { "Open thread" };
            row = row.child(
                // Always laid out, and only painted while the row is hovered,
                // so it never moves anything. A hidden element paints nothing
                // and registers no listeners, so it can't be clicked either.
                h_flex()
                    .id(("row-actions", root_id as u64))
                    .absolute()
                    .top(Metrics::BAR_INSET)
                    .right_3()
                    .p(Metrics::BAR_PADDING)
                    .popover_style(cx)
                    .invisible()
                    .group_hover(ROW_GROUP, |bar| bar.visible())
                    .child(
                        Button::new(("open-thread", root_id as u64))
                            .ghost()
                            .with_size(m.action)
                            .icon(Lucide::MessageSquareText)
                            .tooltip(tooltip)
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.open_thread_from_row(root_id, window, cx)),
                            ),
                    ),
            );
        }

        if look.day_break {
            v_flex()
                .w_full()
                .child(render_day_separator(day_label(local_day(root.date), today, &self.settings.time), cx))
                .child(row)
                .into_any_element()
        } else {
            row.into_any_element()
        }
    }

    /// "N replies" under a thread's root, with its latest repliers.
    fn render_thread_summary(
        &self,
        thread: &Thread,
        root_id: i64,
        today: NaiveDate,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (link, muted, background, border, radius) =
            (theme.primary, theme.muted_foreground, theme.background, theme.border, theme.radius);
        let count = thread.replies.len();
        let label = if count == 1 { "1 reply".to_string() } else { format!("{count} replies") };
        let last = thread.replies.last().map(|&i| self.messages[i].date).unwrap_or_default();
        let senders: Vec<SenderId> = thread.replies.iter().map(|&i| self.messages[i].sender).collect();
        let repliers = recent_repliers(&senders, SUMMARY_AVATARS);

        h_flex()
            .id(("thread", root_id as u64))
            .group(SUMMARY_GROUP)
            .mt_1()
            // The padding is pulled back out, so the avatars line up with the text.
            .ml(px(-6.))
            .max_w(px(520.))
            .gap_2()
            .px(px(5.))
            .py(px(3.))
            .rounded(radius)
            // Always there, so the hover color doesn't change the size.
            .border_1()
            .border_color(transparent_black())
            .cursor_pointer()
            .hover(move |summary| summary.bg(background).border_color(border))
            .on_click(cx.listener(move |this, _, window, cx| this.open_thread_from_row(root_id, window, cx)))
            .child(h_flex().flex_none().gap_0p5().children(repliers.into_iter().map(|sender| {
                let id = sender_id(sender);
                avatar(&self.sender_name(sender), id, px(20.))
            })))
            .child(div().flex_none().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(link).child(label))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .text_color(muted)
                    .child(format!("Last reply {}", format_time(last, today, &self.settings.time))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .invisible()
                    .group_hover(SUMMARY_GROUP, |chevron| chevron.visible())
                    .child(Icon::new(IconName::ChevronRight).small().text_color(muted)),
            )
    }

    fn render_thread_panel(
        &self,
        root_id: i64,
        today: NaiveDate,
        m: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (border, muted) = (cx.theme().border, cx.theme().muted_foreground);
        let chat_title = self.selected_chat.and_then(|id| self.chats.get(&id)).map(|c| c.title.clone());
        let mut messages =
            v_flex().id("thread-messages").flex_1().overflow_y_scroll().track_scroll(&self.thread_scroll).py_2();
        if let Some(thread) = self.threads.iter().find(|t| self.messages[t.root].id == root_id) {
            messages = messages.child(self.render_panel_message(&self.messages[thread.root], false, today, m, cx));
            if !thread.replies.is_empty() {
                let count = thread.replies.len();
                let label = if count == 1 { "1 reply".to_string() } else { format!("{count} replies") };
                messages = messages.child(
                    h_flex()
                        .px_4()
                        .py_2()
                        .gap_3()
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(muted)
                                .child(label),
                        )
                        .child(div().flex_1().h(px(1.)).bg(border)),
                );
                let replies: Vec<&Msg> = thread.replies.iter().map(|&i| &self.messages[i]).collect();
                let facts: Vec<RowFacts> = replies.iter().map(|m| RowFacts::of(m, false, false)).collect();
                let looks = row_looks(&facts, self.settings.chats.group_minutes);
                for (message, look) in replies.into_iter().zip(looks) {
                    messages = messages.child(self.render_panel_message(message, look.continued, today, m, cx));
                }
            }
        }

        v_flex()
            .w(px(self.settings.layout.thread_panel_width as f32))
            .h_full()
            .flex_none()
            .drag_over::<ExternalPaths>(|style, _, _, cx| style.bg(cx.theme().drop_target))
            // Dropped images are sent as replies in the thread.
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.send_files(paths.paths().to_vec(), true, window, cx)
            }))
            .border_l_1()
            .border_color(border)
            .child(
                h_flex()
                    .h(m.header)
                    .flex_none()
                    .pl_4()
                    .pr_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(border)
                    .child(header_title().flex_none().font_weight(FontWeight::SEMIBOLD).child("Thread"))
                    .child(div().flex_1().min_w_0().truncate().text_sm().text_color(muted).children(chat_title))
                    .child(
                        Button::new("close-thread")
                            .ghost()
                            .small()
                            .icon(IconName::Close)
                            .tooltip("Close thread")
                            .on_click(cx.listener(|this, _, window, cx| this.open_thread(None, window, cx))),
                    ),
            )
            .child(messages)
            .child(div().flex_none().px_4().pt_1().pb_3().map(|footer| {
                // A reply to a root that isn't on the server yet would go to
                // the whole chat, so there's no composer until it is.
                match self.thread_root_state() {
                    Some(SendState::Sent) => footer.child(self.render_composer(true, window, cx)),
                    Some(SendState::Pending) => footer.py_2().text_sm().text_color(muted).child("Sending…"),
                    Some(SendState::Failed) | None => footer.py_2().text_sm().text_color(muted).child("Not sent"),
                }
            }))
    }

    fn render_panel_message(
        &self,
        message: &Msg,
        continued: bool,
        today: NaiveDate,
        m: Metrics,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hover_bg = cx.theme().list_hover;
        div()
            .id(("thread-message", message.id as u64))
            .group(PANEL_GROUP)
            .px_4()
            .py(if continued { m.continued_y } else { m.row_y })
            .hover(move |row| row.bg(hover_bg))
            .child(self.render_message(message, continued, PANEL_GROUP, today, m, None, cx))
    }

    /// A message beside its sender's avatar, or, when it continues the
    /// sender's previous message, beside its time, which shows on hover of
    /// `group`. `extra` goes under the text, e.g. a thread summary.
    #[allow(clippy::too_many_arguments)]
    fn render_message(
        &self,
        message: &Msg,
        continued: bool,
        group: &'static str,
        today: NaiveDate,
        m: Metrics,
        extra: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> Div {
        let (muted, danger) = (cx.theme().muted_foreground, cx.theme().danger);
        let failed = message.send_state == SendState::Failed;
        let body_line = rems(BODY_LINE_HEIGHT);
        let not_sent = || {
            h_flex()
                .gap_1()
                .text_xs()
                .text_color(danger)
                .child(Icon::new(IconName::CircleAlert).xsmall())
                .child("Not sent")
        };

        let gutter = if continued {
            div()
                .w(m.avatar)
                .flex_none()
                .text_xs()
                .line_height(body_line)
                .text_color(muted)
                .whitespace_nowrap()
                .flex()
                .justify_center()
                .invisible()
                .group_hover(group, |time| time.visible())
                .child(format_clock(message.date, &self.settings.time))
        } else {
            let name = self.sender_name(message.sender);
            div().flex_none().pt(px(2.)).child(avatar(&name, sender_id(message.sender), m.avatar))
        };

        let mut content = v_flex().flex_1().min_w_0();
        if !continued {
            content = content.child(
                h_flex()
                    .gap_2()
                    .line_height(body_line)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.sender_name(message.sender)),
                    )
                    .child(div().flex_none().text_xs().text_color(muted).child(format_time(
                        message.date,
                        today,
                        &self.settings.time,
                    )))
                    .when(failed, |header| header.child(not_sent())),
            );
        }
        content = content
            // Only colors change, so a message keeps its size when it's sent.
            .when(message.send_state == SendState::Pending, |content| content.opacity(0.55))
            .children(self.render_media(message, cx).map(|media| div().pt_1().pb_0p5().child(media)))
            // A media message's caption can be empty.
            .when(message.media.is_none() || !message.text.is_empty(), |content| {
                content.child(div().line_height(relative(BODY_LINE_HEIGHT)).child(message.text.clone()))
            })
            .when(failed && continued, |content| content.child(not_sent()))
            .children(extra);

        h_flex().items_start().gap_3().child(gutter).child(content)
    }

    /// Opens the thread of a main list row, keeping that row in view while
    /// the list narrows for the panel.
    pub(crate) fn open_thread_from_row(&mut self, root_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        // Narrowing rewraps every row, and the list keeps its top row in
        // place, so taller rows above this one could push it out of view.
        // An open panel keeps its width, so the list doesn't move then.
        if self.open_thread.is_none()
            && let Some(ix) = self.threads.iter().position(|t| self.messages[t.root].id == root_id)
        {
            pin_to_top(&self.message_list, ix);
        }
        self.open_thread(Some(root_id), window, cx);
    }
}

/// Makes row `ix` the list's top row, with its top edge at the list's. The
/// list lays rows out from its top row down, so that row is always laid out
/// and shown, however the rows above it change. Near the end, where the rows
/// from `ix` on don't fill the list, it shows the end instead, `ix` included.
fn pin_to_top(list: &ListState, ix: usize) {
    list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) });
}

/// A header's title, whose line height the header's height accounts for.
fn header_title() -> Div {
    div().text_lg().line_height(relative(HEADER_LINE_HEIGHT))
}

/// A day's centered label on a hairline, above its first message.
fn render_day_separator(label: String, cx: &App) -> Div {
    let theme = cx.theme();
    let line = || div().flex_1().h(px(1.)).bg(theme.border);
    h_flex()
        .w_full()
        .px_4()
        .pt_3()
        .pb_1()
        .gap_3()
        .child(line())
        .child(
            div()
                .flex_none()
                .px_2p5()
                .py_0p5()
                .rounded_full()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(label),
        )
        .child(line())
}

/// A colored circle with the initials of `name`; the color is stable per id.
fn avatar(name: &str, id: i64, size: Pixels) -> Div {
    // Small avatars only have room for one letter.
    let letters = if size >= px(28.) { 2 } else { 1 };
    div()
        .size(size)
        .flex_none()
        .rounded_full()
        .bg(rgb(AVATAR_COLORS[avatar_color_index(id)]))
        .text_color(white())
        .text_size(size * 0.4)
        .font_weight(FontWeight::SEMIBOLD)
        .flex()
        .items_center()
        .justify_center()
        .child(initials(name, letters))
}

fn sender_id(sender: SenderId) -> i64 {
    match sender {
        SenderId::User(id) | SenderId::Chat(id) => id,
    }
}

/// Avatar backgrounds, calm enough for white initials on light and dark themes.
const AVATAR_COLORS: [u32; 7] = [0xd95c66, 0xd9824a, 0x8b6fd6, 0x4fa36a, 0x3ea0ad, 0x4f8bd6, 0xd0628f];

/// Which of [`AVATAR_COLORS`] an id gets. Group ids are negative, and get one too.
fn avatar_color_index(id: i64) -> usize {
    id.rem_euclid(AVATAR_COLORS.len() as i64) as usize
}

/// The first letters of the first `max` words of `name`, in upper case.
fn initials(name: &str, max: usize) -> String {
    name.split_whitespace().filter_map(|word| word.chars().next()).take(max).flat_map(char::to_uppercase).collect()
}

/// An unread count as the chat list shows it.
fn unread_label(unread: i32) -> String {
    if unread > 999 { "999+".into() } else { unread.to_string() }
}

/// What decides how a message row relates to the one above it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RowFacts {
    pub sender: SenderId,
    /// Unix timestamp.
    pub date: i64,
    /// The local day of `date`.
    pub day: NaiveDate,
    /// The row has a thread summary under it, which ends a group.
    pub has_replies: bool,
    /// The row starts with a note that it replies to an unloaded message, so
    /// it shows its sender again.
    pub root_is_reply: bool,
}

impl RowFacts {
    pub(crate) fn of(message: &Msg, has_replies: bool, root_is_reply: bool) -> Self {
        RowFacts {
            sender: message.sender,
            date: message.date.into(),
            day: local_day(message.date),
            has_replies,
            root_is_reply,
        }
    }
}

/// How a message row looks next to the one above it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RowLook {
    /// A day separator goes above it: it's the first row, or the first of its day.
    pub day_break: bool,
    /// It continues the row above, so it shows no name or avatar.
    pub continued: bool,
}

/// How each of `rows` looks. A row continues the one above if both are from
/// the same sender on the same day, at most `group_minutes` apart (0 never
/// groups), and neither a thread summary nor a reply note sits between them.
pub(crate) fn row_looks(rows: &[RowFacts], group_minutes: u32) -> Vec<RowLook> {
    let window = i64::from(group_minutes) * 60;
    let mut looks = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let previous = i.checked_sub(1).map(|i| &rows[i]);
        let day_break = previous.is_none_or(|previous| previous.day != row.day);
        let continued = !day_break
            && group_minutes > 0
            && !row.root_is_reply
            && previous.is_some_and(|previous| {
                previous.sender == row.sender
                    && !previous.has_replies
                    // Sorted by id, so a pending message's date can be earlier.
                    && (row.date - previous.date).abs() <= window
            });
        looks.push(RowLook { day_break, continued });
    }
    looks
}

/// A day separator's label: "Today", "Yesterday", or the day in `[time]`'s
/// format for this year or for other years.
pub(crate) fn day_label(day: NaiveDate, today: NaiveDate, formats: &Time) -> String {
    if day == today {
        "Today".into()
    } else if today.pred_opt() == Some(day) {
        "Yesterday".into()
    } else if day.year() == today.year() {
        // Both formats were checked on startup, so formatting can't panic.
        day.format(&formats.day_format).to_string()
    } else {
        day.format(&formats.other_year_day_format).to_string()
    }
}

/// The distinct senders of `replies` (oldest first), newest first, at most `max`.
fn recent_repliers(replies: &[SenderId], max: usize) -> Vec<SenderId> {
    let mut repliers = Vec::new();
    for &sender in replies.iter().rev() {
        if repliers.len() == max {
            break;
        }
        if !repliers.contains(&sender) {
            repliers.push(sender);
        }
    }
    repliers
}

/// The local day of a Unix timestamp.
pub(crate) fn local_day(timestamp: i32) -> NaiveDate {
    DateTime::from_timestamp(timestamp.into(), 0)
        .map(|time| time.with_timezone(&Local).date_naive())
        .unwrap_or_default()
}

/// A message time in `[time]`'s format for today's or for older messages.
fn format_time(timestamp: i32, today: NaiveDate, formats: &Time) -> String {
    let Some(time) = DateTime::from_timestamp(timestamp.into(), 0) else { return String::new() };
    let time = time.with_timezone(&Local);
    // Both formats were checked on startup, so formatting can't panic.
    let format = if time.date_naive() == today { &formats.today_format } else { &formats.older_format };
    time.format(format).to_string()
}

/// The time of day of a continued message, in today's format: the day
/// separator above already says which day it is.
fn format_clock(timestamp: i32, formats: &Time) -> String {
    let Some(time) = DateTime::from_timestamp(timestamp.into(), 0) else { return String::new() };
    time.with_timezone(&Local).format(&formats.today_format).to_string()
}

#[cfg(test)]
mod tests {
    use chrono::{Days, Local, NaiveDate};
    use frillogram_core::{
        SenderId,
        config::{Appearance, Density, Time},
    };
    use gpui_kit::{ListAlignment, ListOffset, ListState, px};

    use super::{
        AVATAR_COLORS, BADGE_HEIGHT, CHAT_LINE_HEIGHT, HEADER_LINE_HEIGHT, Metrics, RowFacts, RowLook, TEXT_LG,
        avatar_color_index, day_label, format_time, initials, pin_to_top, recent_repliers, row_looks, unread_label,
    };

    #[test]
    fn headers_fit_their_titles_and_chat_titles_their_badges() {
        for (density, avatar_size, font_size) in [
            (Density::Compact, 20, 24),
            (Density::Comfortable, 20, 30),
            (Density::Compact, 64, 10),
            (Density::Comfortable, 36, 12),
        ] {
            let m = Metrics::new(&Appearance { density, avatar_size, font_size, ..Appearance::default() });
            let rem = px(font_size as f32);
            let case = format!("{density:?} {avatar_size} {font_size}");
            // The compact padding is the smaller one.
            assert!(m.header >= rem * (TEXT_LG * HEADER_LINE_HEIGHT) + px(12.), "{case}");
            assert!(m.header >= m.avatar + px(12.), "{case}");
            // The thread header's close button.
            assert!(m.header >= rem * 1.5 + px(12.), "{case}");
            assert!(m.chat_title >= BADGE_HEIGHT && m.chat_title >= rem * CHAT_LINE_HEIGHT, "{case}");
        }
        // The defaults' header fits the avatar, as before.
        assert_eq!(Metrics::new(&Appearance::default()).header, px(56.));
    }

    #[test]
    fn a_pinned_row_stays_the_top_row_while_the_list_remeasures() {
        let list = ListState::new(10, ListAlignment::Bottom, px(600.));
        // Scrolled up in history, partway into row 3.
        list.scroll_to(ListOffset { item_ix: 3, offset_in_item: px(40.) });
        pin_to_top(&list, 7);
        list.remeasure();
        assert_eq!(list.logical_scroll_top(), ListOffset { item_ix: 7, offset_in_item: px(0.) });
    }

    #[test]
    fn times_use_the_configured_formats() {
        let formats = Time { today_format: "today %Y".into(), older_format: "older %Y".into(), ..Time::default() };
        let now = Local::now();
        let year = now.format("%Y");
        let timestamp = now.timestamp() as i32;
        assert_eq!(format_time(timestamp, now.date_naive(), &formats), format!("today {year}"));
        let tomorrow = now.date_naive().checked_add_days(Days::new(1)).unwrap();
        assert_eq!(format_time(timestamp, tomorrow, &formats), format!("older {year}"));
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn day_labels_name_recent_days_and_add_the_year_only_when_it_differs() {
        let formats = Time::default();
        let today = day(2026, 11, 3);
        assert_eq!(day_label(today, today, &formats), "Today");
        assert_eq!(day_label(day(2026, 11, 2), today, &formats), "Yesterday");
        assert_eq!(day_label(day(2026, 11, 1), today, &formats), "Sunday, November 1");
        assert_eq!(day_label(day(2025, 12, 30), today, &formats), "Tuesday, December 30, 2025");
        // Yesterday across a year boundary is still "Yesterday".
        let new_year = day(2027, 1, 1);
        assert_eq!(day_label(day(2026, 12, 31), new_year, &formats), "Yesterday");
        assert_eq!(day_label(day(2026, 12, 30), new_year, &formats), "Wednesday, December 30, 2026");
        // A day after today, from a skewed clock, gets a date too.
        assert_eq!(day_label(day(2026, 11, 4), today, &formats), "Wednesday, November 4");
        let custom = Time { day_format: "%d.%m.".into(), other_year_day_format: "%d.%m.%Y".into(), ..Time::default() };
        assert_eq!(day_label(day(2026, 10, 1), today, &custom), "01.10.");
        assert_eq!(day_label(day(2024, 10, 1), today, &custom), "01.10.2024");
    }

    const ADA: SenderId = SenderId::User(1);
    const BOB: SenderId = SenderId::User(2);

    fn row(sender: SenderId, minute: i64, day: NaiveDate) -> RowFacts {
        RowFacts { sender, date: minute * 60, day, has_replies: false, root_is_reply: false }
    }

    fn looks(rows: &[RowFacts], group_minutes: u32) -> Vec<(bool, bool)> {
        row_looks(rows, group_minutes)
            .into_iter()
            .map(|RowLook { day_break, continued }| (day_break, continued))
            .collect()
    }

    #[test]
    fn messages_from_one_sender_within_the_window_group() {
        let d = day(2026, 11, 3);
        let rows = [row(ADA, 0, d), row(ADA, 3, d), row(ADA, 8, d), row(BOB, 9, d), row(ADA, 10, d), row(ADA, 16, d)];
        // 8 is 5 minutes after 3, so it still groups; 16 is 6 after 10.
        assert_eq!(
            looks(&rows, 5),
            [(true, false), (false, true), (false, true), (false, false), (false, false), (false, false)]
        );
    }

    #[test]
    fn zero_minutes_turns_grouping_off() {
        let d = day(2026, 11, 3);
        assert_eq!(looks(&[row(ADA, 0, d), row(ADA, 0, d)], 0), [(true, false), (false, false)]);
    }

    #[test]
    fn a_new_day_breaks_a_group_and_gets_a_separator() {
        let rows = [row(ADA, 0, day(2026, 11, 2)), row(ADA, 1, day(2026, 11, 3)), row(ADA, 2, day(2026, 11, 3))];
        assert_eq!(looks(&rows, 5), [(true, false), (true, false), (false, true)]);
    }

    #[test]
    fn thread_summaries_and_reply_notes_break_a_group() {
        let d = day(2026, 11, 3);
        let root = RowFacts { has_replies: true, ..row(ADA, 0, d) };
        let reply_note = RowFacts { root_is_reply: true, ..row(ADA, 2, d) };
        // A thread root can itself continue a group; what comes after it can't.
        assert_eq!(looks(&[row(ADA, 0, d), root, row(ADA, 1, d)], 5), [(true, false), (false, true), (false, false)]);
        assert_eq!(
            looks(&[row(ADA, 0, d), reply_note, row(ADA, 3, d)], 5),
            [(true, false), (false, false), (false, true)]
        );
    }

    #[test]
    fn an_earlier_pending_date_still_groups() {
        let d = day(2026, 11, 3);
        assert_eq!(looks(&[row(ADA, 10, d), row(ADA, 9, d)], 5), [(true, false), (false, true)]);
        assert!(looks(&[], 5).is_empty());
    }

    #[test]
    fn repliers_are_distinct_newest_first_and_capped() {
        let carol = SenderId::User(3);
        let dave = SenderId::Chat(-4);
        assert_eq!(recent_repliers(&[ADA, BOB, ADA, carol, ADA, dave], 3), vec![dave, ADA, carol]);
        assert_eq!(recent_repliers(&[ADA, ADA], 3), vec![ADA]);
        assert!(recent_repliers(&[], 3).is_empty());
        assert!(recent_repliers(&[ADA], 0).is_empty());
    }

    #[test]
    fn avatar_colors_are_stable_and_cover_negative_ids() {
        for id in [0, 1, 6, 7, -1, -7, -1001234567890, i64::MIN, i64::MAX] {
            let index = avatar_color_index(id);
            assert!(index < AVATAR_COLORS.len(), "{id}");
            assert_eq!(index, avatar_color_index(id));
        }
        assert_eq!(avatar_color_index(7), avatar_color_index(0));
        assert_ne!(avatar_color_index(1), avatar_color_index(2));
        assert_eq!(avatar_color_index(-1), AVATAR_COLORS.len() - 1);
    }

    #[test]
    fn initials_take_the_first_letters_of_the_first_words() {
        assert_eq!(initials("Ada Lovelace", 2), "AL");
        assert_eq!(initials("  ada   b. lovelace ", 2), "AB");
        assert_eq!(initials("ünal", 2), "Ü");
        assert_eq!(initials("Ada Lovelace", 1), "A");
        assert_eq!(initials("", 2), "");
    }

    #[test]
    fn large_unread_counts_are_capped() {
        assert_eq!(unread_label(7), "7");
        assert_eq!(unread_label(999), "999");
        assert_eq!(unread_label(1000), "999+");
    }
}
