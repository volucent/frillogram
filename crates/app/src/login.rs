use frillogram_core::AuthStep;
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

impl FrillogramApp {
    pub(crate) fn render_setup(&self, error: String, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        centered(
            card(cx)
                .child(div().text_xl().font_weight(FontWeight::BOLD).child("Frillogram needs API credentials"))
                .child(div().text_color(cx.theme().danger).child(error))
                .child(
                    div()
                        .text_color(muted)
                        .child("Create an app at my.telegram.org → API development tools, then start Frillogram with:"),
                )
                .child(
                    div()
                        .p_3()
                        .rounded_md()
                        .bg(cx.theme().muted)
                        .font_family("monospace")
                        .text_sm()
                        .child("FRILLOGRAM_API_ID=12345 FRILLOGRAM_API_HASH=abc… cargo run"),
                ),
        )
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
