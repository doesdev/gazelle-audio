// Phones on the network, end to end, without leaving this machine: the Workspace page's Phones
// section, pairing, the pair page a phone opens, and revoking.
//
// The rule is that no test listens on anything but loopback, so the server runs with the debug
// build's phone seams (`phoneSeams`): its phone listener binds 127.0.0.1 on a port of its own, and a
// request carrying `x-gazelle-test-peer` is treated as coming from that address. That is how a
// request here plays a phone: through the API, or as a whole browser context whose every request
// carries the header.

import { expect, test, type Browser, type Page } from "@playwright/test";

import { startServer, TEST_PEER_HEADER, type RunningServer } from "../../../packages/client/test/integration/server.ts";

let server: RunningServer;

const PHONE = { [TEST_PEER_HEADER]: "192.168.1.50:51000" };

test.beforeAll(async () => {
  server = await startServer(["--backend", "loopback", "--dry-run"], { webUi: true, phoneSeams: true });
});

test.afterAll(async () => {
  await server?.stop();
});

interface Status {
  allow_phones: boolean;
  listening: boolean;
  port: number;
  phones: { id: string; name: string }[];
  pairing: { code: string } | null;
}

const status = async (): Promise<Status> => (await fetch(`${server.url}/api/v1/remote`)).json() as Promise<Status>;

async function setAllowed(on: boolean): Promise<void> {
  const answer = await fetch(`${server.url}/api/v1/remote`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ allow_phones: on }) });
  expect(answer.status).toBe(200);
}

/** Pair as a phone through the API, with a code started from this machine. */
async function pairByApi(name: string): Promise<{ token: string; id: string }> {
  const started = (await (await fetch(`${server.url}/api/v1/remote/pairing`, { method: "POST" })).json()) as { code: string };
  const answer = await fetch(`${server.url}/api/v1/remote/pair`, { method: "POST", headers: { ...PHONE, "content-type": "application/json" }, body: JSON.stringify({ code: started.code, name }) });
  expect(answer.status).toBe(200);
  const body = (await answer.json()) as { token: string; phone: { id: string } };
  return { token: body.token, id: body.phone.id };
}

async function revokeAll(): Promise<void> {
  for (const phone of (await status()).phones) await fetch(`${server.url}/api/v1/remote/phones/${phone.id}`, { method: "DELETE" });
  await fetch(`${server.url}/api/v1/remote/pairing`, { method: "DELETE" });
}

test.beforeEach(async () => {
  await revokeAll();
  await setAllowed(false);
});

const section = (page: Page) => page.getByTestId("phones");

test("allowing phones, pairing one and revoking it, from the Workspace page", async ({ page }) => {
  await page.goto(`${server.url}/#/workspace`);
  const phones = section(page);
  await expect(phones.getByTestId("phones-reach")).toHaveText("Off: only this computer can reach Gazelle.");
  await expect(phones.getByTestId("phones-none")).toBeVisible();
  await expect(phones.getByTestId("phones-pair")).toBeDisabled();
  await expect(phones.getByTestId("phones-firewall")).toContainText("private networks only");

  // Turning phones on takes a second click, and starts the listener at once: no restart.
  const allow = phones.getByTestId("phones-allow");
  await expect(allow).toHaveText("Allow phones on this network: Off");
  await allow.click();
  await expect(allow).toHaveText("Confirm");
  await allow.click();
  await expect(allow).toHaveText("Allow phones on this network: On");
  await expect(allow).toHaveAttribute("aria-pressed", "true");
  const now = await status();
  expect(now.allow_phones).toBe(true);
  expect(now.listening).toBe(true);
  // The phone listener answers on its own port, and only to a paired phone.
  const onListener = `http://127.0.0.1:${now.port}`;
  expect((await fetch(`${onListener}/api/v1/health`, { headers: PHONE })).status).toBe(401);

  // Pairing: a code, a QR code and a countdown.
  await phones.getByTestId("phones-pair").click();
  const code = phones.getByTestId("phones-code");
  await expect(code).toHaveText(/^[0-9A-Z]{4}-[0-9A-Z]{4}$/);
  await expect(phones.getByTestId("phones-expires")).toHaveText(/The code expires in [45]:\d\d\./);
  const shown = (await code.textContent()) ?? "";
  expect((await status()).pairing?.code).toBe(shown);
  if ((await phones.getByTestId("phones-pair-urls").locator("p").count()) > 0) {
    await expect(phones.getByTestId("phones-qr")).toBeVisible();
    await expect(phones.getByTestId("phones-pair-urls")).toContainText(`:${now.port}/pair`);
  }

  // A phone pairs with that code; the page sees it land.
  const answer = await fetch(`${onListener}/api/v1/remote/pair`, { method: "POST", headers: { ...PHONE, "content-type": "application/json" }, body: JSON.stringify({ code: shown.toLowerCase(), name: "Test phone" }) });
  expect(answer.status).toBe(200);
  const { token } = (await answer.json()) as { token: string };
  expect(answer.headers.get("set-cookie")).toMatch(/HttpOnly/);
  await expect(phones.getByTestId("phone")).toHaveCount(1);
  await expect(phones.getByTestId("phone")).toContainText("Test phone");
  await expect(phones.getByTestId("phone")).toContainText("192.168.1.50");
  await expect(phones.getByTestId("phones-just-paired")).toHaveText('Paired "Test phone".');
  await expect(phones.getByTestId("phones-pairing")).toBeHidden();

  // The token works, on both listeners; the updater stays on this computer.
  const bearer = { ...PHONE, authorization: `Bearer ${token}` };
  expect((await fetch(`${onListener}/api/v1/devices`, { headers: bearer })).status).toBe(200);
  expect((await fetch(`${server.url}/api/v1/devices`, { headers: bearer })).status).toBe(200);
  expect((await fetch(`${server.url}/api/v1/update`, { headers: bearer })).status).toBe(403);

  // Revoking takes a second click and ends the phone's access at once.
  const revoke = phones.getByTestId("phone-revoke");
  await revoke.click();
  await expect(revoke).toHaveText("Confirm");
  await revoke.click();
  await expect(phones.getByTestId("phone")).toHaveCount(0);
  expect((await fetch(`${onListener}/api/v1/devices`, { headers: bearer })).status).toBe(401);

  // Off again: the listener goes, at once.
  await allow.click();
  await expect(allow).toHaveText("Allow phones on this network: Off");
  await expect.poll(async () => fetch(`${onListener}/api/v1/health`).then(() => "answered", () => "closed")).toBe("closed");
});

