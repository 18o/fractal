use adw::{prelude::*, subclass::prelude::BinImpl};
use gettextrs::gettext;
use gtk::{self, gio, glib, glib::clone, subclass::prelude::*, CompositeTemplate};
use log::{debug, warn};
use matrix_sdk::{
    config::RequestConfig,
    ruma::{
        api::client::session::get_login_types::v3::{
            LoginType::{Password, Sso},
            SsoLoginType,
        },
        IdParseError, OwnedServerName, ServerName,
    },
    Client,
};
use url::{ParseError, Url};

mod idp_button;
mod login_advanced_dialog;

use idp_button::IdpButton;
use login_advanced_dialog::LoginAdvancedDialog;

use crate::{
    components::{EntryRow, PasswordEntryRow, SpinnerButton},
    gettext_f, spawn, spawn_tokio, toast,
    user_facing_error::UserFacingError,
    Session,
};

mod imp {
    use std::cell::{Cell, RefCell};

    use glib::{
        subclass::{InitializingObject, Signal},
        SignalHandlerId,
    };
    use once_cell::sync::Lazy;

    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/org/gnome/Fractal/login.ui")]
    pub struct Login {
        pub current_session: RefCell<Option<Session>>,
        #[template_child]
        pub back_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub next_button: TemplateChild<SpinnerButton>,
        #[template_child]
        pub main_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub homeserver_entry: TemplateChild<EntryRow>,
        #[template_child]
        pub homeserver_help: TemplateChild<gtk::Label>,
        #[template_child]
        pub password_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub username_entry: TemplateChild<EntryRow>,
        #[template_child]
        pub password_entry: TemplateChild<PasswordEntryRow>,
        #[template_child]
        pub sso_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub more_sso_option: TemplateChild<gtk::Button>,
        #[template_child]
        pub offline_info_bar: TemplateChild<gtk::InfoBar>,
        #[template_child]
        pub offline_info_bar_label: TemplateChild<gtk::Label>,
        pub prepared_source_id: RefCell<Option<SignalHandlerId>>,
        pub logged_out_source_id: RefCell<Option<SignalHandlerId>>,
        pub ready_source_id: RefCell<Option<SignalHandlerId>>,
        /// Whether auto-discovery is enabled.
        pub autodiscovery: Cell<bool>,
        pub supports_password: Cell<bool>,
        /// The homeserver to log into.
        pub homeserver: RefCell<Option<Url>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Login {
        const NAME: &'static str = "Login";
        type Type = super::Login;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
            klass.set_css_name("login");
            klass.set_accessible_role(gtk::AccessibleRole::Group);
            klass.install_action("login.next", None, move |widget, _, _| widget.forward());
            klass.install_action("login.prev", None, move |widget, _, _| widget.backward());
            klass.install_action("login.open-advanced", None, move |widget, _, _| {
                spawn!(clone!(@weak widget => async move {
                    widget.open_advanced_dialog().await;
                }));
            });
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Login {
        fn signals() -> &'static [Signal] {
            static SIGNALS: Lazy<Vec<Signal>> = Lazy::new(|| {
                vec![Signal::builder(
                    "new-session",
                    &[Session::static_type().into()],
                    <()>::static_type().into(),
                )
                .build()]
            });
            SIGNALS.as_ref()
        }

        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: Lazy<Vec<glib::ParamSpec>> = Lazy::new(|| {
                vec![
                    glib::ParamSpecString::new(
                        "homeserver",
                        "Homeserver",
                        "The homeserver to log into",
                        None,
                        glib::ParamFlags::READABLE,
                    ),
                    glib::ParamSpecBoolean::new(
                        "autodiscovery",
                        "Auto-discovery",
                        "Whether auto-discovery is enabled",
                        true,
                        glib::ParamFlags::READWRITE | glib::ParamFlags::CONSTRUCT,
                    ),
                ]
            });

            PROPERTIES.as_ref()
        }

        fn property(&self, obj: &Self::Type, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "homeserver" => obj.homeserver_pretty().to_value(),
                "autodiscovery" => obj.autodiscovery().to_value(),
                _ => unimplemented!(),
            }
        }

