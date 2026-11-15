#!/bin/sh
# Runs a command inside the GNOME SDK (blueprint-compiler, GTK headers, …)
# with the host's Cargo. Development convenience only.
exec flatpak run --filesystem=home --share=network --env=PATH=/usr/bin:$HOME/.cargo/bin --command=sh org.gnome.Sdk//${GNOME_SDK_BRANCH:-51} -c "$*"
