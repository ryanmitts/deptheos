# Contributing

Bug reports and pull requests are welcome at
https://github.com/ryanmitts/deptheos.

## The license your contribution is under

Deptheos is GPL-3.0 with three additional permissions, all at the top of
[LICENSE.md](LICENSE.md): one to combine it with Adobe's DNG SDK and XMP
Toolkit, one to combine it with LibRaw under its CDDL, and one to
distribute it through app stores. By opening a pull
request you license your contribution under those same terms, permissions
included, and you say that you have the right to do so (it is your own work,
or its license allows it). You keep the copyright in what you wrote.

The app-store permission is why this is written down. An app store's terms
restrict what people may do with an app they download, which the GPL alone
does not allow, so the iOS app can be in the App Store only if every
contributor's code carries that permission.

Don't add code copied from a GPL project that lacks these permissions, or a
dependency whose license `licenses/about.toml` does not accept
(`script/notices.sh --check` fails on one).

## Before you open a pull request

`CLAUDE.md` has the project's conventions and the quick checks for each part
of the code. `docs/decisions/` says why things are built as they are; read
the record for an area before changing its approach.
