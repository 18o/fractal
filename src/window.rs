use adw::subclass::prelude::AdwApplicationWindowImpl;
use gettextrs::gettext;
use glib::signal::Inhibit;
use gtk::{self, gdk, gio, glib, glib::clone, prelude::*, subclass::prelude::*, CompositeTemplate};
use log::{info, warn};

use crate::{
    account_switcher::AccountSwitcher,
    config::{APP_ID, PROFILE},
    secret::{self, SecretError},
    spawn, Application, ErrorPage, Greeter, Login, Session,
};

mod imp {
    use glib::subclass::InitializingObject;
    use once_cell::sync::Lazy;

    use super::*;

    #[derive(Debug, CompositeTemplate, Default)]
    #[template(resource = "/org/gnome/Fractal/window.ui")]
    pub struct Window {
        #[template_child]
        pub main_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading: TemplateChild<gtk::WindowHandle>,
        #[template_child]
        pub greeter: TemplateChild<Greeter>,
        #[template_child]
        pub login: TemplateChild<Login>,
        #[template_child]
        pub error_page: TemplateChild<ErrorPage>,
        #[template_child]
        pub sessions: TemplateChild<gtk::Stack>,
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub offline_info_bar: TemplateChild<gtk::InfoBar>,
        #[template_child]
        pub offline_info_bar_label: TemplateChild<gtk::Label>,
        pub account_switcher: AccountSwitcher,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "Window";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);

            klass.add_binding_action(
                gdk::Key::v,
                gdk::ModifierType::CONTROL_MASK,
                "win.paste",
                None,
            );
            klass.add_binding_action(
                gdk::Key::Insert,
                gdk::ModifierType::SHIFT_MASK,
                "win.paste",
                None,
            );
            klass.install_action("win.paste", None, move |widget, _, _| {
                if let Some(session) = widget
                    .imp()
                    .sessions
                    .visible_child()
                    .and_then(|c| c.downcast::<Session>().ok())
                {
                    session.handle_paste_action();
                }
            });
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Window {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: Lazy<Vec<glib::ParamSpec>> = Lazy::new(|| {
                vec![glib::ParamSpecBoolean::new(
                    "has-sessions",
                    "Has Sessions",
                    "Whether this window has sessions",
                    false,
                    glib::ParamFlags::READABLE,
                )]
            });

            PROPERTIES.as_ref()
        }

        fn property(&self, obj: &Self::Type, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "has-sessions" => obj.has_sessions().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self, obj: &Self::Type) {
            self.parent_constructed(obj);

            let builder = gtk::Builder::from_resource("/org/gnome/Fractal/shortcuts.ui");
            let shortcuts = builder.object("shortcuts").unwrap();
            obj.set_help_overlay(Some(&shortcuts));

            // Devel Profile
            if PROFILE == "Devel" {
                obj.add_css_class("devel");
            }

            obj.load_window_size();

            // Ask for the toggle fullscreen state
            let fullscreen = gio::SimpleAction::new("toggle-fullscreen", None);
            fullscreen.connect_activate(clone!(@weak obj as window => move |_, _| {
                if window.is_fullscreened() {
                    window.unfullscreen();
                } else {
                    window.fullscreen();
                }
            }));
            obj.add_action(&fullscreen);

            spawn!(clone!(@weak obj => async move {
                obj.restore_sessions().await;
            }));

            self.account_switcher.set_pages(Some(self.sessions.pages()));

            let monitor = gio::NetworkMonitor::default();
            monitor.connect_network_changed(clone!(@weak obj => move |_, _| {
                obj.update_network_state();
            }));

            obj.update_network_state();
        }
    }

    impl WindowImpl for Window {
        // save window state on delete event
        fn close_request(&self, obj: &Self::Type) -> Inhibit {
            if let Err(err) = obj.save_window_size() {
                warn!("Failed to save window state, {}", &err);
            }
            Inhibit(false)
        }
    }

    impl WidgetImpl for Window {}
    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}
}

glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends gtk::Widget, gtk::Window, gtk::Root, gtk::ApplicationWindow, adw::ApplicationWindow, @implements gtk::Accessible, gio::ActionMap, gio::ActionGroup;
}

impl Window {
    pub fn new(app: &Application) -> Self {
        glib::Object::new(&[("application", &Some(app)), ("icon-name", &Some(APP_ID))])
            .expect("Failed to create Window")
    }

    fn add_session(&self, session: &Session) {
        let priv_ = &self.imp();
        let prev_has_sessions = self.has_sessions();

        priv_.sessions.add_child(session);
        priv_.sessions.set_visible_child(session);
        // We need to grab the focus so that keyboard shortcuts work
        session.grab_focus();

        session.connect_logged_out(clone!(@weak self as obj => move |session| {
            obj.remove_session(session)
        }));

        if !prev_has_sessions {
            self.notify("has-sessions");
        }
    }

