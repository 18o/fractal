[![Our chat room](https://img.shields.io/matrix/fractal-gtk:matrix.org?color=blue&label=%23fractal%3Agnome.org&logo=matrix)](https://matrix.to/#/#fractal:gnome.org)
[![Our Gitlab project](https://img.shields.io/badge/gitlab.gnome.org%2F-World%2FFractal-green?logo=gitlab)](https://gitlab.gnome.org/World/fractal/)
[![Our documentation](https://img.shields.io/badge/%F0%9F%95%AE-Docs-B7410E?logo=rust)](https://world.pages.gitlab.gnome.org/fractal/)
[![Official package](https://img.shields.io/flathub/downloads/org.gnome.Fractal?logo=flathub)](https://flathub.org/apps/org.gnome.Fractal)

# Fractal

Fractal is a Matrix messaging app for GNOME written in Rust. Its interface is optimized for
collaboration in large groups, such as free software projects, and will fit all screens, big or small.

<img
    src="https://gitlab.gnome.org/World/fractal/raw/main/screenshots/main.png"
    alt="Fractal’s main window"
    width="882"
    height="672"
/>

Highlights:

* Find rooms to discuss your favorite topics, or talk privately to people, securely thanks to end-to-end encryption
* Send rich formatted messages, files, or your current location
* Reply to specific messages, react with emoji, edit or remove messages
* View images, and play audio and video directly in the conversation
* See who has read messages, and who is typing
* Log into multiple accounts at once (with Single-Sign On support)

## Contents

[TOC]

## Installation instructions

Flatpak is the recommended installation method. For installing any of our Flatpaks, you need to
make sure your system is [set up with the Flathub remote](https://flathub.org/setup).

All of our Flatpaks can be installed in parallel, offering you the opportunity to try out the
development version while keeping the stable release around for daily use.

### Stable version

The current stable version is 11.2 (released June 10th 2025).

You can get the official Fractal Flatpak from Flathub.

<a href="https://flathub.org/apps/org.gnome.Fractal">
<img
    src="https://flathub.org/api/badge?svg&locale=en&light"
    alt="Get Fractal on Flathub"
    width="240"
/>
</a>

### Beta version

The current beta version is 12.beta (released June 26th 2025).

It is available as a Flatpak on Flathub Beta.

To get it, first set up the Flathub Beta remote:

<a href="https://flathub.org/beta-repo/flathub-beta.flatpakrepo">
<img
    src="https://gitlab.gnome.org/World/fractal/uploads/81944cf92504343a03121a58722345a2/flathub-beta-badge.svg"
    alt="Add Flathub Beta repository"
    width="240px"
    height="80px"
/>
</a>

Then install the application.

<a href="https://flathub.org/beta-repo/appstream/org.gnome.Fractal.flatpakref">
<img
    src="https://gitlab.gnome.org/World/fractal/uploads/31a40da5d71a30c47f135e78ffef3df5/fractal-beta-badge.svg"
    alt="Download Fractal Beta"
    width="240px"
    height="80px"
/>
</a>

Or from the command line:

```sh
# Add the Flathub Beta repo
flatpak remote-add --user --if-not-exists flathub-beta https://flathub.org/beta-repo/flathub-beta.flatpakrepo

# Install Fractal Beta
flatpak install --user flathub-beta org.gnome.Fractal
```

Finally, run the application:

```sh
flatpak run org.gnome.Fractal//beta
```

If you have both the stable and beta versions installed, your system will only show one icon in the
apps list and launch the stable version by default. If you want to run the beta version by default,
use this command:

```sh
flatpak make-current org.gnome.Fractal beta
```

_Note that you can go back to using the stable version by default by using the same command and
replacing `beta` with `stable`._

### Development version

If you want to try the upcoming version of Fractal without building it yourself, it is available as a nightly Flatpak
in [the gnome-nightly repo](https://nightly.gnome.org/).

First, set up the GNOME nightlies.

<a href="https://nightly.gnome.org/gnome-nightly.flatpakrepo">
<img
    src="https://gitlab.gnome.org/World/fractal/uploads/c276f92660dcf50067714ac08e193fea/gnome-nightly-badge.svg"
    alt="Add gnome-nightly repository"
    width="240px"
    height="80px"
/>
</a>

Then install the application.

<a href="https://nightly.gnome.org/repo/appstream/org.gnome.Fractal.Devel.flatpakref">
<img
    src="https://gitlab.gnome.org/World/fractal/uploads/5e42d322eaacc7da2a52bfda9f7a4e53/fractal-nightly-badge.svg"
    alt="Download Fractal Nightly"
    width="240px"
    height="80px"
/>
</a>

Or from the command line:

```sh
# Add the gnome-nightly repo
flatpak remote-add --user --if-not-exists gnome-nightly https://nightly.gnome.org/gnome-nightly.flatpakrepo

# Install the nightly build
flatpak install --user gnome-nightly org.gnome.Fractal.Devel
```

### Runtime Dependencies

On top of the dependencies required at build time and checked by Meson, Fractal depends on the
following dependencies at runtime:

* xdg-desktop-portal and its backends: some functionalities are dependant on the following portals,
  and a permission will be asked when necessary, but Fractal should work without them:
  * Secret: this portal or a Secret Service is required, see [storing secrets](#storing-secrets).
  * Camera: scan QR codes during verification.
  * Location: send the user’s location in a conversation.
  * Settings: get the 12h/24h time format system preference.
* GStreamer plugins:
  * gst-plugin-gtk4 (gstgtk4): required to preview videos in the timeline and to present the output
    of the camera.
  * libgstpipewire with the `pipewiredeviceprovider`: used to list and access the cameras.
* glycin: all images are loaded with this library so loaders for the different image formats need to
  be installed.

#### Storing secrets

Fractal doesn’t store your **password**, but it stores your **access token** and the **passphrase**
used to encrypt the database and the local cache.

The Fractal Flatpaks use the [Secret **Portal**](https://docs.flatpak.org/en/latest/portal-api-reference.html#gdbus-org.freedesktop.portal.Secret)
to store those secrets. If you are using GNOME this should just work. If you are using a different
desktop environment or are facing issues, make sure `xdg-desktop-portal` is installed along with a
service that provides the [Secret portal backend interface](https://docs.flatpak.org/en/latest/portal-api-reference.html#gdbus-org.freedesktop.impl.portal.Secret),
like gnome-keyring or KWallet (since version 6.2).

Any version that is not sandboxed relies on software that implements the [Secret **Service** API](https://www.freedesktop.org/wiki/Specifications/secret-storage-spec/)
to store those secrets. Therefore, you need to have software providing that service on your system,
like gnome-keyring, KeepassXC ([setup guide](https://avaldes.co/2020/01/28/secret-service-keepassxc.html)),
or KWallet. Once again, if you are using GNOME this should just work.

If you prefer to use software that only implements the Secret Service API while using the Flatpaks,
you need to make sure that no service implementing the Secret portal backend interface is running,
and you need to allow Fractal to access the D-Bus service with this command:

```sh
flatpak override --user --talk-name=org.freedesktop.secrets org.gnome.Fractal
```

_For the nightly version, change the application name to `org.gnome.Fractal.Devel`._

Or with [Flatseal](https://flathub.org/apps/details/com.github.tchx84.Flatseal), by adding
`org.freedesktop.secrets` in the **Session Bus** > **Talk** list of Fractal.

## Security Best Practices

You should use a strong **password** that is hard to guess to protect the secrets stored on your
device, whether the password is used directly to unlock your secrets (with a password manager for
example) or if it is used to open your user session and your secrets are unlocked automatically
(which is normally the case with a GNOME session).

Furthermore, make sure to lock your system when stepping away from the computer since an unlocked
computer can allow other people to access your private communications and your secrets.

## Contributing

### Code

Please follow our [contributing guidelines](CONTRIBUTING.md).

### Translations

Fractal is translated by the GNOME translation team on [Damned lies](https://l10n.gnome.org/).

Find your language in the list on [the Fractal module page on Damned lies](https://l10n.gnome.org/module/fractal/).

The names of the emoji displayed during verification come from [the Matrix specification repository](https://github.com/matrix-org/matrix-spec/tree/main/data-definitions).
They are translated on [Element’s translation platform](https://translate.element.io/projects/matrix-doc/sas-emoji-v1).

## Frequently Asked Questions

* Does Fractal have encryption support?

**Yes**, since Fractal 5, encryption is supported using Cross-Signing. See
<https://gitlab.gnome.org/World/fractal/-/issues/717> for more info on the state of encryption.

* Can I run Fractal with the window closed?

Currently Fractal does not support this. Fractal is a GNOME application, and accordingly adheres to
the GNOME guidelines and paradigms. This will be revisited if or when GNOME gets a proper paradigm
to interact with apps running in the background.

## The origin of Fractal

The current version is a complete rewrite of Fractal built on top of the
[matrix-rust-sdk](https://github.com/matrix-org/matrix-rust-sdk) using [GTK4](https://gtk.org/).

The previous version of Fractal was using GTK3 and its own backend to talk to a matrix homeserver,
the code can be found in the [`legacy` branch](https://gitlab.gnome.org/World/fractal/-/tree/legacy).

Initial versions were based on Fest <https://github.com/fest-im/fest>, formerly called ruma-gtk.
In the origins of the project it was called guillotine, based on French revolution, in relation with
the Riot client name, but it's a negative name so we decide to change for a math one.

The name Fractal was proposed by Regina Bíró.

## Code of Conduct

Fractal follows the official [GNOME Code of Conduct](https://conduct.gnome.org/).