        fn set_property(
            &self,
            obj: &Self::Type,
            _id: usize,
            value: &glib::Value,
            pspec: &glib::ParamSpec,
        ) {
            match pspec.name() {
                "autodiscovery" => obj.set_autodiscovery(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self, obj: &Self::Type) {
            obj.action_set_enabled("login.next", false);

            self.parent_constructed(obj);

            let monitor = gio::NetworkMonitor::default();
            monitor.connect_network_changed(clone!(@weak obj => move |_, _| {
                obj.update_network_state();
            }));

            obj.update_network_state();

            self.main_stack
                .connect_visible_child_notify(clone!(@weak obj => move |_|
                    obj.update_next_action();
                    obj.focus_default();
                ));
            obj.update_next_action();

            self.homeserver_entry
                .connect_activated(clone!(@weak obj => move|_| {
                    obj.default_widget().activate();
                }));
            self.homeserver_entry
                .connect_changed(clone!(@weak obj => move |_| obj.update_next_action()));
            self.username_entry
                .connect_activated(clone!(@weak obj => move|_| {
                    obj.default_widget().activate();
                }));
            self.username_entry
                .connect_changed(clone!(@weak obj => move |_| obj.update_next_action()));
            self.password_entry
                .connect_activated(clone!(@weak obj => move|_| {
                    obj.default_widget().activate();
                }));
            self.password_entry
                .connect_changed(clone!(@weak obj => move |_| obj.update_next_action()));
            self.more_sso_option
                .connect_clicked(clone!(@weak obj => move |_| obj.login_with_sso(None)));
        }
    }

    impl WidgetImpl for Login {}

    impl BinImpl for Login {}
}

glib::wrapper! {
    /// A widget handling the login flows.
    pub struct Login(ObjectSubclass<imp::Login>)
        @extends gtk::Widget, adw::Bin, @implements gtk::Accessible;
}

impl Login {
    pub fn new() -> Self {
        glib::Object::new(&[]).expect("Failed to create Login")
    }

    pub fn homeserver(&self) -> Option<Url> {
        self.imp().homeserver.borrow().clone()
    }

    fn reload_sso_panel(&self, login_types: &SsoLoginType) {
        let priv_ = &mut imp::Login::from_instance(self);
        let mut child = priv_.sso_box.first_child();
        while child.is_some() {
            priv_.sso_box.remove(&child.unwrap());
            child = priv_.sso_box.first_child();
        }
        let mut has_unknown_methods = false;
        let mut has_known_methods = false;
        let homeserver: Url = self.homeserver().unwrap();
        for provider in login_types.identity_providers.iter() {
            let opt_brand = provider.brand.as_ref();
            if opt_brand.is_none() {
                has_unknown_methods = true;
                continue;
            }
            let btn = IdpButton::new_from_identity_provider(homeserver.clone(), provider);
            if let Some(real) = btn {
                self.imp().sso_box.append(&real);
                real.connect_clicked(
                    clone!(@weak self as obj => move |btn| obj.login_with_sso(btn.id())),
                );
                has_known_methods = true;
            } else {
                has_unknown_methods = true;
            }
        }
        priv_.sso_box.set_visible(has_known_methods);
        priv_.more_sso_option.set_visible(has_unknown_methods);
    }

    pub fn homeserver_pretty(&self) -> Option<String> {
        let homeserver = self.homeserver();
        homeserver
            .as_ref()
            .and_then(|url| url.as_ref().strip_suffix('/').map(ToOwned::to_owned))
            .or_else(|| homeserver.as_ref().map(ToString::to_string))
    }

    pub fn set_homeserver(&self, homeserver: Option<Url>) {
        let priv_ = imp::Login::from_instance(self);

        if self.homeserver() == homeserver {
            return;
        }

        priv_.homeserver.replace(homeserver);
        self.notify("homeserver");
    }

    fn visible_child(&self) -> String {
        let priv_ = imp::Login::from_instance(self);
        priv_.main_stack.visible_child_name().unwrap().into()
    }

