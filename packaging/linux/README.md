# Linux packaging

`script/bundle-linux.sh` builds the tarball, .deb and .rpm from these:

- `com.deptheos.Deptheos.desktop`: the menu entry. The id is the Mac's
  bundle id, and the app gives its windows the same id (`APP_ID` in
  `crates/deptheos_gpui/src/main.rs`) so the desktop matches them to this
  entry and its icon.
- `com.deptheos.Deptheos.metainfo.xml`: AppStream metadata for software
  centers (`appstreamcli validate` checks it).
- `nfpm.yaml`: the .deb and .rpm, with their dependencies.
- `icons/hicolor/<n>x<n>/apps/com.deptheos.Deptheos.png`: the app icon.

## The icons

Rendered from `DeptheosAppIcon.icon` with Icon Composer's `ictool`, which
ships inside Xcode 26 (actool's .icns stops at 256 px), on a Mac from the
repository root:

```sh
ictool="/Applications/Xcode.app/Contents/Applications/Icon Composer.app/Contents/Executables/ictool"
for n in 16 32 48 64 128 256 512; do
  mkdir -p packaging/linux/icons/hicolor/${n}x${n}/apps
  "$ictool" DeptheosAppIcon.icon --export-image \
    --output-file packaging/linux/icons/hicolor/${n}x${n}/apps/com.deptheos.Deptheos.png \
    --platform macOS --rendition Default --width $n --height $n --scale 1
done
```

They are committed because Linux has no tool that reads a .icon. Rerun this
when the icon changes.
