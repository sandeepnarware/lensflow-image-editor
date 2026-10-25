//! The pre-editor screens, driven headless through their accessibility tree.

use eframe::egui::accesskit::Role;
use eframe::egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use lf_account::LockReason;
use lf_app::screens::{self, Action, View};
use lf_app::session::{PAUSED_MESSAGE, Session};

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
