use adw::{prelude::*, subclass::prelude::*};
use gettextrs::gettext;
use gtk::{
    glib::{self, clone},
    CompositeTemplate,
};
use log::error;
use matrix_sdk::ruma::{api::client::account::deactivate, assign};

use crate::{
    components::{AuthDialog, SpinnerButton},
    session::{Session, UserExt},
    spawn, toast,
};

mod imp {
    use glib::{subclass::InitializingObject, WeakRef};

    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/org/gnome/Fractal/account-settings-deactivate-account-subpage.ui")]
    pub struct DeactivateAccountSubpage {
        pub session: WeakRef<Session>,
        #[template_child]
        pub confirmation: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub button: TemplateChild<SpinnerButton>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DeactivateAccountSubpage {
        const NAME: &'static str = "DeactivateAccountSubpage";
        type Type = super::DeactivateAccountSubpage;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DeactivateAccountSubpage {
        fn properties() -> &'static [glib::ParamSpec] {
            use once_cell::sync::Lazy;
            static PROPERTIES: Lazy<Vec<glib::ParamSpec>> = Lazy::new(|| {
                vec![glib::ParamSpecObject::new(
                    "session",
                    "Session",
                    "The session",
                    Session::static_type(),
                    glib::ParamFlags::READWRITE,
                )]
            });

            PROPERTIES.as_ref()
        }

        fn set_property(
            &self,
            obj: &Self::Type,
            _id: usize,
            value: &glib::Value,
            pspec: &glib::ParamSpec,
        ) {
            match pspec.name() {
                "session" => obj.set_session(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, obj: &Self::Type, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "session" => obj.session().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self, obj: &Self::Type) {
            self.parent_constructed(obj);

            self.confirmation
                .connect_entry_activated(clone!(@weak obj => move|_| {
                    spawn!(
                        clone!(@weak obj => async move {
                            obj.deactivate_account().await;
                        })
                    );
                }));
            self.confirmation
                .connect_changed(clone!(@weak obj => move|_| {
                    obj.update_button();
                }));

            self.button.connect_clicked(clone!(@weak obj => move|_| {
                spawn!(
                    clone!(@weak obj => async move {
                        obj.deactivate_account().await;
                    })
                );
            }));
        }
    }

    impl WidgetImpl for DeactivateAccountSubpage {}
    impl BoxImpl for DeactivateAccountSubpage {}
}

glib::wrapper! {
    /// Account settings page about the user and the session.
    pub struct DeactivateAccountSubpage(ObjectSubclass<imp::DeactivateAccountSubpage>)
        @extends gtk::Widget, gtk::Box, @implements gtk::Accessible;
}

impl DeactivateAccountSubpage {
    pub fn new(session: &Session) -> Self {
        glib::Object::new(&[("session", session)])
            .expect("Failed to create DeactivateAccountSubpage")
    }

    pub fn session(&self) -> Option<Session> {
        self.imp().session.upgrade()
    }

    pub fn set_session(&self, session: Option<Session>) {
        if let Some(session) = session {
            let priv_ = self.imp();
            priv_.session.set(Some(&session));
            priv_.confirmation.set_title(&self.user_id());
        }
    }

    fn user_id(&self) -> String {
        self.session()
            .as_ref()
            .and_then(|session| session.user())
            .unwrap()
            .user_id()
            .to_string()
    }

    fn update_button(&self) {
        self.imp()
            .button
            .set_sensitive(self.can_deactivate_account());
    }

    fn can_deactivate_account(&self) -> bool {
        let confirmation = self.imp().confirmation.text();
        confirmation == self.user_id()
    }

    async fn deactivate_account(&self) {
        if !self.can_deactivate_account() {
            return;
        }

        let priv_ = self.imp();
        priv_.button.set_loading(true);
        priv_.confirmation.set_sensitive(false);

        let session = self.session().unwrap();
        let dialog = AuthDialog::new(
            self.root()
                .as_ref()
                .and_then(|root| root.downcast_ref::<gtk::Window>()),
            &session,
        );

        let result = dialog
            .authenticate(move |client, auth_data| async move {
                if let Some(auth) = auth_data {
                    let auth = Some(auth.as_matrix_auth_data());
                    let request = assign!(deactivate::v3::Request::new(), { auth });
                    client.send(request, None).await.map_err(Into::into)
                } else {
                    let request = deactivate::v3::Request::new();
                    client.send(request, None).await.map_err(Into::into)
                }
            })
            .await;

        match result {
            Ok(_) => {
                if let Some(session) = self.session() {
                    toast!(session, gettext("Account successfully deactivated"));
                    session.cleanup_session().await;
                }
                self.activate_action("account-settings.close", None)
                    .unwrap();
            }
            Err(err) => {
                error!("Failed to deactivate account: {:?}", err);
                toast!(self, gettext("Could not deactivate account"));
            }
        }
        priv_.button.set_loading(false);
        priv_.confirmation.set_sensitive(true);
    }
}