    fn remove_session(&self, session: &Session) {
        let priv_ = self.imp();

        priv_.sessions.remove(session);

        if let Some(child) = priv_.sessions.first_child() {
            priv_.sessions.set_visible_child(&child);
        } else {
            self.notify("has-sessions");
            self.switch_to_greeter_page(false);
        }
    }

    pub async fn restore_sessions(&self) {
        match secret::restore_sessions().await {
            Ok(sessions) => {
                if sessions.is_empty() {
                    self.switch_to_greeter_page(false);
                } else {
                    for stored_session in sessions {
                        info!(
                            "Restoring previous session for user: {}",
                            stored_session.user_id
                        );
                        if let Some(path) = stored_session.path.to_str() {
                            info!("Database path: {path}");
                        }

                        let session = Session::new();
                        spawn!(
                            glib::PRIORITY_DEFAULT_IDLE,
                            clone!(@weak session => async move {
                                session.login_with_previous_session(stored_session).await;
                            })
                        );
                        self.add_session(&session);
                    }
                }

                let priv_ = self.imp();
                priv_.login.connect_new_session(
                    clone!(@weak self as obj => move |_login, session| {
                        obj.add_session(&session);
                        obj.switch_to_loading_page();
                    }),
                );

                priv_.main_stack.connect_visible_child_notify(
                    clone!(@weak self as obj => move |_| obj.set_default_by_child()),
                );

                self.set_default_by_child();
            }
            Err(error) => {
                warn!("Failed to restore previous sessions: {:?}", error);
                self.switch_to_error_page(
                    &format!(
                        "{}\n\n{}",
                        gettext("Failed to restore previous sessions"),
                        error,
                    ),
                    error,
                );
            }
        }
    }

    /// Whether this window has sessions.
    pub fn has_sessions(&self) -> bool {
        self.imp().sessions.pages().n_items() > 0
    }

    pub fn save_window_size(&self) -> Result<(), glib::BoolError> {
        let settings = Application::default().settings();

        let size = self.default_size();

        settings.set_int("window-width", size.0)?;
        settings.set_int("window-height", size.1)?;

        settings.set_boolean("is-maximized", self.is_maximized())?;

        Ok(())
    }

    fn load_window_size(&self) {
        let settings = Application::default().settings();

        let width = settings.int("window-width");
        let height = settings.int("window-height");
        let is_maximized = settings.boolean("is-maximized");

        self.set_default_size(width, height);
        self.set_property("maximized", &is_maximized);
    }

    /// Change the default widget of the window based on the visible child.
    ///
    /// These are the default widgets:
    /// - `Greeter` screen => `Login` button.
    /// - `Login screen` => `Next` button.
    fn set_default_by_child(&self) {
        let priv_ = self.imp();

        if priv_.main_stack.visible_child() == Some(priv_.greeter.get().upcast()) {
            self.set_default_widget(Some(&priv_.greeter.default_widget()));
        } else if priv_.main_stack.visible_child() == Some(priv_.login.get().upcast()) {
            self.set_default_widget(Some(&priv_.login.default_widget()));
        } else {
            self.set_default_widget(gtk::Widget::NONE);
        }
    }

    pub fn switch_to_loading_page(&self) {
        let priv_ = self.imp();
        priv_.main_stack.set_visible_child(&*priv_.loading);
    }

    pub fn switch_to_sessions_page(&self) {
        let priv_ = self.imp();
        priv_.main_stack.set_visible_child(&priv_.sessions.get());
    }

    pub fn switch_to_login_page(&self) {
        let priv_ = self.imp();
        priv_.main_stack.set_visible_child(&*priv_.login);
        priv_.login.focus_default();
    }

    pub fn switch_to_greeter_page(&self, clean: bool) {
        let priv_ = self.imp();
        if clean {
            priv_.login.clean();
        }
        priv_.main_stack.set_visible_child(&*priv_.greeter);
    }

    pub fn switch_to_error_page(&self, message: &str, error: SecretError) {
        let priv_ = self.imp();
        priv_.error_page.display_secret_error(message, error);
        priv_.main_stack.set_visible_child(&*priv_.error_page);
    }

    /// This appends a new toast to the list
    pub fn add_toast(&self, toast: &adw::Toast) {
        self.imp().toast_overlay.add_toast(toast);
    }

    pub fn account_switcher(&self) -> &AccountSwitcher {
        &self.imp().account_switcher
    }

    fn update_network_state(&self) {
        let priv_ = self.imp();
        let monitor = gio::NetworkMonitor::default();

        if !monitor.is_network_available() {
            priv_
                .offline_info_bar_label
                .set_label(&gettext("No network connection"));
            priv_.offline_info_bar.set_revealed(true);
        } else if monitor.connectivity() < gio::NetworkConnectivity::Full {
            priv_
                .offline_info_bar_label
                .set_label(&gettext("No Internet connection"));
            priv_.offline_info_bar.set_revealed(true);
        } else {
            priv_.offline_info_bar.set_revealed(false);
        }
    }
}
