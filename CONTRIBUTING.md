# Contributing

On this page you can learn how to contribute to [Fractal](https://gitlab.gnome.org/World/fractal/)
by working on the code.

## Getting Started

Here are a few links to help you get started with Rust and the GTK Rust bindings:

- [Learn Rust](https://www.rust-lang.org/learn)
- [GUI development with Rust and GTK 4](https://gtk-rs.org/gtk4-rs/stable/latest/book)
- [gtk-rs website](https://gtk-rs.org/)

[The Rust docs of our application](https://gnome.pages.gitlab.gnome.org/fractal/) and the [GNOME Development Center](https://developer.gnome.org/)
might also be useful.

Don't hesitate to join [our Matrix room](https://matrix.to/#/#fractal:gnome.org) to come talk to us
and ask us any questions you might have. The [“Rust ❤️ GNOME” room](https://matrix.to/#/#rust:gnome.org)
can also provide general help about using Rust in GNOME.

## Build Instructions

### Prerequisites

Fractal is written in Rust, so you will need to have at least Rust 1.80 and Cargo available on your
system. You will also need to install the Rust nightly toolchain to be able to run our
[pre-commit hook](#pre-commit), which can be done with:

```sh
rustup toolchain install nightly
```

If you're building Fractal with Flatpak (via GNOME Builder or the command line), you will need to
manually add the necessary remotes and install the required freedesktop.org extensions:

```sh
# Add Flathub and the gnome-nightly repo
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak remote-add --user --if-not-exists gnome-nightly https://nightly.gnome.org/gnome-nightly.flatpakrepo

# Install the gnome-nightly Sdk and Platform runtime
flatpak install --user gnome-nightly org.gnome.Sdk//master org.gnome.Platform//master

# Install the required rust-stable extension from Flathub
flatpak install --user flathub org.freedesktop.Sdk.Extension.rust-stable//25.08beta

# Install the required llvm extension from Flathub
flatpak install --user flathub org.freedesktop.Sdk.Extension.llvm20//25.08beta
```

If you are building the flatpak manually you will also need flatpak-builder on your system, or the
`org.flatpak.Builder` flatpak from Flathub.

### GNOME Builder

Using [GNOME Builder](https://apps.gnome.org/Builder/) with [flatpak](https://flatpak.org/) is
the recommended way of building and installing Fractal.

You can find help on cloning and building a project in the [docs of Builder](https://builder.readthedocs.io/).

### Flatpak via fenv

As an alternative, [fenv](https://gitlab.gnome.org/ZanderBrown/fenv) allows to setup a flatpak
environment from the command line and execute commands in that environment.

First, install fenv:

```sh
cargo install --git https://gitlab.gnome.org/ZanderBrown/fenv fenv
```

After that, setup the project:

```sh
# Set up the flatpak environment
fenv gen build-aux/org.gnome.Fractal.Devel.json
```

Finally, build and run the application:

```sh
# Build the project
fenv build

# Launch Fractal
fenv run
```

_Note that fenv will use `_build` as build directory._

To test changes you make to the code, re-run these two last commands.

### Install the flatpak

Some features that interact with the system require the app to be installed to test them (i.e.
notifications, command line arguments, etc.).

GNOME Builder can export a flatpak of the app after it has been successfully built.

Fractal can then be installed with:

```sh
flatpak install --user --bundle path/to/org.gnome.Fractal.Devel.flatpak
```

Alternatively, it can be built and installed with flatpak-builder:

```sh
flatpak-builder --user --install app build-aux/org.gnome.Fractal.Devel.json
```

_Note that the `flatpak-builder` command can be replaced with `flatpak run org.flatpak.Builder`._

It can then be entirely removed from your system with:

```sh
flatpak remove --delete-data org.gnome.Fractal.Devel
```

### GNU/Linux

If you decide to ignore our recommendation and build on your host system, outside of Flatpak, you
will need Meson and Ninja.

```sh
meson setup --prefix=/usr/local _build
ninja -C _build
sudo ninja -C _build install
```

## Pre-commit

We expect all code contributions to be correctly formatted. To help with that, a pre-commit hook
should get installed as part of the building process. It runs the `hooks/checks.sh` script. It's a
quick script that makes sure that the code is correctly formatted with `rustfmt`, among other
things. Make sure that this script is effectively run before submitting your merge request,
otherwise CI will probably fail right away.

You should also run `cargo clippy` as that will catch common errors and improve the quality of your
submissions and is once again checked by our CI.

## Commit

Please follow the [GNOME commit message guidelines](https://handbook.gnome.org/development/commit-messages.html).
We enforce the use of a tag as a prefix for the summary line. It should be the area of the app that
is changed.

## Merge Request

You must pass all the prerequisites of the [Change Submission Guide](https://handbook.gnome.org/development/change-submission.html).

Before submitting a merge request, make sure that [your fork is available publicly](https://gitlab.gnome.org/help/user/public_access.md),
otherwise CI won't be able to run.

Use the title of your commit as the title of your MR if there's only one. Otherwise it should
summarize all your commits. If your commits do several tasks that can be separated, open several
merge requests.

In the details, write a more detailed description of what it does. If your changes include a change
in the UI or the UX, provide screenshots in both light and dark mode, and/or a screencast of the
new behavior.

Don't forget to mention the issue that this merge request solves or is related to, if applicable.
GitLab recognizes the syntax `Closes #XXXX` or `Fixes #XXXX` that will close the corresponding
issue accordingly when your change is merged.

We expect to always work with a clean commit history. When you apply fixes or suggestions,
[amend](https://git-scm.com/docs/git-commit#Documentation/git-commit.txt---amend) or
[fixup](https://git-scm.com/docs/git-commit#Documentation/git-commit.txt---fixupamendrewordltcommitgt)
and [squash](https://git-scm.com/docs/git-rebase#Documentation/git-rebase.txt---autosquash) your
previous commits that you can then [force push](https://git-scm.com/docs/git-push#Documentation/git-push.txt--f).
