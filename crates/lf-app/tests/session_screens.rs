//! The pre-editor screens, driven headless through their accessibility tree.

use eframe::egui::accesskit::Role;
use eframe::egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use lf_account::LockReason;
use lf_account::entitlement::{Entitlement, Release, Standing};
use lf_app::screens::{self, Action, View};
use lf_app::session::{PAUSED_MESSAGE, READ_ONLY_BANNER, Session};

struct Screen {
    session: Session,
    view: View,
    actions: Vec<Action>,
}

fn harness(session: Session) -> Harness<'static, Screen> {
    let state = Screen {
        session,
        view: View::default(),
        actions: Vec::new(),
    };
    Harness::new_ui_state(
        |ui, s: &mut Screen| {
            if let Some(action) = screens::show(ui, &mut s.session, &s.view) {
                s.actions.push(action);
            }
        },
        state,
    )
}

#[test]
fn sign_in_screen_has_email_password_and_button() {
    let mut h = harness(Session::signing_in());
    h.run();
    h.get_by_role_and_label(Role::TextInput, "Email");
    let password = h.get_by_label("Password");
    assert_eq!(password.accesskit_node().role(), Role::PasswordInput);
    h.get_by_role_and_label(Role::Button, "Sign in").click();
    h.run();
    assert_eq!(h.state().actions, [Action::SignIn]);
    assert!(
        h.query_by_label("Code from your authenticator app")
            .is_none()
    );
}

#[test]
fn two_factor_step_shows_code_field() {
    let mut session = Session::signing_in();
    if let Session::SigningIn { two_factor, .. } = &mut session {
        *two_factor = true;
    }
    let mut h = harness(session);
    h.run();
    h.get_by_label("Code from your authenticator app");
    assert!(h.query_by_label("Password").is_none());
    h.get_by_role_and_label(Role::Button, "Sign in");
}

#[test]
fn sign_in_errors_show_under_the_form_and_dev_switch_is_hidden() {
    let mut session = Session::signing_in();
    if let Session::SigningIn { error, .. } = &mut session {
        *error = Some("The email or password is not right.".into());
    }
    let mut h = harness(session);
    h.run();
    h.get_by_label("The email or password is not right.");
    assert!(h.query_by_label("Using the development server.").is_none());

    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::D);
    h.run();
    assert_eq!(h.state().actions, [Action::ToggleInstance]);
    h.state_mut().view.dev_server = true;
    h.run();
    h.get_by_label("Using the development server.");
}

#[test]
fn locked_paused_screen_shows_message() {
    let mut h = harness(Session::Locked {
        reason: LockReason::Paused,
        message: PAUSED_MESSAGE.into(),
    });
    h.run();
    h.get_by_label("Your studio is paused. Resume your LensFlow plan to use lensflow-ImageEditor.");
    h.get_by_role_and_label(Role::Button, "Try again");
    h.get_by_role_and_label(Role::Button, "Sign out");
    assert!(h.query_by_label("Switch studio").is_none());
}

#[test]
fn chooser_lists_studios_with_roles() {
    let user = serde_json::from_value(serde_json::json!({
        "id": 1, "name": "Asha", "email": "a@example.com",
        "studios": [
            {"id": 1, "name": "Frames", "slug": "frames", "role": "owner", "permissions": ["galleries.manage"]},
            {"id": 2, "name": "Lights", "slug": "lights", "role": "editor", "permissions": []}
        ]
    }))
    .unwrap();
    let mut h = harness(Session::ChoosingStudio { user });
    h.run();
    h.get_by_label("owner");
    h.get_by_label("editor");
    h.get_by_role_and_label(Role::Button, "Lights").click();
    h.run();
    assert_eq!(h.state().actions, [Action::ChooseStudio("lights".into())]);
}

fn active(read_only: bool, update: Option<&str>) -> Session {
    let studio = serde_json::from_value(serde_json::json!({
        "id": 1, "name": "Frames", "slug": "frames", "role": "owner",
        "permissions": ["galleries.manage"]
    }))
    .unwrap();
    let standing = if read_only {
        Standing::ReadOnly
    } else {
        Standing::Active
    };
    Session::Active {
        studio,
        entitlement: Entitlement::example(standing, true),
        stale: None,
        read_only,
        update: update.map(|version| Release {
            version: version.into(),
            notes: None,
            published_at: None,
        }),
    }
}

fn chrome(session: Session, can_switch: bool) -> Harness<'static, Screen> {
    let state = Screen {
        session,
        view: View {
            can_switch,
            ..View::default()
        },
        actions: Vec::new(),
    };
    Harness::new_ui_state(
        |ui, s: &mut Screen| {
            let top = screens::active_top(ui, &s.session, &s.view);
            let status = screens::status_bar(ui, &s.session, &s.view);
            if let Some(action) = top.or(status) {
                s.actions.push(action);
            }
        },
        state,
    )
}

#[test]
fn account_menu_offers_switch_studio_and_sign_out() {
    let mut h = chrome(active(false, None), true);
    h.run();
    h.get_by_label("Account").click();
    h.run();
    h.get_by_role_and_label(Role::Button, "Sign out");
    h.get_by_role_and_label(Role::Button, "Switch studio")
        .click();
    h.run();
    assert_eq!(h.state().actions, [Action::SwitchStudio]);

    let mut h = chrome(active(false, None), false);
    h.run();
    h.get_by_label("Account").click();
    h.run();
    assert!(
        h.query_by_label("Switch studio").is_none(),
        "one studio: nothing to switch to"
    );
    h.get_by_role_and_label(Role::Button, "Sign out").click();
    h.run();
    assert_eq!(h.state().actions, [Action::SignOut]);
}

#[test]
fn status_bar_shows_standing_last_checked_and_the_update_offer() {
    let mut h = chrome(active(false, Some("1.3.0")), false);
    h.run();
    h.get_by_label("Active");
    h.get_by_label("Last checked just now");
    assert!(h.query_by_label(READ_ONLY_BANNER).is_none());
    h.get_by_label("Version 1.3.0 is ready — Update").click();
    h.run();
    assert_eq!(h.state().actions, [Action::Update]);
}

#[test]
fn read_only_shows_the_banner() {
    let mut h = chrome(active(true, None), false);
    h.run();
    h.get_by_label("Your LensFlow plan has lapsed: editing works, but preset sync and AI are off until you renew.");
    h.get_by_label("Read-only");
    assert!(h.query_by_label_contains("is ready").is_none());
}

#[test]
fn the_server_switch_is_ignored_while_signing_in() {
    let mut session = Session::signing_in();
    if let Session::SigningIn { busy, .. } = &mut session {
        *busy = true;
    }
    // A spinner keeps repainting, so step instead of running to rest.
    let mut h = harness(session);
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::D);
    h.run_steps(2);
    assert!(h.state().actions.is_empty());
}