    fn set_visible_child(&self, visible_child: &str) {
        let priv_ = imp::Login::from_instance(self);
        priv_.main_stack.set_visible_child_name(visible_child);
    }

    fn update_next_action(&self) {
        let priv_ = imp::Login::from_instance(self);
        match self.visible_child().as_ref() {
            "homeserver" => {
                let homeserver = priv_.homeserver_entry.text();
                let enabled = if self.autodiscovery() {
                    build_server_name(homeserver.as_str()).is_ok()
                } else {
                    build_homeserver_url(homeserver.as_str()).is_ok()
                };
                self.action_set_enabled("login.next", enabled);
                priv_.next_button.set_visible(true);
            }
            "password" => {
                let username_length = priv_.username_entry.text().len();
                let password_length = priv_.password_entry.text().len();
                self.action_set_enabled("login.next", username_length != 0 && password_length != 0);
                priv_.next_button.set_visible(true);
            }
            _ => {
                priv_.next_button.set_visible(false);
            }
        }
    }

    fn forward(&self) {
        match self.visible_child().as_ref() {
            "homeserver" => {
                if self.autodiscovery() {
                    self.try_autodiscovery();
                } else {
                    self.check_homeserver();
                }
            }
            "password" => self.login_with_password(),
            _ => {}
        }
    }

    fn backward(&self) {
        match self.visible_child().as_ref() {
            "password" => self.set_visible_child("homeserver"),
            "sso_message_page" => {
                self.set_visible_child(if self.imp().supports_password.get() {
                    "password"
                } else {
                    "homeserver"
                });
            }
            _ => {
                self.activate_action("app.show-greeter", None).unwrap();
            }
        }
    }

    pub fn autodiscovery(&self) -> bool {
        self.imp().autodiscovery.get()
    }

    fn set_autodiscovery(&self, autodiscovery: bool) {
        let priv_ = self.imp();

        priv_.autodiscovery.set(autodiscovery);
        if autodiscovery {
            priv_.homeserver_entry.set_title(&gettext("Domain Name"));
            priv_.homeserver_help.set_markup(&gettext(
                "The domain of your Matrix homeserver, for example gnome.org",
            ));
        } else {
            priv_.homeserver_entry.set_title(&gettext("Homeserver URL"));
            priv_.homeserver_help.set_markup(&gettext_f(
                // Translators: Do NOT translate the content between '{' and '}', this is a
                // variable name.
                "The URL of your Matrix homeserver, for example {address}",
                &[(
                    "address",
                    "<span segment=\"word\">https://gnome.modular.im</span>",
                )],
            ));
        }
        self.update_next_action();
    }

    async fn open_advanced_dialog(&self) {
        let dialog =
            LoginAdvancedDialog::new(self.root().unwrap().downcast_ref::<gtk::Window>().unwrap());
        self.bind_property("autodiscovery", &dialog, "autodiscovery")
            .flags(glib::BindingFlags::SYNC_CREATE | glib::BindingFlags::BIDIRECTIONAL)
            .build();
        dialog.run_future().await;
    }

    fn try_autodiscovery(&self) {
        let server = build_server_name(self.imp().homeserver_entry.text().as_str()).unwrap();

        self.freeze();

        let handle =
            spawn_tokio!(async move { Client::builder().server_name(&server).build().await });

        spawn!(
            glib::PRIORITY_DEFAULT_IDLE,
            clone!(@weak self as obj => async move {
                match handle.await.unwrap() {
                    Ok(client) => {
                        let homeserver = client.homeserver().await;
                        obj.set_homeserver(Some(homeserver));
                        obj.check_login_types(client).await;
                    }
                    Err(error) => {
                        warn!("Failed to discover homeserver: {}", error);
                        toast!(obj, error.to_user_facing());
                    }
                };
                obj.unfreeze();
            })
        );
    }

    fn switch_off_sso(&self) {
        let priv_ = self.imp();
        priv_.sso_box.set_visible(false);
        priv_.more_sso_option.set_visible(false);
    }

