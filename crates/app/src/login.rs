use frillogram_core::{AuthStep, ConfigError};
use gpui_kit::{
    assets::IconName as Lucide,
    component::{
        ActiveTheme, Disableable, Icon, IconName, Sizable,
        button::{Button, ButtonVariants},
        input::{Input, InputState},
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};

use crate::{app::FrillogramApp, chat::drags_window};

/// The `[telegram]` section the setup screen suggests; SETUP.md has the other ways.
const TELEGRAM_SECTION: &str = "[telegram]\napi_id = 12345\napi_hash_command = \"pass frillogram/api_hash\"";

impl FrillogramApp {
    pub(crate) fn render_setup(&self, error: &ConfigError, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        // Messages span several lines, e.g. the file, then what's wrong in it.
        let message = v_flex().text_color(cx.theme().danger).children(error.to_string().lines().map(str::to_string));
        let card = card(cx)
            .w(px(560.))
            .child(mark(IconName::CircleAlert, cx.theme().danger.opacity(0.12), cx.theme().danger))
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child("Frillogram can't start"))
            .child(message);
        let card = match error {
            ConfigError::File(_) => card.child(div().text_color(muted).child(
                "Fix the file and start Frillogram again. `frillogram --print-default-config` prints every key with \
                 its default and allowed values, and SETUP.md’s “Configuration” explains them.",
            )),
            ConfigError::Credentials(_) => card
                .child(div().text_color(muted).child(
                    "Create an app at my.telegram.org → API development tools, then add its credentials to the \
                     config file, for example with the api_hash kept in pass:",
                ))
                .child(
                    v_flex()
                        .p_3()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().muted)
                        .font_family("monospace")
                        .text_sm()
                        .children(TELEGRAM_SECTION.lines()),
                )
                .child(div().text_color(muted).child(
                    "SETUP.md also covers environment variables and a private .env file. Never put the hash itself \
                     in the config file or on the command line.",
                )),
            ConfigError::DataDir(_) => card.child(
                div()
                    .text_color(muted)
                    .child("SETUP.md’s “Where data is kept” explains where Frillogram keeps its data."),
            ),
        };
        centered(card, cx)
    }

    pub(crate) fn render_login(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let (title, hint, input) = match &self.auth {
            AuthStep::Phone => {
                ("Log in to Telegram", "Your phone number, with country code.".to_string(), Some(&self.phone_input))
            }
            AuthStep::Code { hint } => ("Enter the code", hint.clone(), Some(&self.code_input)),
            AuthStep::Password { hint } => {
                let hint =
                    if hint.is_empty() { "Two-step verification is on.".into() } else { format!("Hint: {hint}") };
                ("Enter your password", hint, Some(&self.password_input))
            }
            AuthStep::Unsupported(reason) => ("Can't log in here yet", reason.clone(), None),
            AuthStep::Starting | AuthStep::Ready => ("Connecting…", String::new(), None),
            AuthStep::Closed => ("Disconnected", "Restart Frillogram to reconnect.".into(), None),
        };

        let mut card =
            card(cx).child(mark(Lucide::MessagesSquare, cx.theme().primary, cx.theme().primary_foreground)).child(
                v_flex()
                    .gap_1()
                    .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child(title))
                    .when(!hint.is_empty(), |text| text.child(div().text_color(muted).child(hint))),
            );
        if let Some(input) = input {
            card = card.child(Input::new(input)).child(
                Button::new("login-next")
                    .primary()
                    .w_full()
                    .label("Next")
                    .loading(self.busy)
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, window, cx| this.submit_login(window, cx))),
            );
        }
        if let Some(error) = self.error.text() {
            card = card.child(div().text_sm().text_color(cx.theme().danger).child(error));
        }
        centered(card, cx)
    }

    pub(crate) fn focus_login_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.login_input(&self.auth) else { return };
        input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// The input that answers `step`, if it asks for one.
    pub(crate) fn login_input(&self, step: &AuthStep) -> Option<&Entity<InputState>> {
        match step {
            AuthStep::Phone => Some(&self.phone_input),
            AuthStep::Code { .. } => Some(&self.code_input),
            AuthStep::Password { .. } => Some(&self.password_input),
            _ => None,
        }
    }
}

fn card(cx: &mut Context<FrillogramApp>) -> Div {
    let theme = cx.theme();
    v_flex()
        .w(px(420.))
        .gap_5()
        .p_10()
        .rounded(theme.radius_lg * 2.)
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .text_color(theme.popover_foreground)
        .shadow_md()
}

/// A rounded square with `icon` at the top of a card.
fn mark(icon: impl Into<Icon>, background: Hsla, foreground: Hsla) -> Div {
    div()
        .size_12()
        .rounded_xl()
        .bg(background)
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::new(icon).with_size(px(24.)).text_color(foreground))
}

/// `content` in the middle of the window, on the sidebar's color so the card
/// stands out. A strip along the top moves the window, as a title bar would.
fn centered(content: Div, cx: &mut Context<FrillogramApp>) -> Div {
    div()
        .relative()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(cx.theme().sidebar)
        .child(content)
        .child(div().id("drag-strip").absolute().top_0().left_0().right_0().h_10().map(|area| drags_window(area, cx)))
}