/** A browser context that is a phone: every request it makes carries the test peer header. */
async function phoneContext(browser: Browser) {
  return browser.newContext({ extraHTTPHeaders: PHONE, viewport: { width: 390, height: 800 } });
}

test("a phone opening the pairing address pairs, keeps the cookie, and lands on the app", async ({ browser }) => {
  await setAllowed(true);
  const started = (await (await fetch(`${server.url}/api/v1/remote/pairing`, { method: "POST" })).json()) as { code: string };
  const context = await phoneContext(browser);
  const page = await context.newPage();
  await page.goto(`${server.url}/pair#code=${started.code}`);
  await expect(page.getByTestId("pair-page")).toBeVisible();
  await expect(page.getByTestId("pair-code")).toHaveValue(started.code);
  // The code is taken out of the address as soon as the page has it.
  expect(new URL(page.url()).hash).toBe("");
  await page.getByTestId("pair-name").fill("Pocket");
  await page.getByTestId("pair-submit").click();
  await page.waitForURL(`${server.url}/`);
  await expect(page.getByTestId("connection")).toHaveAttribute("data-state", "open");
  const cookies = await context.cookies();
  expect(cookies.find((c) => c.name === "gazelle_token")?.httpOnly).toBe(true);
  expect((await status()).phones.map((p) => p.name)).toEqual(["Pocket"]);

  // On the phone, the Phones section says where phones are managed, and there is no update prompt.
  await page.goto(`${server.url}/#/workspace`);
  await expect(page.getByTestId("phones-elsewhere")).toBeVisible();
  await expect(page.getByTestId("phones-allow")).toBeHidden();
  await context.close();
});

test("a wrong code on the pair page says what to do, and a device that is not paired is told how to pair", async ({ browser }) => {
  await setAllowed(true);
  await fetch(`${server.url}/api/v1/remote/pairing`, { method: "POST" });
  const context = await phoneContext(browser);
  const page = await context.newPage();
  await page.goto(`${server.url}/pair`);
  await page.getByTestId("pair-code").fill("0000-0000");
  await page.getByTestId("pair-submit").click();
  await expect(page.getByTestId("pair-status")).toContainText("That code did not work");
  await expect(page.getByTestId("pair-submit")).toBeEnabled();

  await page.goto(`${server.url}/`);
  await expect(page.getByRole("heading", { name: "This device is not paired with Gazelle" })).toBeVisible();
  await context.close();
});

test("a phone paired through the API is listed with where it was seen", async ({ page }) => {
  await setAllowed(true);
  await pairByApi("Tablet");
  await page.goto(`${server.url}/#/workspace`);
  await expect(section(page).getByTestId("phone")).toContainText("Tablet");
  await expect(section(page).getByTestId("phone")).toContainText("last seen just now from 192.168.1.50");
  await expect(section(page).getByTestId("phones-none")).toBeHidden();
});
