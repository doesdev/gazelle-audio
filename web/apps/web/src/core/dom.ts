// A small element builder: `h("button", { class: "mute", "on:click": toggle }, "M")`.
// Attribute values of `false`, `null` or `undefined` are left off; `true` sets an empty attribute.
// Reactive bindings are done by elements with effects, so this stays static.

export type Child = Node | string | number | null | undefined | false | readonly Child[];

export type Props = Record<string, string | number | boolean | null | undefined | EventListener>;

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, props: Props = {}, ...children: Child[]): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key.startsWith("on:")) {
      if (typeof value === "function") element.addEventListener(key.slice(3), value);
    } else if (value === true) {
      element.setAttribute(key, "");
    } else if (value !== false && value !== null && value !== undefined && typeof value !== "function") {
      element.setAttribute(key, String(value));
    }
  }
  append(element, children);
  return element;
}

export function append(parent: Node, children: readonly Child[]): void {
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    if (Array.isArray(child)) append(parent, child);
    else parent.appendChild(child instanceof Node ? child : document.createTextNode(String(child)));
  }
}
