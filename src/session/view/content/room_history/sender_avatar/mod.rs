use std::slice;

use adw::{prelude::*, subclass::prelude::*};
use gettextrs::{gettext, ngettext};
use gtk::{CompositeTemplate, gdk, glib, glib::clone};
use ruma::{OwnedEventId, events::room::power_levels::PowerLevelUserAction};

use crate::{
    Window,
    components::{
        Avatar, RoomMemberDestructiveAction, UserProfileDialog, confirm_mute_room_member_dialog,
        confirm_room_member_destructive_action_dialog,
    },
    gettext_f,
    prelude::*,
    session::{
        model::{Member, MemberRole, Membership, User},
        view::content::RoomHistory,
    },
    toast,
    utils::{BoundObject, key_bindings},
};

mod imp {
    use std::cell::{Cell, RefCell};

    use glib::subclass::InitializingObject;

    use super::*;

    #[derive(Debug, Default, CompositeTemplate, glib::Properties)]
    #[template(
        resource = "/org/gnome/Fractal/ui/session/view/content/room_history/sender_avatar/mod.ui"
    )]
    #[properties(wrapper_type = super::SenderAvatar)]
    pub struct SenderAvatar {
        #[template_child]
        avatar: TemplateChild<Avatar>,
        #[template_child]
        user_id_btn: TemplateChild<gtk::Button>,
        /// Whether this avatar is active.
        ///
        /// This avatar is active when the popover is displayed.
        #[property(get)]
        active: Cell<bool>,
        direct_member_handler: RefCell<Option<glib::SignalHandlerId>>,
        permissions_handler: RefCell<Option<glib::SignalHandlerId>>,
        /// The displayed member.
        #[property(get, set = Self::set_sender, explicit_notify, nullable)]
        sender: BoundObject<Member>,
        /// The popover of this avatar.
        popover: BoundObject<gtk::PopoverMenu>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SenderAvatar {
        const NAME: &'static str = "ContentSenderAvatar";
        type Type = super::SenderAvatar;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
            Self::bind_template_callbacks(klass);

            klass.set_layout_manager_type::<gtk::BinLayout>();
            klass.set_css_name("sender-avatar");
            klass.set_accessible_role(gtk::AccessibleRole::ToggleButton);

            klass.install_action("sender-avatar.copy-user-id", None, |obj, _, _| {
                if let Some(popover) = obj.imp().popover.obj() {
                    popover.popdown();
                }

                let Some(sender) = obj.sender() else {
                    return;
                };

                obj.clipboard().set_text(sender.user_id().as_str());
                toast!(obj, gettext("Matrix user ID copied to clipboard"));
            });

            klass.install_action("sender-avatar.mention", None, |obj, _, _| {
                obj.imp().mention();
            });

            klass.install_action_async(
                "sender-avatar.open-direct-chat",
                None,
                |obj, _, _| async move {
                    obj.imp().open_direct_chat().await;
                },
            );

            klass.install_action("sender-avatar.permalink", None, |obj, _, _| {
                let Some(sender) = obj.sender() else {
                    return;
                };

                obj.clipboard()
                    .set_text(&sender.matrix_to_uri().to_string());
                toast!(obj, gettext("Link copied to clipboard"));
            });

            klass.install_action_async("sender-avatar.invite", None, |obj, _, _| async move {
                obj.imp().invite().await;
            });

            klass.install_action_async(
                "sender-avatar.revoke-invite",
                None,
                |obj, _, _| async move {
                    obj.imp().kick().await;
                },
            );

            klass.install_action_async("sender-avatar.mute", None, |obj, _, _| async move {
                obj.imp().toggle_muted().await;
            });

            klass.install_action_async("sender-avatar.unmute", None, |obj, _, _| async move {
                obj.imp().toggle_muted().await;
            });

            klass.install_action_async("sender-avatar.kick", None, |obj, _, _| async move {
                obj.imp().kick().await;
            });

            klass.install_action_async("sender-avatar.deny-access", None, |obj, _, _| async move {
                obj.imp().kick().await;
            });

            klass.install_action_async("sender-avatar.ban", None, |obj, _, _| async move {
                obj.imp().ban().await;
            });

            klass.install_action_async("sender-avatar.unban", None, |obj, _, _| async move {
                obj.imp().unban().await;
            });

            klass.install_action_async(
                "sender-avatar.remove-messages",
                None,
                |obj, _, _| async move {
                    obj.imp().remove_messages().await;
                },
            );

            klass.install_action_async("sender-avatar.ignore", None, |obj, _, _| async move {
                obj.imp().toggle_ignored().await;
            });

            klass.install_action_async(
                "sender-avatar.stop-ignoring",
                None,
                |obj, _, _| async move {
                    obj.imp().toggle_ignored().await;
                },
            );

            klass.install_action("sender-avatar.view-details", None, |obj, _, _| {
                obj.imp().view_details();
            });

            klass.install_action("sender-avatar.activate", None, |obj, _, _| {
                obj.imp().show_popover(1, 0.0, 0.0);
            });

            key_bindings::add_activate_bindings(klass, "sender-avatar.activate");
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for SenderAvatar {
        fn constructed(&self) {
            self.parent_constructed();

            self.set_pressed_state(false);
        }

        fn dispose(&self) {
            self.disconnect_signals();

            if let Some(popover) = self.popover.obj() {
                popover.unparent();
                popover.remove_child(&*self.user_id_btn);
            }

            self.avatar.unparent();
        }
    }

    impl WidgetImpl for SenderAvatar {}

    impl AccessibleImpl for SenderAvatar {
        fn first_accessible_child(&self) -> Option<gtk::Accessible> {
            // Hide the children in the a11y tree.
            None
        }
    }

    #[gtk::template_callbacks]
    impl SenderAvatar {
        /// Set the list of room members.
        fn set_sender(&self, sender: Option<Member>) {
            let prev_sender = self.sender.obj();

            if prev_sender == sender {
                return;
            }

            self.disconnect_signals();

            if let Some(sender) = sender {
                let room = sender.room();
                let direct_member_handler = room.connect_direct_member_notify(clone!(
                    #[weak(rename_to = imp)]
                    self,
                    move |_| {
                        imp.update_actions();
                    }
                ));
                self.direct_member_handler
                    .replace(Some(direct_member_handler));

                let permissions_handler = room.permissions().connect_changed(clone!(
                    #[weak(rename_to = imp)]
                    self,
                    move |_| {
                        imp.update_actions();
                    }
                ));
                self.permissions_handler.replace(Some(permissions_handler));

                let display_name_handler = sender.connect_display_name_notify(clone!(
                    #[weak(rename_to = imp)]
                    self,
                    move |_| {
                        imp.update_accessible_label();
                    }
                ));

                let membership_handler = sender.connect_membership_notify(clone!(
                    #[weak(rename_to = imp)]
                    self,
                    move |_| {
                        imp.update_actions();
                    }
                ));

                let power_level_handler = sender.connect_power_level_notify(clone!(
                    #[weak(rename_to = imp)]
                    self,
                    move |_| {
                        imp.update_actions();
                    }
                ));

                let is_ignored_handler = sender.connect_is_ignored_notify(clone!(
                    #[weak(rename_to = imp)]
                    self,
                    move |_| {
                        imp.update_actions();
                    }
                ));

                self.sender.set(
                    sender,
                    vec![
                        display_name_handler,
                        membership_handler,
                        power_level_handler,
                        is_ignored_handler,
                    ],
                );
                self.update_accessible_label();
                self.update_actions();
            }

            self.obj().notify_sender();
        }

        /// Disconnect all the signals.
        fn disconnect_signals(&self) {
            if let Some(sender) = self.sender.obj() {
                let room = sender.room();

                if let Some(handler) = self.direct_member_handler.take() {
                    room.disconnect(handler);
                }
                if let Some(handler) = self.permissions_handler.take() {
                    room.permissions().disconnect(handler);
                }
            }

            self.sender.disconnect_signals();
        }

        /// Update the accessible label for the current sender.
        fn update_accessible_label(&self) {
            let Some(sender) = self.sender.obj() else {
                return;
            };

            let label = gettext_f("{user}’s avatar", &[("user", &sender.display_name())]);
            self.obj()
                .update_property(&[gtk::accessible::Property::Label(&label)]);
        }

        /// Update the actions for the current state.
        fn update_actions(&self) {
            let Some(sender) = self.sender.obj() else {
                return;
            };
            let obj = self.obj();

            let room = sender.room();
            let is_direct_chat = room.direct_member().is_some();
            let permissions = room.permissions();
            let membership = sender.membership();
            let sender_id = sender.user_id();
            let is_own_user = sender.is_own_user();
            let power_level = sender.power_level();
            let role = permissions.role(power_level);

            obj.action_set_enabled(
                "sender-avatar.mention",
                !is_own_user && membership == Membership::Join && permissions.can_send_message(),
            );

            obj.action_set_enabled(
                "sender-avatar.open-direct-chat",
                !is_direct_chat && !is_own_user,
            );

            obj.action_set_enabled(
                "sender-avatar.invite",
                !is_own_user
                    && matches!(membership, Membership::Leave | Membership::Knock)
                    && permissions.can_do_to_user(sender_id, PowerLevelUserAction::Kick),
            );

            obj.action_set_enabled(
                "sender-avatar.revoke-invite",
                !is_own_user
                    && membership == Membership::Invite
                    && permissions.can_do_to_user(sender_id, PowerLevelUserAction::Kick),
            );

            obj.action_set_enabled(
                "sender-avatar.mute",
                !is_own_user
                    && role != MemberRole::Muted
                    && permissions.default_power_level() > permissions.mute_power_level()
                    && permissions
                        .can_do_to_user(sender_id, PowerLevelUserAction::ChangePowerLevel),
            );

            obj.action_set_enabled(
                "sender-avatar.unmute",
                !is_own_user
                    && role == MemberRole::Muted
                    && permissions.default_power_level() > permissions.mute_power_level()
                    && permissions
                        .can_do_to_user(sender_id, PowerLevelUserAction::ChangePowerLevel),
            );

            obj.action_set_enabled(
                "sender-avatar.kick",
                !is_own_user
                    && membership == Membership::Join
                    && permissions.can_do_to_user(sender_id, PowerLevelUserAction::Kick),
            );

            obj.action_set_enabled(
                "sender-avatar.deny-access",
                !is_own_user
                    && membership == Membership::Knock
                    && permissions.can_do_to_user(sender_id, PowerLevelUserAction::Kick),
            );

            obj.action_set_enabled(
                "sender-avatar.ban",
                !is_own_user
                    && membership != Membership::Ban
                    && permissions.can_do_to_user(sender_id, PowerLevelUserAction::Ban),
            );

            obj.action_set_enabled(
                "sender-avatar.unban",
                !is_own_user
                    && membership == Membership::Ban
                    && permissions.can_do_to_user(sender_id, PowerLevelUserAction::Unban),
            );

            obj.action_set_enabled(
                "sender-avatar.remove-messages",
                !is_own_user && permissions.can_redact_other(),
            );

            obj.action_set_enabled("sender-avatar.ignore", !is_own_user && !sender.is_ignored());

            obj.action_set_enabled(
                "sender-avatar.stop-ignoring",
                !is_own_user && sender.is_ignored(),
            );
        }

        /// Set the popover of this avatar.
        fn set_popover(&self, popover: Option<gtk::PopoverMenu>) {
            let old_popover = self.popover.obj();

            if old_popover == popover {
                return;
            }

            // Reset the state.
            if let Some(popover) = old_popover {
                popover.unparent();
                popover.remove_child(&*self.user_id_btn);
            }
            self.popover.disconnect_signals();
            self.set_active(false);

            if let Some(popover) = popover {
                // We need to remove the popover from the previous button, if any.
                if popover.parent().is_some() {
                    popover.unparent();
                }

                let parent_handler = popover.connect_parent_notify(clone!(
                    #[weak(rename_to = imp)]
                    self,
                    move |popover| {
                        if popover.parent().is_none_or(|w| w != *imp.obj()) {
                            imp.popover.disconnect_signals();
                            popover.remove_child(&*imp.user_id_btn);
                        }
                    }
                ));
                let closed_handler = popover.connect_closed(clone!(
                    #[weak(rename_to = imp)]
                    self,
                    move |_| {
                        imp.set_active(false);
                    }
                ));

                popover.add_child(&*self.user_id_btn, "user-id");
                popover.set_parent(&*self.obj());

                self.popover
                    .set(popover, vec![parent_handler, closed_handler]);
            }
        }

        /// Set whether this avatar is active.
        fn set_active(&self, active: bool) {
            if self.active.get() == active {
                return;
            }

            self.active.set(active);

            self.obj().notify_active();
            self.set_pressed_state(active);
        }

        /// Set the CSS and a11 states.
        fn set_pressed_state(&self, pressed: bool) {
            let obj = self.obj();

            if pressed {
                obj.set_state_flags(gtk::StateFlags::CHECKED, false);
            } else {
                obj.unset_state_flags(gtk::StateFlags::CHECKED);
            }

            let tristate = if pressed {
                gtk::AccessibleTristate::True
            } else {
                gtk::AccessibleTristate::False
            };
            obj.update_state(&[gtk::accessible::State::Pressed(tristate)]);
        }

        /// The `RoomHistory` that is an ancestor of this avatar.
        fn room_history(&self) -> Option<RoomHistory> {
            self.obj()
                .ancestor(RoomHistory::static_type())
                .and_downcast()
        }

        /// Handle a click on the container.
        ///
        /// Shows a popover with the room member menu.
        #[template_callback]
        fn show_popover(&self, _n_press: i32, x: f64, y: f64) {
            let Some(room_history) = self.room_history() else {
                return;
            };

            self.set_active(true);

            let popover = room_history.sender_context_menu();
            self.set_popover(Some(popover.clone()));

            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 0, 0)));
            popover.popup();
        }

        /// Add a mention of the sender to the message composer.
        fn mention(&self) {
            let Some(sender) = self.sender.obj() else {
                return;
            };
            let Some(room_history) = self.room_history() else {
                return;
            };

            room_history.message_toolbar().mention_member(&sender);
        }

        /// View the sender details.
        fn view_details(&self) {
            let Some(sender) = self.sender.obj() else {
                return;
            };

            let dialog = UserProfileDialog::new();
            dialog.set_room_member(sender);
            dialog.present(Some(&*self.obj()));
        }

        /// Open a direct chat with the current sender.
        ///
        /// If one doesn't exist already, it is created.
        async fn open_direct_chat(&self) {
            let Some(sender) = self.sender.obj().and_upcast::<User>() else {
                return;
            };
            let obj = self.obj();

            let room = if let Some(room) = sender.direct_chat() {
                room
            } else {
                toast!(obj, &gettext("Creating a new Direct Chat…"));

                if let Ok(room) = sender.get_or_create_direct_chat().await {
                    room
                } else {
                    toast!(obj, &gettext("Could not create a new Direct Chat"));
                    return;
                }
            };

            let Some(main_window) = obj.root().and_downcast::<Window>() else {
                return;
            };

            main_window.session_view().select_room(room);
        }

        /// Invite the sender to the room.
        async fn invite(&self) {
            let Some(sender) = self.sender.obj() else {
                return;
            };
            let obj = self.obj();

            toast!(obj, gettext("Inviting user…"));

            let room = sender.room();
            let user_id = sender.user_id().clone();
            if room.invite(&[user_id]).await.is_err() {
                toast!(obj, gettext("Could not invite user"));
            }
        }

        /// Kick the user from the room.
        async fn kick(&self) {
            let Some(sender) = self.sender.obj() else {
                return;
            };
            let obj = self.obj();

            let Some(response) = confirm_room_member_destructive_action_dialog(
                &sender,
                RoomMemberDestructiveAction::Kick,
                &*obj,
            )
            .await
            else {
                return;
            };

            let membership = sender.membership();

            let label = match membership {
                Membership::Invite => gettext("Revoking invite…"),
                Membership::Knock => gettext("Denying access…"),
                _ => gettext("Kicking user…"),
            };
            toast!(obj, label);

            let room = sender.room();
            let user_id = sender.user_id().clone();
            if room.kick(&[(user_id, response.reason)]).await.is_err() {
                let error = match membership {
                    Membership::Invite => gettext("Could not revoke invite of user"),
                    Membership::Knock => gettext("Could not deny access to user"),
                    _ => gettext("Could not kick user"),
                };
                toast!(obj, error);
            }
        }

        /// (Un)mute the user in the room.
        async fn toggle_muted(&self) {
            let Some(sender) = self.sender.obj() else {
                return;
            };
            let obj = self.obj();

            let old_power_level = sender.power_level();
            let permissions = sender.room().permissions();

            // Warn if user is muted but was not before.
            let mute_power_level = permissions.mute_power_level();
            let mute = old_power_level > mute_power_level;
            if mute && !confirm_mute_room_member_dialog(slice::from_ref(&sender), &*obj).await {
                return;
            }

            let user_id = sender.user_id().clone();

            let (new_power_level, text) = if mute {
                (mute_power_level, gettext("Muting member…"))
            } else {
                (
                    permissions.default_power_level(),
                    gettext("Unmuting member…"),
                )
            };
            toast!(obj, text);

            let text = if permissions
                .set_user_power_level(user_id, new_power_level)
                .await
                .is_ok()
            {
                if mute {
                    gettext("Member muted")
                } else {
                    gettext("Member unmuted")
                }
            } else if mute {
                gettext("Could not mute member")
            } else {
                gettext("Could not unmute member")
            };
            toast!(obj, text);
        }

        /// Ban the room member.
        async fn ban(&self) {
            let Some(sender) = self.sender.obj() else {
                return;
            };
            let obj = self.obj();

            let permissions = sender.room().permissions();
            let redactable_events = if permissions.can_redact_other() {
                sender.redactable_events()
            } else {
                vec![]
            };

            let Some(response) = confirm_room_member_destructive_action_dialog(
                &sender,
                RoomMemberDestructiveAction::Ban(redactable_events.len()),
                &*obj,
            )
            .await
            else {
                return;
            };

            toast!(obj, gettext("Banning user…"));

            let room = sender.room();
            let user_id = sender.user_id().clone();
            if room
                .ban(&[(user_id, response.reason.clone())])
                .await
                .is_err()
            {
                toast!(obj, gettext("Could not ban user"));
            }

            if response.remove_events {
                self.remove_known_messages_inner(&sender, redactable_events, response.reason)
                    .await;
            }
        }

        /// Unban the room member.
        async fn unban(&self) {
            let Some(sender) = self.sender.obj() else {
                return;
            };
            let obj = self.obj();

            toast!(obj, gettext("Unbanning user…"));

            let room = sender.room();
            let user_id = sender.user_id().clone();
            if room.unban(&[(user_id, None)]).await.is_err() {
                toast!(obj, gettext("Could not unban user"));
            }
        }

        /// Remove the known events of the room member.
        async fn remove_messages(&self) {
            let Some(sender) = self.sender.obj() else {
                return;
            };

            let redactable_events = sender.redactable_events();

            let Some(response) = confirm_room_member_destructive_action_dialog(
                &sender,
                RoomMemberDestructiveAction::RemoveMessages(redactable_events.len()),
                &*self.obj(),
            )
            .await
            else {
                return;
            };

            self.remove_known_messages_inner(&sender, redactable_events, response.reason)
                .await;
        }

        async fn remove_known_messages_inner(
            &self,
            sender: &Member,
            events: Vec<OwnedEventId>,
            reason: Option<String>,
        ) {
            let obj = self.obj();
            let n = u32::try_from(events.len()).unwrap_or(u32::MAX);
            toast!(
                obj,
                ngettext(
                    // Translators: Do NOT translate the content between '{' and '}',
                    // this is a variable name.
                    "Removing 1 message sent by the user…",
                    "Removing {n} messages sent by the user…",
                    n,
                ),
                n,
            );

            let room = sender.room();

            if let Err(failed_events) = room.redact(&events, reason).await {
                let n = u32::try_from(failed_events.len()).unwrap_or(u32::MAX);
                toast!(
                    obj,
                    ngettext(
                        // Translators: Do NOT translate the content between '{' and '}',
                        // this is a variable name.
                        "Could not remove 1 message sent by the user",
                        "Could not remove {n} messages sent by the user",
                        n,
                    ),
                    n,
                );
            }
        }

        /// Toggle whether the user is ignored or not.
        async fn toggle_ignored(&self) {
            let Some(sender) = self.sender.obj().and_upcast::<User>() else {
                return;
            };
            let obj = self.obj();
            let is_ignored = sender.is_ignored();

            let label = if is_ignored {
                gettext("Stop ignoring user…")
            } else {
                gettext("Ignoring user…")
            };
            toast!(obj, label);

            if is_ignored {
                if sender.stop_ignoring().await.is_err() {
                    toast!(obj, gettext("Could not stop ignoring user"));
                }
            } else if sender.ignore().await.is_err() {
                toast!(obj, gettext("Could not ignore user"));
            }
        }
    }
}

glib::wrapper! {
    /// An avatar with a popover menu for room members.
    pub struct SenderAvatar(ObjectSubclass<imp::SenderAvatar>)
        @extends gtk::Widget, @implements gtk::Accessible;
}

impl SenderAvatar {
    pub fn new() -> Self {
        glib::Object::new()
    }
}
