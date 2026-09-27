# Gazelle Remote (Android)

A small Android app that is a remote for the Gazelle running on your computer: Gazelle's own web
page, full screen, plus pairing by scanning the QR code Gazelle shows. It carries no copy of the web
app; it shows the one your computer serves, so it is always the same version as your Gazelle.

It is a native shell rather than an installable web app (a PWA) because Chrome installs web apps
only from HTTPS, and a computer on your own network speaks plain HTTP.

The user's side of it (getting it, installing it, pairing) is in the manual's
[Remote page chapter](../docs/manual/14-remote-page.md#the-android-app).

## What it does

- **First run, or "Pair with another computer":** a native screen with two ways in. **Scan the code
  on your computer** uses Google's code scanner (Play services), which shows its own camera screen
  and hands back only the text, so the app needs no camera permission and has no camera code. A
  phone without Play services is told so, and types instead: the computer's address (`IP:port`, the
  port 8420 when left out) and the code, read as Gazelle reads it (case, spaces and dashes do not
  matter).
- **A scanned code is accepted only as Gazelle writes it:** `http://`, an IPv4 address, an IPv6
  address in brackets or a plain host name, an explicit port, the path `/pair`, and a fragment of
  `code=` and a code shaped `XXXX-XXXX` from Gazelle's alphabet. Anything else is refused with a
  plain message.
- **Pairing is the web page's.** The app opens `http://host:port/pair#code=...` in the WebView;
  Gazelle's pair page asks for a name for the phone, pairs, and lands on `/#/remote`. The server's
  answer sets the phone's key as an HttpOnly cookie, which the WebView's cookie jar keeps (flushed
  to disk when pairing completes and whenever the app leaves the screen). When the WebView reaches
  `/#/remote` on that origin, the app remembers `http://host:port`, and opens its Remote page from
  then on.
- **Full screen, edge to edge,** with the system bars, cutouts and keyboard as padding, and the bars
  dark or light with the phone's theme. The screen stays on while the app is in front. Back goes
  back through the page's history, then leaves.
- **When the page does not load** (the computer is off, another network, phones not allowed), a
  native screen says "Gazelle is not answering at http://host:port", with **Retry** and **Pair with
  another computer**.
- **When the phone is no longer paired**, Gazelle's page links to `/pair`. The app opens its own
  pairing screen for that instead.
- **The one menu** is a long press on the app's icon: **Pair with another computer**. The pairing
  screen then shows the address it is paired with now, and **Back to Gazelle**.
- No notifications, no background service, no other permission than the network.

## Security model

- **Plain HTTP on your network.** Gazelle has no certificate a phone could check for a private
  address, so the app allows cleartext traffic (`res/xml/network_security_config.xml`, in the base
  config, since a domain config can only list host names and cannot say "any private address").
  Anyone who can watch your network's traffic can see what the phone and Gazelle send, the key
  included, as the manual says for phones in a browser.
- **Confined to one origin.** The WebView loads pages only from the paired origin, scheme, host and
  port all equal (`Links.sameOrigin`, unit tested). A link anywhere else opens in the system browser
  (web and mail links only, and only when tapped), and any request the page makes to another origin
  is refused with a 403. The app has no other network code.
- **The WebView is locked down:** JavaScript and DOM storage on (the page is a JavaScript app); file
  and content access off; mixed content never; no JavaScript interface; no pop-up windows;
  geolocation off; Safe Browsing left at its default.
- **The key lives in the WebView's cookie jar,** HttpOnly, so not even the page's own script can
  read it, and the app never reads it either. Backup is off (`allowBackup="false"`). "Pair with
  another computer" clears every cookie, the page's storage and the saved address. Revoking the
  phone on the computer ends it whatever the phone holds.

## Building

You need JDK 17 (the Android Gradle Plugin's requirement), the Android SDK (API 36, found through
`ANDROID_HOME` or a `local.properties` with `sdk.dir=...`), and Gradle 9.6.0 or later. From this
directory:

```
gradle :app:assembleDebug
gradle :app:testDebugUnitTest
gradle :app:lintDebug
```

The debug APK is `app/build/outputs/apk/debug/app-debug.apk`. There is no Gradle wrapper in the
tree, since one cannot be generated without Gradle; `gradle wrapper --gradle-version 9.6.0` adds one
whenever that is wanted.

The versions are pinned in `gradle/libs.versions.toml`: the Android Gradle Plugin 9.4.1, which
compiles Kotlin itself (its "built-in Kotlin", at the Kotlin 2.2.10 it depends on), compile and
target SDK 36, minimum SDK 26 (Android 8.0).

A release passes the server's version: `-PgazelleVersionName=1.5.0 -PgazelleVersionCode=10500`,
the code being `major * 10000 + minor * 100 + patch` (`xtask check-version` prints it as
`android_version_code`).

## How CI builds it

- **Every change** (`.github/workflows/ci.yml`, job `android`, on Ubuntu): Temurin 17, Gradle
  9.6.0 through `gradle/actions/setup-gradle`, then
  `gradle :app:lintDebug :app:testDebugUnitTest :app:assembleDebug`, and the debug APK kept as the
  run's `gazelle-remote-debug` artifact.
- **A release** (`.github/workflows/release.yml`): the `android` job builds the release APK
  unsigned, with the server's version, beside the Windows build and holding no secret. The `sign`
  job, in the protected `release` environment, signs it with the app's key (`xtask sign-apk`, the
  SDK's `apksigner` on the Windows runner) as `Gazelle-Remote.apk` in the release directory, before
  `SHA256SUMS` is written and signed, so the release's signature covers it too. The dry run signs
  it with a throwaway key made on the spot. `docs/releasing.md` has the whole procedure and the
  one-time key setup.

## Layout

| Path | What it is |
|---|---|
| `app/src/main/kotlin/.../MainActivity.kt` | The activity: the WebView, the pairing screen, the trouble screen |
| `app/src/main/kotlin/.../Links.kt` | Plain Kotlin: parsing scanned and typed addresses, and where the WebView may go |
| `app/src/test/kotlin/.../LinksTest.kt` | JUnit 4 tests of `Links`, run by `testDebugUnitTest` |
| `app/src/main/res/xml/network_security_config.xml` | Cleartext allowed, and why |
| `app/src/main/res/xml/shortcuts.xml` | The icon's long press menu |
| `app/src/main/res/drawable/ic_launcher_foreground.xml` | The desktop icon's "G", as a vector |
