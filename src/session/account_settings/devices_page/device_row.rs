use adw::{self, prelude::*};
use gettextrs::gettext;
use gtk::{glib, glib::clone, subclass::prelude::*, CompositeTemplate};
use log::error;

use super::Device;
use crate::{
    components::{AuthError, SpinnerButton},
    gettext_f, spawn, toast,
};

mod imp {
    use std::cell::{Cell, RefCell};

    use glib::subclass::InitializingObject;
    use once_cell::sync::Lazy;

    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/org/gnome/Fractal/account-settings-device-row.ui")]
    pub struct DeviceRow {
        #[template_child]
        pub display_name: TemplateChild<gtk::Label>,
        #[template_child]
        pub verified_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub last_seen_ip: TemplateChild<gtk::Label>,
        #[template_child]
        pub last_seen_ts: TemplateChild<gtk::Label>,
        #[template_child]
        pub delete_logout_button: TemplateChild<SpinnerButton>,
        #[template_child]
        pub verify_button: TemplateChild<SpinnerButton>,
        pub device: RefCell<Option<Device>>,
        pub is_current_device: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DeviceRow {
        const NAME: &'static str = "AccountSettingsDeviceRow";
        type Type = super::DeviceRow;
        type ParentType = gtk::ListBoxRow;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DeviceRow {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: Lazy<Vec<glib::ParamSpec>> = Lazy::new(|| {
                vec![
                    glib::ParamSpecObject::new(
                        "device",
                        "Device",
                        "The device this row is showing",
                        Device::static_type(),
                        glib::ParamFlags::READWRITE | glib::ParamFlags::CONSTRUCT_ONLY,
                    ),
                    glib::ParamSpecBoolean::new(
                        "is-current-device",
                        "Is Current Device",
                        "Whether this is the device of the current session",
                        false,
                        glib::ParamFlags::READWRITE | glib::ParamFlags::CONSTRUCT_ONLY,
                    ),
                ]
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
                "device" => {
                    obj.set_device(value.get().unwrap());
                }
                "is-current-device" => {
                    obj.set_current_device(value.get().unwrap());
                }
                _ => unimplemented!(),
            }
        }

        fn property(&self, obj: &Self::Type, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "device" => obj.device().to_value(),
                "is-current-device" => obj.is_current_device().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self, obj: &Self::Type) {
            self.parent_constructed(obj);

            match &obj.is_current_device() {
                false => self
                    .delete_logout_button
                    .set_label(&gettext("Disconnect Session")),
                true => {
                    self.delete_logout_button.set_label(&gettext("Log Out"));
                    self.delete_logout_button
                        .add_css_class("destructive-action");
                }
            }

            self.delete_logout_button
                .connect_clicked(clone!(@weak obj => move |_| {
                    match &obj.is_current_device() {
                        false=> obj.delete(),
                        true => obj.confirm_logout()
                    }
                }));

            self.verify_button
                .connect_clicked(clone!(@weak obj => move |_| {
                    todo!("Not implemented");
                }));
        }
    }
    impl WidgetImpl for DeviceRow {}
    impl ListBoxRowImpl for DeviceRow {}
}

glib::wrapper! {
    pub struct DeviceRow(ObjectSubclass<imp::DeviceRow>)
        @extends gtk::Widget, gtk::ListBoxRow, @implements gtk::Accessible;
}

impl DeviceRow {
    pub fn new(device: &Device, is_current_device: bool) -> Self {
        glib::Object::new(&[
            ("device", device),
            ("is-current-device", &is_current_device),
        ])
        .expect("Failed to create DeviceRow")
    }

    pub fn device(&self) -> Option<Device> {
        self.imp().device.borrow().clone()
    }

    pub fn set_device(&self, device: Option<Device>) {
        let priv_ = self.imp();

        if self.device() == device {
            return;
        }

        if let Some(ref device) = device {
            priv_.display_name.set_label(device.display_name());
            self.set_tooltip_text(Some(device.device_id().as_str()));

            priv_.verified_icon.set_visible(device.is_verified());
            // TODO: Implement verification
            // priv_.verify_button.set_visible(!device.is_verified());

            if let Some(last_seen_ip) = device.last_seen_ip() {
                priv_.last_seen_ip.set_label(last_seen_ip);
                priv_.last_seen_ip.show();
            } else {
                priv_.last_seen_ip.hide();
            }

            if let Some(last_seen_ts) = device.last_seen_ts() {
                let last_seen_ts = format_date_time_as_string(last_seen_ts);
                priv_.last_seen_ts.set_label(&last_seen_ts);
                priv_.last_seen_ts.show();
            } else {
                priv_.last_seen_ts.hide();
            }
        }

        priv_.device.replace(device);
        self.notify("device");
    }

    fn set_current_device(&self, input_bool: bool) {
        let priv_ = self.imp();
        if priv_.is_current_device.get() == input_bool {
            return;
        }
        priv_.is_current_device.replace(input_bool);
        self.notify("is-current-device");
    }

    pub fn is_current_device(&self) -> bool {
        self.imp().is_current_device.get()
    }

