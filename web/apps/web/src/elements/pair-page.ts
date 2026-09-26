// The pair page, `/pair`: what a phone opens from the QR code on the computer. A chunk of its own,
// loaded only on this path, before (and instead of) the app, since an unpaired phone may not ask
// the server for anything else.

import { h } from "../core/dom.ts";
import { codeFromFragment, defaultPhoneName, pairHere, submitPair } from "../store/pair.ts";

export function showPairPage(): void {
  const fromLink = codeFromFragment(location.hash);
  // The code has done its job in the address: take it out of the address bar and the history.
  if (location.hash !== "") history.replaceState(null, "", location.pathname);

  const code = h("input", {
    id: "pair-code",
    name: "code",
    autocomplete: "one-time-code",
    autocapitalize: "characters",
    spellcheck: "false",
    placeholder: "XXXX-XXXX",
    "data-testid": "pair-code",
    value: fromLink ?? "",
  });
  const name = h("input", { id: "pair-name", name: "name", maxlength: 60, "data-testid": "pair-name", value: defaultPhoneName(navigator.userAgent) });
  const button = h("button", { type: "submit", "data-testid": "pair-submit" }, "Pair this phone");
  const status = h("p", { role: "status", "data-testid": "pair-status" });
  const form = h(
    "form",
    {},
    h("label", { for: "pair-code" }, "Code shown on the computer"),
    code,
    h("label", { for: "pair-name" }, "A name for this phone"),
    name,
    button,
    status,
  );
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    button.disabled = true;
    status.textContent = "Pairing...";
    void submitPair(code.value, name.value, pairHere(location.origin)).then((outcome) => {
      status.textContent = outcome.message;
      status.dataset["ok"] = String(outcome.ok);
      // Paired: on to the page laid out for a phone.
      if (outcome.ok) location.replace("/#/remote");
      else button.disabled = false;
    });
  });

  const box = h(
    "div",
    { class: "boot-error pair", "data-testid": "pair-page" },
    h("h1", {}, "Pair this phone with Gazelle"),
    h("p", {}, "Pairing lets this phone control Gazelle on the computer. It lasts until you revoke it there, on the Workspace page under Phones."),
    form,
  );
  document.title = "Pair with Gazelle";
  document.body.replaceChildren(box);
  (fromLink === undefined ? code : name).focus();
}