    async fn check_login_types(&self, client: Client) {
        let login_types = spawn_tokio!(async move { client.get_login_types().await })
            .await
            .unwrap()
            .unwrap();
        let sso = login_types
            .flows
            .iter()
            .find(|flow| matches!(flow, Sso(_sso_providers)));
        let password = login_types
            .flows
            .iter()
            .find(|flow| matches!(flow, Password(_)));
        let has_sso = sso.is_some();
        let has_password = password.is_some();
        self.imp().supports_password.replace(has_password);
        if has_sso && has_password {
            if let Sso(login_type) = sso.unwrap() {
                self.reload_sso_panel(login_type);
            }
        } else if !has_sso {
            self.switch_off_sso();
        }
        if has_password {
            self.show_password_page();
        } else {
            self.login_with_sso(None);
        }
    }

    fn check_homeserver(&self) {
        let homeserver = build_homeserver_url(self.imp().homeserver_entry.text().as_str()).unwrap();
        let homeserver_clone = homeserver.clone();

        self.freeze();

        let handle = spawn_tokio!(async move {
            Client::builder()
                .homeserver_url(homeserver_clone)
                .request_config(RequestConfig::new().disable_retry())
                .build()
                .await
        });

        spawn!(
            glib::PRIORITY_DEFAULT_IDLE,
            clone!(@weak self as obj => async move {
                match handle.await.unwrap() {
                    Ok(client) => {
                        obj.set_homeserver(Some(homeserver));
                        obj.check_login_types(client).await;
                    }
                    Err(error) => {
                        warn!("Failed to check homeserver: {}", error);
                        toast!(obj, error.to_user_facing());
                    }
                };
                obj.unfreeze();
            })
        );
    }

    fn show_password_page(&self) {
        let priv_ = self.imp();

        let domain_name = if self.autodiscovery() {
            priv_.homeserver_entry.text().to_string()
        } else {
            self.homeserver_pretty().unwrap()
        };

        priv_.password_title.set_markup(&gettext_f(
            // Translators: Do NOT translate the content between '{' and '}', this is a variable
            // name.
            "Connecting to {domain_name}",
            &[(
                "domain_name",
                &format!("<span segment=\"word\">{}</span>", domain_name),
            )],
        ));

        self.set_visible_child("password");
    }

    fn login_with_password(&self) {
        let priv_ = self.imp();
        let homeserver = self.homeserver().unwrap();
        let username = priv_.username_entry.text().to_string();
        let password = priv_.password_entry.text().to_string();
        let autodiscovery = self.autodiscovery();

        self.freeze();

        let session = Session::new();
        self.set_handler_for_prepared_session(&session);

        spawn!(
            glib::PRIORITY_DEFAULT_IDLE,
            clone!(@weak session => async move {
                session.login_with_password(homeserver, username, password, autodiscovery).await;
            })
        );
        priv_.current_session.replace(Some(session));
    }

    fn login_with_sso(&self, idp_id: Option<String>) {
        let priv_ = imp::Login::from_instance(self);
        let homeserver = self.homeserver().unwrap();
        self.set_visible_child("sso_message_page");

        let session = Session::new();
        self.set_handler_for_prepared_session(&session);
        spawn!(
            glib::PRIORITY_DEFAULT_IDLE,
            clone!(@weak session, @weak self as s => async move {
                session.login_with_sso(homeserver, idp_id).await;
                s.set_visible_child("homeserver");
            })
        );
        priv_.current_session.replace(Some(session));
    }

    pub fn clean(&self) {
        let priv_ = self.imp();
        priv_.homeserver_entry.set_text("");
        priv_.username_entry.set_text("");
        priv_.password_entry.set_text("");
        priv_.autodiscovery.set(true);
        priv_.homeserver.take();
        priv_.main_stack.set_visible_child_name("homeserver");
        self.unfreeze();
        self.drop_session_reference();
    }

