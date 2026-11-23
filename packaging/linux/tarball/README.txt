Deptheos for Linux
==================

Deptheos is a free, open-source focus stacker for macro photography that
runs on the GPU. https://deptheos.com

  bin/deptheos-gui   the desktop app
  bin/deptheos       the command line (deptheos --help)
  share/             the menu entry, icons, AppStream metadata, licenses
                     and the sample stacks

Install
-------

Run the app from this folder (bin/deptheos-gui), or install it:

  ./install.sh                         into ~/.local, for you
  sudo PREFIX=/usr/local ./install.sh  into /usr/local, for every user
  ./install.sh --uninstall             removes it again (same PREFIX)

The .deb and .rpm on the releases page install the same files under /usr
and pull in the libraries below themselves.

Libraries it needs
------------------

Built on Ubuntu 22.04, so it runs on glibc 2.35 or newer: Ubuntu 22.04+,
Debian 12+, Fedora 36+ and their relatives.

Ubuntu and Debian:

  sudo apt install libgomp1 libfontconfig1 libfreetype6 libxkbcommon0 \
    libxkbcommon-x11-0 libxcb1 libwayland-client0 libvulkan1 \
    mesa-vulkan-drivers xdg-desktop-portal

Fedora:

  sudo dnf install libgomp fontconfig freetype libxkbcommon libxkbcommon-x11 \
    libxcb libwayland-client vulkan-loader mesa-vulkan-drivers \
    xdg-desktop-portal

A desktop install has most of these already. The file dialogs go through
the desktop portal (xdg-desktop-portal), or zenity if there is none.

The GPU
-------

Deptheos stacks and draws with Vulkan, so it needs a Vulkan driver:
mesa-vulkan-drivers covers AMD, Intel and, on ARM boards, the Raspberry
Pi 5 (v3dv); NVIDIA's own driver brings its own. vulkaninfo --summary
(vulkan-tools) lists what it finds.

In a virtual machine, turn on the virtual GPU's acceleration (in UTM,
virtio-gpu with GPU acceleration on). Without it, Vulkan falls back to
Mesa's CPU renderer (lavapipe): Deptheos runs, but stacking is slow.

Licenses
--------

Deptheos is free software under the GNU GPL version 3, with two additional
permissions, for the Adobe DNG SDK and XMP Toolkit and for app stores:
share/doc/deptheos/LICENSE.md. The name and logo are not under the GPL:
TRADEMARKS.md. The libraries it includes
and their licenses: THIRD_PARTY_NOTICES.txt.