    fn delete(&self) {
        self.imp().delete_logout_button.set_loading(true);

        if let Some(device) = self.device() {
            spawn!(clone!(@weak self as obj => async move {
                let window: Option<gtk::Window> = obj.root().and_then(|root| root.downcast().ok());
                match device.delete(window.as_ref()).await {
                    Ok(_) => obj.hide(),
                    Err(AuthError::UserCancelled) => {},
                    Err(err) => {
                        error!("Failed to disconnect device {}: {err:?}", device.device_id());
                        let device_name = device.display_name();
                        // Translators: Do NOT translate the content between '{' and '}', this is a variable name.
                        let error_message = gettext_f("Failed to disconnect device “{device_name}”", &[("device_name", device_name)]);
                        toast!(obj, error_message);
                    },
                }
                obj.imp().delete_logout_button.set_loading(false);
            }));
        }
    }

    fn confirm_logout(&self) {
        self.imp().delete_logout_button.set_loading(true);

        let window: Option<gtk::Window> = self.root().and_then(|root| root.downcast().ok());
        let dialog = gtk::MessageDialog::new(window.as_ref(), gtk::DialogFlags::MODAL, gtk::MessageType::Info, gtk::ButtonsType::OkCancel, &gettext("Fractal doesn't support online backup of room encryption keys so you might lose access to your encrypted message history. It is recommended to backup your encryption keys before proceeding."));
        dialog.show();
        dialog.connect_response(
            clone!(@weak self as obj, @weak dialog => move |_, response| {
                match &response {
                    gtk::ResponseType::Ok => {
                        obj.logout();
                    }
                    _ => {
                        obj.imp().delete_logout_button.set_loading(false);
                    }
                }
                dialog.destroy();
            }),
        );
    }

    fn logout(&self) {
        if let Some(device) = self.device() {
            device.session().logout()
        }
    }
}

// This was ported from Nautilus and simplified for our use case.
// See: https://gitlab.gnome.org/GNOME/nautilus/-/blob/master/src/nautilus-file.c#L5488
pub fn format_date_time_as_string(datetime: glib::DateTime) -> glib::GString {
    let now = glib::DateTime::now_local().unwrap();
    let format;
    let days_ago = {
        let today_midnight =
            glib::DateTime::from_local(now.year(), now.month(), now.day_of_month(), 0, 0, 0f64)
                .unwrap();

        let date = glib::DateTime::from_local(
            datetime.year(),
            datetime.month(),
            datetime.day_of_month(),
            0,
            0,
            0f64,
        )
        .unwrap();

        today_midnight.difference(&date).as_days()
    };

    let use_24 = {
        let local_time = datetime.format("%X").unwrap().as_str().to_ascii_lowercase();
        local_time.ends_with("am") || local_time.ends_with("pm")
    };

    // Show only the time if date is on today
    if days_ago == 0 {
        if use_24 {
            // Translators: Time in 24h format
            format = gettext("Last seen at %H:%M");
        } else {
            // Translators: Time in 12h format
            format = gettext("Last seen at %l:%M %p");
        }
    }
    // Show the word "Yesterday" and time if date is on yesterday
    else if days_ago == 1 {
        if use_24 {
            // Translators: this is the word Yesterday followed by
            // a time in 24h format. i.e. "Last seen Yesterday at 23:04"
            // xgettext:no-c-format
            format = gettext("Last seen Yesterday at %H:%M");
        } else {
            // Translators: this is the word Yesterday followed by
            // a time in 12h format. i.e. "Last seen Yesterday at 9:04 PM"
            // xgettext:no-c-format
            format = gettext("Last seen Yesterday at %l:%M %p");
        }
    }
    // Show a week day and time if date is in the last week
    else if days_ago > 1 && days_ago < 7 {
        if use_24 {
            // Translators: this is the name of the week day followed by
            // a time in 24h format. i.e. "Last seen Monday at 23:04"
            // xgettext:no-c-format
            format = gettext("Last seen %A at %H:%M");
        } else {
            // Translators: this is the week day name followed by
            // a time in 12h format. i.e. "Last seen Monday at 9:04 PM"
            // xgettext:no-c-format
            format = gettext("Last seen %A at %l:%M %p");
        }
    } else if datetime.year() == now.year() {
        if use_24 {
            // Translators: this is the day of the month followed
            // by the abbreviated month name followed by a time in
            // 24h format i.e. "Last seen February 3 at 23:04"
            // xgettext:no-c-format
            format = gettext("Last seen %B %-e at %H:%M");
        } else {
            // Translators: this is the day of the month followed
            // by the abbreviated month name followed by a time in
            // 12h format i.e. "Last seen February 3 at 9:04 PM"
            // xgettext:no-c-format
            format = gettext("Last seen %B %-e at %l:%M %p");
        }
    } else if use_24 {
        // Translators: this is the day number followed
        // by the abbreviated month name followed by the year followed
        // by a time in 24h format i.e. "Last seen February 3 2015 at 23:04"
        // xgettext:no-c-format
        format = gettext("Last seen %B %-e %Y at %H:%M");
    } else {
        // Translators: this is the day number followed
        // by the abbreviated month name followed by the year followed
        // by a time in 12h format i.e. "Last seen February 3 2015 at 9:04 PM"
        // xgettext:no-c-format
        format = gettext("Last seen %B %-e %Y at %l:%M %p");
    }

    datetime.format(&format).unwrap()
}
