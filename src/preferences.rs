// SPDX-License-Identifier: GPL-3.0-or-later
use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{glib, CompositeTemplate};

use crate::application::Application;
use crate::i18n::tr;

mod imp {
    use super::*;

    #[derive(CompositeTemplate, Default)]
    #[template(resource = "/digital/fischers/stempeluhr/ui/preferences-dialog.ui")]
    pub struct PreferencesDialog {
        #[template_child]
        pub server_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub change_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub disconnect_row: TemplateChild<adw::ButtonRow>,
        #[template_child]
        pub start_behavior_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub global_shortcut_row: TemplateChild<adw::SwitchRow>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PreferencesDialog {
        const NAME: &'static str = "StempeluhrPreferencesDialog";
        type Type = super::PreferencesDialog;
        type ParentType = adw::PreferencesDialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PreferencesDialog {}
    impl WidgetImpl for PreferencesDialog {}
    impl AdwDialogImpl for PreferencesDialog {}
    impl PreferencesDialogImpl for PreferencesDialog {}
}

glib::wrapper! {
    pub struct PreferencesDialog(ObjectSubclass<imp::PreferencesDialog>)
        @extends adw::PreferencesDialog, adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PreferencesDialog {
    pub fn new(app: &Application) -> Self {
        let dialog: Self = glib::Object::new();
        let imp = dialog.imp();
        let settings = app.settings();

        let url = settings.string("server-url");
        let connected = !url.is_empty();
        imp.server_row.set_subtitle(&if connected {
            url.to_string()
        } else {
            tr("Not connected")
        });
        imp.disconnect_row.set_visible(connected);

        // Change the connection.
        let this = dialog.downgrade();
        let a = app.clone();
        imp.change_button.connect_clicked(move |_| {
            if let Some(d) = this.upgrade() {
                d.close();
            }
            a.activate_action("connect", None);
        });

        // Disconnect, after confirmation.
        let this = dialog.downgrade();
        let a = app.clone();
        imp.disconnect_row.connect_activated(move |_| {
            let (Some(d), a) = (this.upgrade(), a.clone()) else { return };
            glib::spawn_future_local(async move {
                let confirm = adw::AlertDialog::new(
                    Some(&tr("Disconnect from Kimai?")),
                    Some(&tr("The access token is removed from this device. Your time tracking in Kimai is not affected.")),
                );
                confirm.add_responses(&[("cancel", &tr("_Cancel")), ("disconnect", &tr("_Disconnect"))]);
                confirm.set_response_appearance("disconnect", adw::ResponseAppearance::Destructive);
                confirm.set_default_response(Some("cancel"));
                confirm.set_close_response("cancel");
                if confirm.choose_future(Some(&d)).await == "disconnect" {
                    a.disconnect().await;
                    d.close();
                }
            });
        });

        // Start behavior: "last" | "window".
        imp.start_behavior_row
            .set_selected(if settings.string("start-behavior") == "window" {
                1
            } else {
                0
            });
        let s = settings.clone();
        imp.start_behavior_row.connect_selected_notify(move |row| {
            let value = if row.selected() == 1 {
                "window"
            } else {
                "last"
            };
            if let Err(e) = s.set_string("start-behavior", value) {
                tracing::warn!("cannot save setting: {e}");
            }
        });

        // Optional global shortcut.
        imp.global_shortcut_row
            .set_active(settings.boolean("global-shortcut-enabled"));
        let s = settings.clone();
        let a = app.clone();
        imp.global_shortcut_row.connect_active_notify(move |row| {
            let on = row.is_active();
            if let Err(e) = s.set_boolean("global-shortcut-enabled", on) {
                tracing::warn!("cannot save setting: {e}");
            }
            a.set_global_shortcut(on);
        });

        dialog
    }
}
