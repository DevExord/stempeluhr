#!/bin/sh
# Builds the Flatpak locally and installs it for the current user.
# Needs flatpak-builder (here: the copy inside the GNOME Builder Flatpak),
# org.gnome.Sdk//51 and org.freedesktop.Sdk.Extension.rust-stable//26.08.
#
# The build happens into ./_repo; the bundle is installed with the host's
# flatpak (flatpak-builder inside the Builder sandbox would otherwise install
# into *its own* user installation).
set -e
cd "$(dirname "$0")/.."
if command -v flatpak-builder >/dev/null 2>&1; then
  FB="flatpak-builder"
else
  FB="flatpak run --filesystem=home --talk-name=org.freedesktop.Flatpak --command=flatpak-builder org.gnome.Builder"
fi
$FB --force-clean --disable-rofiles-fuse --state-dir=.flatpak-builder \
    --repo=_repo _flatpak build-aux/digital.fischers.stempeluhr.local.yaml
flatpak build-bundle _repo stempeluhr.flatpak digital.fischers.stempeluhr
flatpak install --user --reinstall -y --noninteractive stempeluhr.flatpak
echo "Installed. Run: flatpak run digital.fischers.stempeluhr"
