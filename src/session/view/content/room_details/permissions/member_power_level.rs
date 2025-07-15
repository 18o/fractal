use adw::subclass::prelude::*;
use gtk::{glib, glib::clone, prelude::*};
use ruma::{Int, OwnedUserId, events::room::power_levels::PowerLevelUserAction};

use crate::{
    prelude::*,
    session::model::{MemberRole, POWER_LEVEL_MAX, POWER_LEVEL_MIN, Permissions, PowerLevel, User},
    utils::BoundObjectWeakRef,
};

mod imp {
    use std::cell::{Cell, OnceCell};

    use super::*;

    #[derive(Debug, Default, glib::Properties)]
    #[properties(wrapper_type = super::MemberPowerLevel)]
    pub struct MemberPowerLevel {
        /// The permissions to watch.
        #[property(get, set = Self::set_permissions, construct_only)]
        permissions: BoundObjectWeakRef<Permissions>,
        /// The room member or remote user.
        #[property(get, construct_only)]
        user: OnceCell<User>,
        /// The wanted power level of the member.
        ///
        /// Initially, it should be the same as the member's, but can change
        /// independently.
        #[property(get, set = Self::set_power_level, explicit_notify,  minimum = POWER_LEVEL_MIN, maximum = POWER_LEVEL_MAX)]
        power_level: Cell<PowerLevel>,
        /// The wanted role of the member.
        #[property(get, builder(MemberRole::default()))]
        role: Cell<MemberRole>,
        /// Whether this member's power level can be edited.
        #[property(get)]
        editable: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MemberPowerLevel {
        const NAME: &'static str = "RoomDetailsPermissionsMemberPowerLevel";
        type Type = super::MemberPowerLevel;
    }

    #[glib::derived_properties]
    impl ObjectImpl for MemberPowerLevel {
        fn constructed(&self) {
            self.parent_constructed();

            self.update_power_level();
            self.update_role();
            self.update_editable();
        }
    }

    impl MemberPowerLevel {
        /// Set the room member.
        fn set_permissions(&self, permissions: &Permissions) {
            let changed_handler = permissions.connect_changed(clone!(
                #[weak(rename_to = imp)]
                self,
                move |_| {
                    imp.update_power_level();
                    imp.update_role();
                    imp.update_editable();
                }
            ));
            self.permissions.set(permissions, vec![changed_handler]);
        }

        /// Update the wanted power level of the member.
        fn update_power_level(&self) {
            let Some(user) = self.user.get() else {
                return;
            };
            let Some(permissions) = self.permissions.obj() else {
                return;
            };

            self.set_power_level(permissions.user_power_level(user.user_id()));
        }

        /// Set the wanted power level of the member.
        fn set_power_level(&self, power_level: PowerLevel) {
            if self.power_level.get() == power_level {
                return;
            }

            self.power_level.set(power_level);
            self.update_role();
            self.obj().notify_power_level();
        }

        /// Update the wanted role of the member.
        fn update_role(&self) {
            let Some(permissions) = self.permissions.obj() else {
                return;
            };

            let role = permissions.role(self.power_level.get());

            if self.role.get() == role {
                return;
            }

            self.role.set(role);
            self.obj().notify_role();
        }

        /// Update whether this member's power level can be edited.
        fn update_editable(&self) {
            let Some(user) = self.user.get() else {
                return;
            };
            let Some(permissions) = self.permissions.obj() else {
                return;
            };

            let editable =
                permissions.can_do_to_user(user.user_id(), PowerLevelUserAction::ChangePowerLevel);

            if self.editable.get() == editable {
                return;
            }

            self.editable.set(editable);
            self.obj().notify_editable();
        }
    }
}

glib::wrapper! {
    /// A room member with a cached wanted power level.
    pub struct MemberPowerLevel(ObjectSubclass<imp::MemberPowerLevel>);
}

impl MemberPowerLevel {
    /// Constructs a new `MemberPowerLevel` with the given user and permissions.
    pub fn new(user: &impl IsA<User>, permissions: &Permissions) -> Self {
        glib::Object::builder()
            .property("user", user)
            .property("permissions", permissions)
            .build()
    }

    /// Get the parts of this member, to use in the power levels event.
    ///
    /// Returns `None` if the permissions could not be upgraded, or if the power
    /// level is the users default.
    pub(crate) fn to_parts(&self) -> Option<(OwnedUserId, Int)> {
        let permissions = self.permissions()?;

        let users_default = permissions.default_power_level();
        let pl = self.power_level();

        if pl == users_default {
            return None;
        }

        Some((self.user().user_id().clone(), Int::new_saturating(pl)))
    }

    /// The string to use to search for this member.
    pub(crate) fn search_string(&self) -> String {
        let user = self.user();
        format!(
            "{} {} {} {}",
            user.display_name(),
            user.user_id(),
            self.role(),
            self.power_level(),
        )
    }
}
