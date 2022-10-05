mod account_settings;
mod avatar;
mod content;
mod event_source_dialog;
mod media_viewer;
pub mod room;
mod room_creation;
mod room_list;
mod sidebar;
mod user;
pub mod verification;

use std::{
    collections::HashSet,
    convert::TryFrom,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use adw::subclass::prelude::BinImpl;
use futures::StreamExt;
use gettextrs::gettext;
use gtk::{
    self, gdk, gio,
    gio::prelude::*,
    glib,
    glib::{clone, signal::SignalHandlerId},
    prelude::*,
    subclass::prelude::*,
    CompositeTemplate,
};
use log::{debug, error, warn};
use matrix_sdk::{
    config::{RequestConfig, StoreConfig, SyncSettings},
    deserialized_responses::SyncResponse,
    room::Room as MatrixRoom,
    ruma::{
        api::{
            client::{
                error::ErrorKind,
                filter::{FilterDefinition, LazyLoadOptions, RoomEventFilter, RoomFilter},
                session::logout,
            },
            error::{FromHttpResponseError, ServerError},
        },
        assign,
        events::{
            direct::DirectEventContent, room::encryption::SyncRoomEncryptionEvent,
            GlobalAccountDataEvent,
        },
        RoomId,
    },
    store::{MigrationConflictStrategy, OpenStoreError, SledStateStore},
    Client, ClientBuildError, Error, HttpError, RumaApiError, StoreError,
};
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use thiserror::Error;
use tokio::task::JoinHandle;
use url::Url;

use self::{
    account_settings::AccountSettings,
    content::{verification::SessionVerification, Content},
    media_viewer::MediaViewer,
    room_list::RoomList,
    sidebar::Sidebar,
    verification::VerificationList,
};
pub use self::{
    avatar::Avatar,
    room::{Room, SupportedEvent},
    room_creation::RoomCreation,
    user::{User, UserActions, UserExt},
};
use crate::{
    gettext_f, secret,
    secret::{Secret, StoredSession},
    session::sidebar::ItemList,
    spawn, spawn_tokio, toast,
    utils::check_if_reachable,
    UserFacingError, Window,
};

#[derive(Error, Debug)]
pub enum ClientSetupError {
    #[error(transparent)]
    Store(#[from] OpenStoreError),
    #[error(transparent)]
    Client(#[from] ClientBuildError),
    #[error(transparent)]
    Sdk(#[from] Error),
}

impl UserFacingError for ClientSetupError {
    fn to_user_facing(self) -> String {
        match self {
            ClientSetupError::Store(err) => err.to_user_facing(),
            ClientSetupError::Client(err) => err.to_user_facing(),
            ClientSetupError::Sdk(err) => err.to_user_facing(),
        }
    }
}

#[derive(Debug)]
pub enum LoginError {
    /// Errors that happen during the login process
    Login(ClientSetupError),
    /// Errors that happen while restoring a session
    RestoreSession(ClientSetupError),
    /// Errors that happen when saving a session to SecretService
    StoreSession(secret::SecretError),
}

impl UserFacingError for LoginError {
    fn to_user_facing(self) -> String {
        match self {
            LoginError::Login(err) => gettext_f(
                "Failed to login: {error}",
                &[("error", &err.to_user_facing())],
            ),
            LoginError::RestoreSession(err) => gettext_f(
                "Failed to restore session: {error}",
                &[("error", &err.to_user_facing())],
            ),
            LoginError::StoreSession(err) => gettext_f(
                "Failed to store session: {error}",
                &[("error", &err.to_user_facing())],
            ),
        }
    }
}

#[derive(Debug, Hash, Eq, PartialEq, PartialOrd, Clone, Copy, glib::Enum)]
#[repr(u32)]
#[enum_type(name = "SessionState")]
pub enum State {
    Created,
    LoggedIn,
    CrossSigningSetup,
    Content,
    LoggedOut,
    Cleaned,
}

impl std::default::Default for State {
    fn default() -> Self {
        State::Created
    }
}

mod imp {
    use std::cell::{Cell, RefCell};

    use glib::subclass::InitializingObject;
    use once_cell::{sync::Lazy, unsync::OnceCell};

    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/org/gnome/Fractal/session.ui")]
    pub struct Session {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub leaflet: TemplateChild<adw::Leaflet>,
        #[template_child]
        pub sidebar: TemplateChild<Sidebar>,
        #[template_child]
        pub content: TemplateChild<Content>,
        #[template_child]
        pub media_viewer: TemplateChild<MediaViewer>,
        pub client: OnceCell<Client>,
        pub item_list: OnceCell<ItemList>,
        pub user: OnceCell<User>,
        pub state: Cell<State>,
        /// Whether this session received any sync response
        pub was_synced: Cell<bool>,
        pub info: OnceCell<StoredSession>,
        pub sync_tokio_handle: RefCell<Option<JoinHandle<()>>>,
        pub offline_handler_id: RefCell<Option<SignalHandlerId>>,
        pub offline: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Session {
        const NAME: &'static str = "Session";
        type Type = super::Session;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);

            klass.install_action("session.close-room", None, move |session, _, _| {
                session.select_room(None);
            });

            klass.install_action(
                "session.show-room",
                Some("s"),
                move |session, _, parameter| {
                    if let Ok(room_id) =
                        <&RoomId>::try_from(&*parameter.unwrap().get::<String>().unwrap())
                    {
                        session.select_room_by_id(room_id);
                    } else {
                        error!("Can't show room because the provided id is invalid");
                    }
                },
            );

            klass.install_action("session.logout", None, move |session, _, _| {
                session.logout()
            });

            klass.install_action("session.show-content", None, move |session, _, _| {
                spawn!(clone!(@weak session => async move { session.show_content().await; }));
            });

            klass.install_action("session.room-creation", None, move |session, _, _| {
                session.show_room_creation_dialog();
            });

            klass.add_binding_action(
                gdk::Key::Escape,
                gdk::ModifierType::empty(),
                "session.close-room",
                None,
            );

            klass.install_action("session.toggle-room-search", None, move |session, _, _| {
                session.toggle_room_search();
            });

            klass.add_binding_action(
                gdk::Key::k,
                gdk::ModifierType::CONTROL_MASK,
                "session.toggle-room-search",
                None,
            );

            klass.install_action(
                "session.open-account-settings",
                None,
                move |widget, _, _| {
                    widget.open_account_settings();
                },
            );
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Session {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: Lazy<Vec<glib::ParamSpec>> = Lazy::new(|| {
                vec![
                    glib::ParamSpecObject::new(
                        "item-list",
                        "Item List",
                        "The list of items in the sidebar",
                        ItemList::static_type(),
                        glib::ParamFlags::READABLE,
                    ),
                    glib::ParamSpecObject::new(
                        "user",
                        "User",
                        "The user of this session",
                        User::static_type(),
                        glib::ParamFlags::READABLE,
                    ),
                    glib::ParamSpecBoolean::new(
                        "offline",
                        "Offline",
                        "Whether this session has a connection to the homeserver",
                        false,
                        glib::ParamFlags::READABLE,
                    ),
                    glib::ParamSpecEnum::new(
                        "state",
                        "State",
                        "The the state of the session",
                        State::static_type(),
                        State::default() as i32,
                        glib::ParamFlags::READABLE,
                    ),
                ]
            });

            PROPERTIES.as_ref()
        }

        fn property(&self, obj: &Self::Type, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "item-list" => obj.item_list().to_value(),
                "user" => obj.user().to_value(),
                "offline" => obj.is_offline().to_value(),
                "state" => obj.state().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self, obj: &Self::Type) {
            self.parent_constructed(obj);

            self.sidebar.connect_notify_local(
                Some("selected-item"),
                clone!(@weak obj => move |_, _| {
                    let priv_ = obj.imp();

                    if priv_.sidebar.selected_item().is_none() {
                        priv_.leaflet.navigate(adw::NavigationDirection::Back);
                    } else {
                        priv_.leaflet.navigate(adw::NavigationDirection::Forward);
                    }
                }),
            );

            let monitor = gio::NetworkMonitor::default();
            let handler_id = monitor.connect_network_changed(clone!(@weak obj => move |_, _| {
                spawn!(clone!(@weak obj => async move {
                    obj.update_offline().await;
                }));
            }));

            self.offline_handler_id.replace(Some(handler_id));
        }

        fn dispose(&self, _obj: &Self::Type) {
            // Needs to be disconnected or else it may restart the sync
            if let Some(handler_id) = self.offline_handler_id.take() {
                gio::NetworkMonitor::default().disconnect(handler_id);
            }

            if let Some(handle) = self.sync_tokio_handle.take() {
                handle.abort();
            }
        }
    }
    impl WidgetImpl for Session {}
    impl BinImpl for Session {}
}

glib::wrapper! {
    pub struct Session(ObjectSubclass<imp::Session>)
        @extends gtk::Widget, adw::Bin, @implements gtk::Accessible;
}

impl Session {
    pub fn new() -> Self {
        glib::Object::new(&[]).expect("Failed to create Session")
    }

    pub fn select_room(&self, room: Option<Room>) {
        self.imp()
            .sidebar
            .set_selected_item(room.map(|item| item.upcast()));
    }

    pub fn select_item(&self, item: Option<glib::Object>) {
        self.imp().sidebar.set_selected_item(item);
    }

    pub fn select_room_by_id(&self, room_id: &RoomId) {
        if let Some(room) = self.room_list().get(room_id) {
            self.select_room(Some(room));
        } else {
            warn!("A room with id {} couldn't be found", room_id);
        }
    }

    fn toggle_room_search(&self) {
        let room_search = self.imp().sidebar.room_search_bar();
        room_search.set_search_mode(!room_search.is_search_mode());
    }

    pub async fn login_with_password(
        &self,
        homeserver: Url,
        username: String,
        password: String,
        use_discovery: bool,
    ) -> Result<(), LoginError> {
        let mut path = glib::user_data_dir();
        path.push(glib::uuid_string_random().as_str());

        let passphrase: String = {
            let mut rng = thread_rng();
            (&mut rng)
                .sample_iter(Alphanumeric)
                .take(30)
                .map(char::from)
                .collect()
        };

        let path_clone = path.clone();
        let handle = spawn_tokio!(async move {
            let client = create_client(
                &homeserver,
                path_clone.clone(),
                passphrase.clone(),
                use_discovery,
            )
            .await?;

            let response = client
                .login_username(&username, &password)
                .initial_device_display_name("Fractal")
                .send()
                .await?;

            Ok((
                client,
                StoredSession {
                    homeserver,
                    path: path_clone,
                    user_id: response.user_id,
                    device_id: response.device_id,
                    secret: Secret {
                        passphrase,
                        access_token: response.access_token,
                    },
                },
            ))
        });

        match handle.await.unwrap() {
            Ok((client, session)) => self.handle_login_result(client, session, true).await,
            Err(error) => {
                self.cleanup_files(path);
                Err(LoginError::Login(error))
            }
        }
    }

    pub async fn login_with_sso(
        &self,
        homeserver: Url,
        idp_id: Option<String>,
    ) -> Result<(), LoginError> {
        let mut path = glib::user_data_dir();
        path.push(glib::uuid_string_random().as_str());
        let passphrase: String = {
            let mut rng = thread_rng();
            (&mut rng)
                .sample_iter(Alphanumeric)
                .take(30)
                .map(char::from)
                .collect()
        };

        let path_clone = path.clone();
        let handle = spawn_tokio!(async move {
            let client =
                create_client(&homeserver, path_clone.clone(), passphrase.clone(), true).await?;

            let mut login = client
                .login_sso(|sso_url| async move {
                    let ctx = glib::MainContext::default();
                    ctx.spawn(async move {
                        gtk::show_uri(gtk::Window::NONE, &sso_url, gdk::CURRENT_TIME);
                    });
                    Ok(())
                })
                .initial_device_display_name("Fractal");

            if let Some(idp_id) = idp_id.as_deref() {
                login = login.identity_provider_id(idp_id);
            }

            let response = login.send().await?;

            Ok((
                client,
                StoredSession {
                    homeserver,
                    path: path_clone,
                    user_id: response.user_id,
                    device_id: response.device_id,
                    secret: Secret {
                        passphrase,
                        access_token: response.access_token,
                    },
                },
            ))
        });

        match handle.await.unwrap() {
            Ok((client, session)) => self.handle_login_result(client, session, true).await,
            Err(error) => {
                self.cleanup_files(&path);
                Err(LoginError::Login(error))
            }
        }
    }

    pub async fn login_with_previous_session(
        &self,
        session: StoredSession,
    ) -> Result<(), LoginError> {
        let path = session.path.clone();
        let handle = spawn_tokio!(async move {
            let client = create_client(
                &session.homeserver,
                session.path.clone(),
                session.secret.passphrase.clone(),
                false,
            )
            .await?;

            client
                .restore_login(matrix_sdk::Session {
                    user_id: session.user_id.clone(),
                    device_id: session.device_id.clone(),
                    access_token: session.secret.access_token.clone(),
                    refresh_token: None,
                })
                .await?;

            let was_synced = client.sync_token().await.is_some();

            Ok((client, session, was_synced))
        });

        match handle.await.unwrap() {
            Ok((client, session, was_synced)) => {
                self.imp().was_synced.set(was_synced);
                self.handle_login_result(client, session, true).await
            }
            Err(error) => {
                self.cleanup_files(path);
                Err(LoginError::RestoreSession(error))
            }
        }
    }

    async fn handle_login_result(
        &self,
        client: Client,
        session: StoredSession,
        store_session: bool,
    ) -> Result<(), LoginError> {
        let priv_ = self.imp();

        if store_session {
            if let Err(error) = secret::store_session(&session).await {
                // Destroy the created session
                self.cleanup_token(client).await;
                self.cleanup_files(session.path);

                return Err(LoginError::StoreSession(error));
            }
        };

        priv_.client.set(client).unwrap();
        let user = User::new(self, &session.user_id);
        priv_.user.set(user).unwrap();
        self.notify("user");

        self.update_user_profile();

        priv_.info.set(session).unwrap();

        self.room_list().load();
        self.setup_direct_room_handler();
        self.setup_room_encrypted_changes();

        self.set_state(State::LoggedIn);

        self.sync();

        if priv_.was_synced.get() {
            self.show_content().await;
        }

        Ok(())
    }

    fn sync(&self) {
        if self.state() < State::LoggedIn || self.state() >= State::LoggedOut || self.is_offline() {
            return;
        }

        let client = self.client();
        let session_weak: glib::SendWeakRef<Session> = self.downgrade().into();

        let handle = spawn_tokio!(async move {
            let sync_token = client.sync_token().await;
            if sync_token.is_none() {
                debug!("Proceeding to initial sync…");
            }

            // TODO: only create the filter once and reuse it in the future
            let room_event_filter = assign!(RoomEventFilter::default(), {
                lazy_load_options: LazyLoadOptions::Enabled {include_redundant_members: false},
            });
            let filter = assign!(FilterDefinition::default(), {
                room: assign!(RoomFilter::empty(), {
                    include_leave: true,
                    state: room_event_filter,
                }),
            });

            let sync_settings = SyncSettings::new()
                .timeout(Duration::from_secs(30))
                .filter(filter.into());

            let mut sync_stream = Box::pin(client.sync_stream(sync_settings).await);
            while let Some(response) = sync_stream.next().await {
                let session_weak = session_weak.clone();
                let ctx = glib::MainContext::default();
                ctx.spawn(async move {
                    let session = session_weak
                        .upgrade()
                        .expect("Session doesn't exist anymore");
                    session.handle_sync_response(response);
                });
            }
        });

        self.imp().sync_tokio_handle.replace(Some(handle));
    }

    async fn setup_cross_signing(&self) -> bool {
        let client = self.client();
        let encryption = client.encryption();
        let user_id = self.user().unwrap().user_id();

        if !self.has_cross_signing_keys().await {
            self.set_state(State::CrossSigningSetup);

            let need_new_identity = spawn_tokio!(async move {
                // If there is an error just assume we don't need a new identity since
                // we will try again during the session verification
                encryption
                    .get_user_identity(&user_id)
                    .await
                    .map_or(false, |identity| identity.is_none())
            });

            if need_new_identity.await.unwrap() {
                let encryption = self.client().encryption();

                // TODO: ask user if we should create a new identity for them
                // TODO: handle errors
                let handle =
                    spawn_tokio!(async move { encryption.bootstrap_cross_signing(None).await });
                if handle.await.unwrap().is_ok() {
                    return false;
                }
            }

            let widget = SessionVerification::new(self);
            self.imp()
                .stack
                .add_named(&widget, Some("session-verification"));
            self.imp().stack.set_visible_child(&widget);

            true
        } else {
            false
        }
    }

    pub fn room_list(&self) -> &RoomList {
        self.item_list().room_list()
    }

    pub fn verification_list(&self) -> &VerificationList {
        self.item_list().verification_list()
    }

    pub fn item_list(&self) -> &ItemList {
        self.imp()
            .item_list
            .get_or_init(|| ItemList::new(&RoomList::new(self), &VerificationList::new(self)))
    }

    /// The user of this session.
    pub fn user(&self) -> Option<&User> {
        self.imp().user.get()
    }

    /// Update the profile of this session’s user.
    ///
    /// Fetches the updated profile and updates the local data.
    pub fn update_user_profile(&self) {
        let client = self.client();
        let user = self.user().unwrap().to_owned();

        let handle = spawn_tokio!(async move { client.account().get_profile().await });

        spawn!(glib::PRIORITY_LOW, async move {
            match handle.await.unwrap() {
                Ok(res) => {
                    user.set_display_name(res.displayname);
                    user.set_avatar_url(res.avatar_url);
                }
                Err(error) => error!("Couldn’t fetch account metadata: {}", error),
            }
        });
    }

    pub fn client(&self) -> Client {
        self.imp()
            .client
            .get()
            .expect("The session isn't ready")
            .clone()
    }

    pub fn is_offline(&self) -> bool {
        self.imp().offline.get()
    }

    async fn update_offline(&self) {
        let priv_ = self.imp();
        let monitor = gio::NetworkMonitor::default();

        let is_offline = if monitor.is_network_available() {
            if let Some(info) = priv_.info.get() {
                !check_if_reachable(&info.homeserver).await
            } else {
                false
            }
        } else {
            true
        };

        if self.is_offline() == is_offline {
            return;
        }

        if is_offline {
            debug!("This session is now offline");
        } else {
            debug!("This session is now online");
        }

        priv_.offline.set(is_offline);

        if let Some(handle) = priv_.sync_tokio_handle.take() {
            handle.abort();
        }

        // Restart the sync loop when online
        self.sync();

        self.notify("offline");
    }

    pub fn state(&self) -> State {
        self.imp().state.get()
    }

    fn set_state(&self, state: State) {
        if self.state() == state {
            return;
        }

        self.imp().state.set(state);
        self.notify("state");
    }

    fn handle_sync_response(&self, response: Result<SyncResponse, matrix_sdk::Error>) {
        debug!("Received sync response");
        match response {
            Ok(response) => {
                self.room_list().handle_response_rooms(response.rooms);
                self.verification_list()
                    .handle_response_to_device(response.to_device);

                if !self.imp().was_synced.get() {
                    self.imp().was_synced.set(true);
                    spawn!(clone!(@weak self as obj => async move {
                        obj.show_content().await;
                    }));
                }
            }
            Err(error) => {
                if let matrix_sdk::Error::Http(HttpError::Api(FromHttpResponseError::Server(
                    ServerError::Known(RumaApiError::ClientApi(ref error)),
                ))) = error
                {
                    if let ErrorKind::UnknownToken { soft_logout: _ } = error.kind {
                        spawn!(clone!(@strong self as obj => async move {
                          obj.cleanup_session().await;
                        }));
                    }
                }
                error!("Failed to perform sync: {:?}", error);
            }
        }
    }

    /// Returns the parent GtkWindow containing this widget.
    fn parent_window(&self) -> Option<Window> {
        self.root()?.downcast().ok()
    }

    fn open_account_settings(&self) {
        let window = AccountSettings::new(self.parent_window().as_ref(), self);
        window.show();
    }

    fn show_room_creation_dialog(&self) {
        let window = RoomCreation::new(self.parent_window().as_ref(), self);
        window.show();
    }

    fn logout(&self) {
        spawn!(clone!(@strong self as obj => async move {
            obj.cleanup_token(obj.client()).await;
            obj.cleanup_session().await;
        }));
    }

    async fn cleanup_session(&self) {
        let priv_ = self.imp();
        debug!("The session is about to be logged out");

        // First stop the verification in progress
        if let Some(session_verification) = priv_.stack.child_by_name("session-verification") {
            priv_.stack.remove(&session_verification);
        }

        if let Some(handle) = priv_.sync_tokio_handle.take() {
            handle.abort();
        }

        let info = priv_.info.get().expect("Session info needs to be set");
        if let Err(error) = secret::remove_session(info).await {
            error!(
                "Failed to remove credentials from SecretService after logout: {}",
                error.to_user_facing()
            );
        }

        self.cleanup_files(&info.path);

        debug!("Finished loggoing out the session");
    }

    fn cleanup_files<P: AsRef<Path>>(&self, path: P) {
        if let Err(error) = fs::remove_dir_all(path) {
            error!("Failed to remove database after logout: {}", error);
            toast!(self, gettext("Failed to remove database during logout"));
        }
        self.set_state(State::Cleaned);
    }

    async fn cleanup_token(&self, client: Client) {
        let handle = spawn_tokio!(async move {
            let request = logout::v3::Request::new();
            client.send(request, None).await
        });

        if let Err(error) = handle.await.unwrap() {
            error!("Couldn’t logout the session {}", error);
            toast!(self, gettext("Failed to logout the session."));
        }

        self.set_state(State::LoggedOut)
    }

    pub fn handle_paste_action(&self) {
        self.imp().content.handle_paste_action();
    }

    /// Show the content of the session
    pub async fn show_content(&self) {
        let priv_ = self.imp();

        // This wont' do anything if the keys are already available
        if self.setup_cross_signing().await {
            return;
        }

        priv_.stack.set_visible_child(&*priv_.leaflet);

        if let Some(session_verificiation) = priv_.stack.child_by_name("session-verification") {
            priv_.stack.remove(&session_verificiation);
        }

        self.set_state(State::Content);
    }

    /// Show a media event
    pub fn show_media(&self, event: &SupportedEvent) {
        let priv_ = self.imp();
        priv_.media_viewer.set_event(Some(event.clone()));

        priv_.stack.set_visible_child(&*priv_.media_viewer);
    }

    async fn has_cross_signing_keys(&self) -> bool {
        let encryption = self.client().encryption();
        spawn_tokio!(async move {
            if let Some(cross_signing_status) = encryption.cross_signing_status().await {
                debug!("Cross signing keys status: {:?}", cross_signing_status);
                cross_signing_status.has_self_signing && cross_signing_status.has_user_signing
            } else {
                debug!("Session doesn't have needed cross signing keys");
                false
            }
        })
        .await
        .unwrap()
    }

    fn setup_direct_room_handler(&self) {
        spawn!(
            glib::PRIORITY_DEFAULT_IDLE,
            clone!(@weak self as obj => async move {
                let obj_weak = glib::SendWeakRef::from(obj.downgrade());
                obj.client().add_event_handler(
                    move |event: GlobalAccountDataEvent<DirectEventContent>| {
                        let obj_weak = obj_weak.clone();
                        async move {
                            let ctx = glib::MainContext::default();
                            ctx.spawn(async move {
                                spawn!(async move {
                                    if let Some(session) = obj_weak.upgrade() {
                                        let room_ids = event.content.iter().fold(HashSet::new(), |mut acc, (_, rooms)| {
                                            acc.extend(rooms);
                                            acc
                                        });
                                        for room_id in room_ids {
                                            if let Some(room) = session.room_list().get(room_id) {
                                                room.load_category();
                                            }
                                        }
                                    }
                                });
                            });
                        }
                    },
                );
            })
        );
    }

    fn setup_room_encrypted_changes(&self) {
        let session_weak = glib::SendWeakRef::from(self.downgrade());
        let client = self.client();
        spawn_tokio!(async move {
            client.add_event_handler(move |_: SyncRoomEncryptionEvent, matrix_room: MatrixRoom| {
                let session_weak = session_weak.clone();
                async move {
                    let ctx = glib::MainContext::default();
                    ctx.spawn(async move {
                        if let Some(session) = session_weak.upgrade() {
                            if let Some(room) = session.room_list().get(matrix_room.room_id()) {
                                room.set_is_encrypted(true);
                            }
                        }
                    });
                }
            });
        });
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

async fn create_client(
    homeserver: &Url,
    path: PathBuf,
    passphrase: String,
    use_discovery: bool,
) -> Result<Client, ClientSetupError> {
    let state_store = SledStateStore::builder()
        .path(path)
        .passphrase(passphrase)
        .migration_conflict_strategy(MigrationConflictStrategy::Drop)
        .build()
        .map_err(|err| OpenStoreError::from(StoreError::backend(err)))?;
    let crypto_store = state_store.open_crypto_store()?;
    let store_config = StoreConfig::new()
        .state_store(state_store)
        .crypto_store(crypto_store);
    Client::builder()
        .homeserver_url(homeserver)
        .store_config(store_config)
        // force_auth option to solve an issue with some servers configuration to require
        // auth for profiles:
        // https://gitlab.gnome.org/GNOME/fractal/-/issues/934
        .request_config(RequestConfig::new().retry_limit(2).force_auth())
        .respect_login_well_known(use_discovery)
        .build()
        .await
        .map_err(Into::into)
}
