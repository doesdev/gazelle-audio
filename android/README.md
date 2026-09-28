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
- **Connecting, and giving up quickly.** Before the WebView gets an address, the app asks
  `http://host:port/` itself (`Reach.kt`, unit tested against sockets of its own): a plain GET off
  the main thread, 4 seconds to connect and 4 to answer. Meanwhile a native screen says
  "Connecting to http://host:port..." with a spinner and **Cancel** (back to the pairing screen
  when pairing, else to the trouble screen). Any HTTP answer, even an error, means Gazelle is
  there, and the page loads unseen beneath that screen (so no white page), showing once it has
  committed. A 403 `remote_off` means phones are not allowed, which the app says. A firewall that
  drops connections (a network Windows counts as Public, until the firewall's question is answered
  for public networks) would otherwise leave the WebView waiting a minute or more; now it gives up
  after 4 seconds. If the page still has not shown 15 seconds after the computer answered, the app
  gives up on it too.
- **When the page does not load**, a native screen says "Gazelle is not answering at
  http://host:port", why in a line (no answer in time, nothing listening on that port, no route,
  an unknown name), what to check, and **Retry** and **Pair with another computer**.
- **When the phone is no longer paired**, Gazelle's page links to `/pair`. The app opens its own
  pairing screen for that instead.
- **The one menu** is a long press on the app's icon: **Pair with another computer**. The pairing
  screen then shows the address it is paired with now, and **Back to Gazelle**.
- No notifications, no background service, no other permission than the network.
- **It looks like Gazelle.** The app's own screens (pairing, connecting, trouble) are always
  Gazelle Dark, the web app's default theme, whatever the phone's day or night setting: the
  colours in `res/values/colors.xml` are the tokens of `web/apps/web/themes/gazelle-dark.json`
  under the same names, and the buttons, fields, header and section bars follow
  `web/apps/web/src/elements/styles.ts`, `header.ts` and `section.ts`. Change the web theme and
  those files together. The fonts are the web app's, Inter and Josefin Sans (SIL Open Font
  Licence): the web app bundles them from Fontsource at build time and the repository holds no
  font file, so the app asks Google Fonts for them through Play services' font provider
  (`res/font`, downloadable fonts) and carries none; without Play services the phone's own sans
  serif stands in.

## Security model

- **Plain HTTP on your network.** The network security config covers the app's own reachability
  request as well as the WebView: it applies to the platform's whole HTTP stack. Gazelle has no certificate a phone could check for a private
  address, so the app allows cleartext traffic (`res/xml/network_security_config.xml`, in the base
  config, since a domain config can only list host names and cannot say "any private address").
  Anyone who can watch your network's traffic can see what the phone and Gazelle send, the key
  included, as the manual says for phones in a browser.
- **Confined to one origin.** The WebView loads pages only from the paired origin, scheme, host and
  port all equal (`Links.sameOrigin`, unit tested). A link anywhere else opens in the system browser
  (web and mail links only, and only when tapped), and any request the page makes to another origin
  is refused with a 403. The app's only other request is its own reachability check, a GET of
  the same origin's `/`.
- **The WebView is locked down:** JavaScript and DOM storage on (the page is a JavaScript app); file
  and content access off; mixed content never; no JavaScript interface; no pop-up windows;
  geolocation off; Safe Browsing left at its default.
- **The key lives in the WebView's cookie jar,** HttpOnly, so not even the page's own script can
  read it, and the app never reads it either. Backup is off (`allowBackup="false"`). "Pair with
  another computer" clears every cookie, the page's storage and the saved address. Revoking the
  phone on the computer ends it whatever the phone holds.

## Building