    fn freeze(&self) {
        let priv_ = self.imp();

        self.action_set_enabled("login.next", false);
        priv_.next_button.set_loading(true);
        priv_.main_stack.set_sensitive(false);
    }

    fn unfreeze(&self) {
        let priv_ = self.imp();

        priv_.next_button.set_loading(false);
        priv_.main_stack.set_sensitive(true);
        self.update_next_action();
    }

    pub fn connect_new_session<F: Fn(&Self, Session) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("new-session", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            let session = values[1].get::<Session>().unwrap();

            f(&obj, session);

            None
        })
    }

    fn drop_session_reference(&self) {
        let priv_ = self.imp();

        if let Some(session) = priv_.current_session.take() {
            if let Some(id) = priv_.prepared_source_id.take() {
                session.disconnect(id);
            }
            if let Some(id) = priv_.logged_out_source_id.take() {
                session.disconnect(id);
            }
            if let Some(id) = priv_.ready_source_id.take() {
                session.disconnect(id);
            }
        }
    }

    pub fn default_widget(&self) -> gtk::Widget {
        self.imp().next_button.get().upcast()
    }

    /// Set focus to the proper widget of the current page.
    pub fn focus_default(&self) {
        let priv_ = self.imp();
        match self.visible_child().as_ref() {
            "homeserver" => {
                priv_.homeserver_entry.grab_focus();
            }
            "password" => {
                priv_.username_entry.grab_focus();
            }
            _ => {}
        }
    }

    fn set_handler_for_prepared_session(&self, session: &Session) {
        let priv_ = self.imp();
        priv_
            .prepared_source_id
            .replace(Some(session.connect_prepared(
                clone!(@weak self as login => move |session, error| {
                    match error {
                        Some(e) => {
                            toast!(login, e);
                            login.unfreeze();
                        },
                        None => {
                            debug!("A new session was prepared");
                            login.emit_by_name::<()>("new-session", &[&session]);
                        }
                    }
                }),
            )));

        priv_.ready_source_id.replace(Some(session.connect_ready(
            clone!(@weak self as login => move |_| {
                login.clean();
            }),
        )));

        priv_
            .logged_out_source_id
            .replace(Some(session.connect_logged_out(
                clone!(@weak self as login => move |_| {
                    login.parent_window().switch_to_login_page();
                    login.drop_session_reference();
                    login.unfreeze();
                }),
            )));
    }

    fn parent_window(&self) -> crate::Window {
        self.root()
            .and_then(|root| root.downcast().ok())
            .expect("Login needs to have a parent window")
    }

    fn update_network_state(&self) {
        let priv_ = self.imp();
        let monitor = gio::NetworkMonitor::default();

        if !monitor.is_network_available() {
            priv_
                .offline_info_bar_label
                .set_label(&gettext("No network connection"));
            priv_.offline_info_bar.set_revealed(true);
            self.update_next_action();
            priv_.sso_box.set_sensitive(false);
            priv_.more_sso_option.set_sensitive(false);
        } else if monitor.connectivity() < gio::NetworkConnectivity::Full {
            priv_
                .offline_info_bar_label
                .set_label(&gettext("No Internet connection"));
            priv_.offline_info_bar.set_revealed(true);
            self.update_next_action();
            priv_.sso_box.set_sensitive(true);
            priv_.more_sso_option.set_sensitive(true);
        } else {
            priv_.offline_info_bar.set_revealed(false);
            self.update_next_action();
            priv_.sso_box.set_sensitive(true);
            priv_.more_sso_option.set_sensitive(true);
        }
    }
}

impl Default for Login {
    fn default() -> Self {
        Self::new()
    }
}

fn build_server_name(server: &str) -> Result<OwnedServerName, IdParseError> {
    let server = server
        .strip_prefix("http://")
        .or_else(|| server.strip_prefix("https://"))
        .unwrap_or(server);
    ServerName::parse(server)
}

fn build_homeserver_url(server: &str) -> Result<Url, ParseError> {
    if server.starts_with("http://") || server.starts_with("https://") {
        Url::parse(server)
    } else {
        Url::parse(&format!("https://{}", server))
    }
}
