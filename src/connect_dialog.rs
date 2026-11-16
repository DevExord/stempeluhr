// SPDX-License-Identifier: GPL-3.0-or-later
//! Onboarding / sign-in dialog. The connection is tested before anything is
//! saved.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{glib, CompositeTemplate};
use stempeluhr_core::kimai::{normalize_base_url, Client, ErrorKind, Token, UrlError};

use crate::application::Application;
use crate::i18n::{tr, tr_f};
use crate::{messages, runtime, secrets};

mod imp {
    use super::*;

    #[derive(CompositeTemplate, Default)]
    #[template(resource = "/digital/fischers/stempeluhr/ui/connect-dialog.ui")]
    pub struct ConnectDialog {
        #[template_child]
        pub url_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub token_row: TemplateChild<adw::PasswordEntryRow>,
        #[template_child]
        pub status_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub connect_button: TemplateChild<gtk::Button>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ConnectDialog {
        const NAME: &'static str = "StempeluhrConnectDialog";
        type Type = super::ConnectDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ConnectDialog {}
    impl WidgetImpl for ConnectDialog {}
    impl AdwDialogImpl for ConnectDialog {}
}

glib::wrapper! {
    pub struct ConnectDialog(ObjectSubclass<imp::ConnectDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ConnectDialog {
    pub fn new(app: &Application) -> Self {
        let dialog: Self = glib::Object::new();
        let imp = dialog.imp();
        imp.url_row.set_text(&app.settings().string("server-url"));
        let this = dialog.downgrade();
        let app = app.clone();
        imp.connect_button.connect_clicked(move |_| {
            if let Some(d) = this.upgrade() {
                d.connect(&app);
            }
        });
        dialog
    }

    fn show_status(&self, msg: &str, is_error: bool) {
        let label = &self.imp().status_label;
        label.set_label(msg);
        label.set_visible(!msg.is_empty());
        if is_error {
            label.add_css_class("error");
        } else {
            label.remove_css_class("error");
        }
    }

    fn set_busy(&self, busy: bool) {
        let imp = self.imp();
        imp.url_row.set_sensitive(!busy);
        imp.token_row.set_sensitive(!busy);
        imp.connect_button.set_sensitive(!busy);
        imp.connect_button.set_label(&if busy {
            tr("Connecting…")
        } else {
            tr("Connect")
        });
    }

    fn connect(&self, app: &Application) {
        let imp = self.imp();
        let url = match normalize_base_url(&imp.url_row.text()) {
            Ok(u) => u,
            Err(e) => {
                self.show_status(
                    &match e {
                        UrlError::Empty => tr("Please enter the address of your Kimai server."),
                        UrlError::UnsupportedScheme => tr("The address must start with https:// or http://."),
                        UrlError::HasCredentials => tr("Don’t put a user name or password in the address. Use an API token instead."),
                        UrlError::Invalid => tr("This doesn’t look like a valid address."),
                    },
                    true,
                );
                return;
            }
        };
        let token = imp.token_row.text().trim().to_string();
        if token.is_empty() {
            self.show_status(&tr("Please enter your API token."), true);
            return;
        }
        imp.url_row.set_text(&url);
        self.show_status("", false);
        self.set_busy(true);

        let this = self.clone();
        let app = app.clone();
        glib::spawn_future_local(async move {
            let (test_client, app_client) = match (
                Client::new(&url, Token::new(token.clone())),
                Client::new(&url, Token::new(token.clone())),
            ) {
                (Ok(a), Ok(b)) => (a, b),
                _ => {
                    this.set_busy(false);
                    this.show_status(
                        &messages::describe(&stempeluhr_core::kimai::Error::new(
                            ErrorKind::Other,
                            "client",
                        )),
                        true,
                    );
                    return;
                }
            };
            let result =
                runtime::spawn_result(async move { test_client.test_connection().await }).await;
            match result {
                Err(e) => {
                    this.set_busy(false);
                    this.show_status(&messages::describe(&e), true);
                }
                Ok((_version, user)) => {
                    if let Err(e) = secrets::store(token).await {
                        tracing::warn!(error = %e, "cannot store token");
                        this.set_busy(false);
                        this.show_status(
                            &tr("The token couldn’t be saved to the system keyring, so it can’t be kept safely."),
                            true,
                        );
                        return;
                    }
                    app.set_connection(&url, app_client);
                    this.close();
                    if let Some(win) = app.main_window() {
                        win.toast(&tr_f("Connected as {user}", &[("user", &user.username)]));
                    }
                }
            }
        });
    }
}
