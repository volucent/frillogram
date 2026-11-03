use frillogram_core::{AuthStep, ConfigError};
use gpui_kit::{
    component::{
        ActiveTheme, Disableable,
        button::{Button, ButtonVariants},
        input::{Input, InputState},
        v_flex,
    },
    *,
};

use crate::app::FrillogramApp;

/// The launch command the setup screen suggests; SETUP.md has the other ways.
const RUN_WITH_PASS: &str =
    "FRILLOGRAM_API_ID=$(pass frillogram/api_id) FRILLOGRAM_API_HASH=$(pass frillogram/api_hash) cargo run";

impl FrillogramApp {
    pub(crate) fn render_setup(&self, error: &ConfigError, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let card = card(cx)
            .child(div().text_xl().font_weight(FontWeight::BOLD).child("Frillogram can't start"))
            .child(div().text_color(cx.theme().danger).child(error.to_string()));
        let card = match error {
            ConfigError::Credentials(_) => card
                .child(div().text_color(muted).child(
                    "Create an app at my.telegram.org → API development tools, then start Frillogram with the \
                     credentials set, for example from pass:",
                ))
                .child(
                    div()
                        .p_3()
                        .rounded_md()
                        .bg(cx.theme().muted)
                        .font_family("monospace")
                        .text_sm()
                        .child(RUN_WITH_PASS),
                )
                .child(div().text_color(muted).child(
                    "SETUP.md also covers a private .env file. Avoid typing the hash itself on the command line, \
                     where it stays in your shell history.",
                )),
            ConfigError::DataDir(_) => card.child(
                div()
                    .text_color(muted)
                    .child("SETUP.md’s “Where data is kept” explains where Frillogram keeps its data."),
            ),
        };
        centered(card)
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

        let mut card = card(cx)
            .child(div().text_xl().font_weight(FontWeight::BOLD).child(title))
            .child(div().text_color(muted).child(hint));
        if let Some(input) = input {
            card = card.child(Input::new(input)).child(
                Button::new("login-next")
                    .primary()
                    .label("Next")
                    .loading(self.busy)
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, window, cx| this.submit_login(window, cx))),
            );
        }
        if let Some(error) = &self.error {
            card = card.child(div().text_color(cx.theme().danger).child(error.clone()));
        }
        centered(card)
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
    v_flex().w(px(400.)).gap_3().p_6().rounded_lg().border_1().border_color(cx.theme().border).bg(cx.theme().secondary)
}

fn centered(content: Div) -> Div {
    div().size_full().flex().items_center().justify_center().child(content)
}
