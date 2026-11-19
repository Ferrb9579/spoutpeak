//! The Account page: sign in or create an account, or see who is signed in.

use super::motion;
use super::widgets::{PAGE_PADDING, page, smooth};
use crate::account::{AuthMode, transport_is_safe};
use crate::state::App;
use crate::theme::*;
use incular::prelude::*;

const FORM_WIDTH: f32 = 420.0;
const FIELD_WIDTH: f32 = FORM_WIDTH - 40.0;

fn field(label: &str, control: Widget) -> Widget {
    col(vec![text(label, 12.5, FontWeight::W600, MUTED).into(), gap(6.0), control]).into()
}

fn avatar(name: &str, size: f32) -> Widget {
    let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    Container::new()
        .width(size)
        .height(size)
        .radius(size / 2.0)
        .color(ACCENT_DIM)
        .alignment(Alignment::CENTER)
        .child(text(initial, size * 0.42, FontWeight::W700, TEXT))
        .into()
}

fn signed_in(app: &App, account: crate::account::Account) -> Widget {
    let out = app.clone();
    card(
        col(vec![
            hrow(vec![
                avatar(&account.name, 56.0),
                hgap(16.0),
                Expanded::new(col(vec![
                    text(account.name.clone(), 19.0, FontWeight::W700, TEXT).into(),
                    gap(3.0),
                    text(account.email.clone(), 13.0, FontWeight::W400, MUTED).into(),
                ]))
                .into(),
            ])
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .into(),
            gap(18.0),
            muted("Watch-together rooms show you by this name."),
            gap(18.0),
            icon_text_button(icons::log_in(), "Sign out", Tone::Neutral, move || out.sign_out()),
        ])
        .cross_axis_alignment(CrossAxisAlignment::Start),
    )
    .width(FORM_WIDTH)
    .into()
}

fn signed_out(app: &App) -> Widget {
    let state = app.auth.read(|a| a.clone());
    let signing_up = state.mode == AuthMode::SignUp;
    let submit = {
        let app = app.clone();
        move || if signing_up { app.sign_up() } else { app.sign_in() }
    };

    let (to_in, to_up) = (app.clone(), app.clone());
    let mut children: Vec<Widget> = vec![
        text(if signing_up { "Create your account" } else { "Sign in" }, 20.0, FontWeight::W700, TEXT).into(),
        gap(4.0),
        muted(if signing_up {
            "An account lets you watch together with friends."
        } else {
            "Welcome back. Sign in to watch together."
        }),
        gap(16.0),
        row(vec![
            chip("Sign in", !signing_up, move || to_in.set_auth_mode(AuthMode::SignIn)),
            hgap(8.0),
            chip("Create account", signing_up, move || to_up.set_auth_mode(AuthMode::SignUp)),
        ])
        .into(),
        gap(20.0),
    ];
    let enter = submit.clone();
    children.push(field(
        "Email",
        input(&app.controllers.account_email, "you@example.com", FIELD_WIDTH, move |_| enter()).into(),
    ));
    children.push(gap(14.0));
    if signing_up {
        let enter = submit.clone();
        children.push(field(
            "Name",
            input(&app.controllers.account_name, "What others see", FIELD_WIDTH, move |_| enter()).into(),
        ));
        children.push(gap(14.0));
    }
    let enter = submit.clone();
    children.push(field(
        "Password",
        password_input(
            &app.controllers.account_password,
            if signing_up { "At least 8 characters" } else { "Your password" },
            FIELD_WIDTH,
            move |_| enter(),
        ),
    ));
    if let Some(error) = &state.error {
        children.push(gap(12.0));
        children.push(
            SizedBox::new()
                .width(FIELD_WIDTH)
                .child(text(error.clone(), 12.5, FontWeight::W500, DANGER))
                .into(),
        );
    }
    children.push(gap(20.0));
    children.push(if state.busy {
        super::widgets::loading_line(if signing_up { "Creating your account…" } else { "Signing in…" })
    } else {
        button(if signing_up { "Create account" } else { "Sign in" }, Tone::Primary, submit)
    });
    if !transport_is_safe() {
        children.push(gap(16.0));
        children.push(
            SizedBox::new()
                .width(FIELD_WIDTH)
                .child(text(
                    "This build's server address isn't HTTPS, so signing in is turned off to keep \
                     your password from being sent unencrypted.",
                    12.0,
                    FontWeight::W400,
                    FAINT,
                ))
                .into(),
        );
    }
    card(col(children).cross_axis_alignment(CrossAxisAlignment::Start)).width(FORM_WIDTH).into()
}

pub fn screen(app: &App) -> Widget {
    let body = match app.account() {
        Some(account) => signed_in(app, account),
        None => signed_out(app),
    };
    page(
        "Account",
        vec![],
        smooth(
            &app.controllers.account,
            SingleChildScrollView::new(Padding::new(PAGE_PADDING, body))
                .physics(motion::APP_SCROLLED)
                .controller(app.controllers.account.clone()),
        ),
    )
}