You need JDK 17 (the Android Gradle Plugin's requirement) and the Android SDK with API 36 and
build-tools 36.0.0, found through `ANDROID_HOME` or a `local.properties` with `sdk.dir=...`
(ignored by git). Gradle itself comes from the wrapper in this directory (`gradlew`, Gradle 9.6.0,
its download checked against the SHA-256 in `gradle/wrapper/gradle-wrapper.properties`). From this
directory:

```
./gradlew --no-daemon :app:lintDebug :app:verifyRoborazziDebug :app:assembleDebug
```

That is what CI runs: lint, every unit test with the screenshots checked, and the debug APK,
`app/build/outputs/apk/debug/app-debug.apk`. On Windows the wrapper is `gradlew.bat`, and each
path can be given for the one command, leaving the machine's own settings alone. For example,
with the JDK and SDK where one Windows PC keeps them, in the user's own folder:

```
JAVA_HOME="$USERPROFILE/Android/jdk-17" ANDROID_HOME="$USERPROFILE/Android/Sdk" ./gradlew.bat --no-daemon :app:assembleDebug
```

Keep them out of `%LOCALAPPDATA%` when they are installed from inside a packaged (MSIX) Windows
app, such as a terminal or an assistant running in one: Windows quietly redirects a new folder that
app creates there into the app's own private storage, so the tools work from that app and are
missing everywhere else.

### Screenshot tests

`ScreensTest` renders every screen of the app's own on the JVM (Robolectric, in its native
graphics mode, with Roborazzi) and compares each with the image committed in
`app/src/test/screenshots`: pairing (empty, typed in, a refused code, while paired, without Play
services, on a 360dp phone, at font scale 1.3), connecting, and the trouble screen (no answer in
time, refused, phones not allowed, the page too slow, cancelled, and no answer at font scale 1.3).
They are 411 by 891dp phones at mdpi, so one pixel is one dp. The network and Play services are
stood in (the activity's `probe` and `scannerAvailable`); everything else is the app's own code.
Downloadable fonts do not resolve there, so the images show the fallback sans serif. The tests
also check that every button and field is at least 48dp tall.

`verifyRoborazziDebug` fails on a difference and writes the comparison images under
`app/build/outputs/roborazzi`, which CI keeps when it fails. After changing a screen on purpose,
record the images again, look at them, and commit them with the change:

```
./gradlew --no-daemon :app:recordRoborazziDebug
```

Robolectric renders API 35 here: API 36 needs Java 21 under Robolectric, and the build's JDK is 17.

The versions are pinned in `gradle/libs.versions.toml`: the Android Gradle Plugin 9.4.1, which
compiles Kotlin itself (its "built-in Kotlin", at the Kotlin 2.2.10 it depends on), compile and
target SDK 36, minimum SDK 26 (Android 8.0). The screenshot tests use Robolectric 4.17 and
Roborazzi 1.75.0.

A release passes the server's version: `-PgazelleVersionName=1.5.0 -PgazelleVersionCode=10500`,
the code being `major * 10000 + minor * 100 + patch` (`xtask check-version` prints it as
`android_version_code`).

## How CI builds it

- **Every change** (`.github/workflows/ci.yml`, job `android`, on Ubuntu): Temurin 17,
  `gradle/actions/setup-gradle` to validate the wrapper jar against Gradle's published checksums
  and to cache (its open source "basic" cache), then
  `./gradlew :app:lintDebug :app:verifyRoborazziDebug :app:assembleDebug`, the debug APK kept as
  the run's `gazelle-remote-debug` artifact, and on a failure the reports and screenshot
  comparisons kept as `android-reports`.
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
| `app/src/main/kotlin/.../MainActivity.kt` | The activity: the WebView, the connecting, pairing and trouble screens |
| `app/src/main/kotlin/.../Links.kt` | Plain Kotlin: parsing scanned and typed addresses, and where the WebView may go |
| `app/src/main/kotlin/.../Reach.kt` | Plain Kotlin: asking the computer whether it answers, and what the app shows next |
| `app/src/test/kotlin/.../LinksTest.kt`, `ReachTest.kt` | JUnit 4 tests of both, run by `testDebugUnitTest`; `ReachTest` probes sockets of its own on 127.0.0.1 |
| `app/src/main/res/xml/network_security_config.xml` | Cleartext allowed, and why |
| `app/src/main/res/xml/shortcuts.xml` | The icon's long press menu |
| `app/src/main/res/drawable/ic_launcher_foreground.xml` | The desktop icon's "G", as a vector |
