## Common Problems

If the tests aren't working in the command line: `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`. Need to use the full XCode instead of the command line tools.

## Links

* https://forums.swift.org/t/is-concurrent-now-the-standard-tool-for-shifting-expensive-synchronous-work-off-the-main-actor/82976
* https://developer.apple.com/documentation/security/customizing-the-notarization-workflow

## Commands

* `xcrun notarytool store-credentials "notarytool-password" --apple-id <> --team-id <> --password <>`
