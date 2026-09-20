# The Gazelle manual

Gazelle controls Antelope Audio's Zen Quadro Synergy Core and Zen Studio+ interfaces. This manual covers version 1.2.0 on Windows. Read [Safety](manual/02-safety.md) before anything else.

1. [About Gazelle](manual/01-about.md): what it is, why, and how well it has been tested
2. [Safety](manual/02-safety.md): the risks of software that controls audio hardware, and what to do about them
3. [Getting started](manual/03-getting-started.md): download, stop Antelope's service, first run, the emulator
4. [Concepts](manual/04-concepts.md): devices, inputs, mixes, routing, outputs, effects, surfaces, workspace, snapshots
5. [The app](manual/05-the-app.md): window, tray, header, sidebar, dock, phone use, gestures
6. [The Devices page](manual/06-devices-page.md)
7. [The Inputs page](manual/07-inputs-page.md)
8. [The Outputs page](manual/08-outputs-page.md)
9. [The Mixer page](manual/09-mixer-page.md)
10. [The Routing page](manual/10-routing-page.md)
11. [The Effects page](manual/11-effects-page.md)
12. [The Workspace page](manual/12-workspace-page.md)
13. [Surfaces and digital cables](manual/13-surfaces-and-cables.md)
14. [Snapshots and backup](manual/14-snapshots-and-backup.md)
15. [Install, update and uninstall](manual/15-install-update-uninstall.md)
16. [Troubleshooting](manual/16-troubleshooting.md)
17. [The command line and the API](manual/17-command-line-and-api.md)
18. [Glossary](manual/18-glossary.md)

The [cheat sheet](cheat-sheet.md) is the two-page version to keep beside the desk.

## Building the PDFs

The manual and the cheat sheet print to PDF with the Chromium that Playwright installs for the web app's end-to-end tests. From the repository root:

```
corepack pnpm -C web install --frozen-lockfile
corepack pnpm -C web docs:pdf
```

This writes `docs/dist/gazelle-manual.pdf` (title page, contents with page numbers, numbered figures, a PDF outline) and `docs/dist/gazelle-cheat-sheet.pdf` (at most two pages). `docs/dist` is not committed. If Playwright's Chromium is missing, `corepack pnpm -C web exec playwright install chromium` fetches it.

Before printing, the build checks, and stops on any failure:

- no em dash or en dash anywhere in `README.md` or `docs/` (the project's house style: use commas, colons, semicolons, brackets or "to");
- every relative link, image and `#anchor` resolves;
- `book.json` lists every page under `manual/` exactly once, and every image in `images/` is used;
- when a server binary has been built, the options table in [the command-line chapter](manual/17-command-line-and-api.md) matches its `--help`, flag for flag and default for default.

`corepack pnpm -C web docs:check` runs the checks alone.

## Screenshots

The pictures in `images/` are taken from the running app against the built-in emulator, never real hardware:

```
corepack pnpm -C web docs:screenshots
```

It builds the web app and the server, starts the server with `--backend loopback` (and `GAZELLE_NO_HARDWARE=1`) on a free port, fills in a sample workspace, and photographs each page. The emulator's status reports are a moving test pattern, so for the pictures the clock, preset, power and mute readings shown in the browser are held at plausible values; meters keep moving. Rerun it when a page changes, and commit the images.

## Adding a chapter

Write it under `manual/`, add it to `book.json` in reading order, and link it from the list above. A level-one heading is the chapter title; level-two headings are numbered sections and appear in the contents. An image alone in its paragraph becomes a numbered figure, with its alt text as the caption. A quote starting with **Warning.** or **Note.** becomes a callout.
