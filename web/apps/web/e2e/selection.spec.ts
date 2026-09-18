// Dragging across the UI must not light up labels, readouts and headings, which looks broken.
// Selection is not switched off (user-select stays as it was, so text can still be selected on
// purpose, e.g. while testing); it is drawn transparent. A shadow root cannot be relied on to take
// the document's ::selection rule (Chromium passes it down by highlight inheritance, which is newer
// than shadow DOM), so the rule sits in the stylesheet every element shares as well as in the page.
// Fields a person types in keep a visible selection in the theme's accent.
//
// Chrome reports a transparent ::selection background whether or not a page styles it (its own
// highlight is not a computed value), so "transparent" alone would pass with no rule at all. What
// is checked is what is drawn: an element looks the same selected as not, while the selection is
// really there.

import { expect, test, type Locator, type Page } from "@playwright/test";

import { startServer, type RunningServer } from "../../../packages/client/test/integration/server.ts";
import { resetWorkspace } from "./workspace.ts";

let server: RunningServer;

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--dry-run"], { webUi: true });
});

test.beforeEach(() => resetWorkspace(server));

test.afterAll(async () => {
  await server?.stop();
});

const TRANSPARENT = "rgba(0, 0, 0, 0)";

/** Selects all of `target`'s text, as a drag across it would, and returns what was selected. */
function selectText(target: Locator): Promise<string> {
  return target.evaluate((el) => {
    const range = document.createRange();
    range.selectNodeContents(el);
    const selection = getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
    return selection.toString();
  });
}

/** Whether selecting `target`'s text changes how it looks; the text must really be selected. */
async function selectionShows(page: Page, target: Locator): Promise<boolean> {
  await page.evaluate(() => getSelection()?.removeAllRanges());
  const before = await target.screenshot({ animations: "disabled" });
  expect((await selectText(target)).trim()).not.toBe("");
  const after = await target.screenshot({ animations: "disabled" });
  return !before.equals(after);
}

/** A theme variable resolved to a colour, as computed styles report colours. */
function themeColour(page: Page, variable: string): Promise<string> {
  return page.evaluate((name) => {
    const probe = document.createElement("span");
    probe.style.color = `var(${name})`;
    document.body.append(probe);
    const colour = getComputedStyle(probe).color;
    probe.remove();
    return colour;
  }, variable);
}

test("labels, readouts and headings inside a shadow root look the same selected, and can still be selected", async ({ page }) => {
  await page.goto(`${server.url}/#/devices/loopback-0`);
  const status = page.locator("ga-device-status");
  const label = status.locator(".fields dt").first();
  const readout = status.locator('[data-field="current_preset"]');
  const heading = status.locator("ga-section .title").first();
  await expect(readout).toBeVisible();

  for (const target of [label, readout, heading]) {
    // Inside a shadow root: the document's own rules cannot reach it.
    expect(await target.evaluate((el) => el.getRootNode() instanceof ShadowRoot)).toBe(true);
    expect(await target.evaluate((el) => getComputedStyle(el).userSelect)).not.toBe("none");
    expect(await selectionShows(page, target)).toBe(false);
    const colours = await target.evaluate((el) => ({ own: getComputedStyle(el).color, selected: getComputedStyle(el, "::selection").color }));
    expect(colours.selected).toBe(colours.own);
  }

  // Chromium carries the page's ::selection into shadow roots by inheritance, but that is not a
  // promise every engine keeps, and the shared sheet must hold on its own: with the page's rules
  // taken away, the same text still looks the same selected.
  const removed = await page.evaluate(() => {
    let count = 0;
    for (const sheet of Array.from(document.styleSheets)) {
      for (let i = sheet.cssRules.length - 1; i >= 0; i--) {
        if ((sheet.cssRules[i] as CSSStyleRule).selectorText?.includes("::selection")) {
          sheet.deleteRule(i);
          count++;
        }
      }
    }
    return count;
  });
  expect(removed).toBeGreaterThan(0);
  for (const target of [label, readout, heading]) expect(await selectionShows(page, target)).toBe(false);
});

test("a field keeps a visible selection in the theme's accent, in a shadow root", async ({ page }) => {
  await page.goto(`${server.url}/#/devices/loopback-0`);
  const accent = await themeColour(page, "--ga-accent");
  const accentText = await themeColour(page, "--ga-accent-text");
  expect(accent).not.toBe(TRANSPARENT);

  const name = page.getByTestId("device-name");
  await name.fill("Desk Quadro");
  expect(await name.evaluate((el) => ({ background: getComputedStyle(el, "::selection").backgroundColor, color: getComputedStyle(el, "::selection").color }))).toEqual({ background: accent, color: accentText });
  // And it is drawn: the field looks different with its text selected.
  const before = await name.screenshot({ animations: "disabled" });
  await name.evaluate((el) => (el as HTMLInputElement).select());
  expect(before.equals(await name.screenshot({ animations: "disabled" }))).toBe(false);

  // A textarea, a select and an editable block, made in a shadow root the shared sheet styles; a
  // block marked not editable is not a field.
  const made = await page.locator("ga-device-status").evaluate((host) => {
    const root = host.shadowRoot!;
    const box = document.createElement("div");
    box.innerHTML = '<textarea>typed</textarea><select><option>one</option></select><div contenteditable="true"><span>edited</span></div><div contenteditable><span>edited</span></div><div contenteditable="false"><span>fixed</span></div>';
    root.append(box);
    const background = (selector: string) => getComputedStyle(box.querySelector(selector)!, "::selection").backgroundColor;
    return { textarea: background("textarea"), select: background("select"), editable: background('[contenteditable="true"] span'), bare: background('[contenteditable=""] span'), fixed: background('[contenteditable="false"] span') };
  });
  expect(made).toEqual({ textarea: accent, select: accent, editable: accent, bare: accent, fixed: TRANSPARENT });
});

test("the page itself: text outside a shadow root looks the same selected, and a field does not", async ({ page }) => {
  await page.goto(`${server.url}/#/devices/loopback-0`);
  await page.evaluate(() => {
    const box = document.createElement("div");
    box.id = "light";
    // On top of the app, so a screenshot of it is of it.
    box.style.cssText = "position: fixed; top: 40%; left: 40%; z-index: 2147483647; padding: 8px; background: var(--ga-surface-panel)";
    box.innerHTML = "<p>Plain text in the page</p><input value='typed in the page'>";
    document.body.prepend(box);
  });
  const text = page.locator("#light p");
  expect(await text.evaluate((el) => el.getRootNode() === document)).toBe(true);
  expect(await selectionShows(page, text)).toBe(false);

  const accent = await themeColour(page, "--ga-accent");
  expect(await page.locator("#light input").evaluate((el) => getComputedStyle(el, "::selection").backgroundColor)).toBe(accent);
});
