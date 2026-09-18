// Shared look, taking its cues from dense DAW mixers: small Inter labels, Josefin Sans titles,
// flat controls, and readouts in dark inset boxes. Every colour is a theme variable (--ga-*).

function stylesheet(css: string): CSSStyleSheet {
  const s = new CSSStyleSheet();
  s.replaceSync(css);
  return s;
}

export const shared = stylesheet(`
  :host {
    box-sizing: border-box;
    color: var(--ga-text-primary);
    font: 12px/1.4 "Inter Variable", system-ui, sans-serif;
  }
  *, *::before, *::after { box-sizing: inherit; }
  [hidden] { display: none !important; }

  /* A drag across the UI must not light up labels, readouts and headings. Text can still be
     selected (on purpose, or while testing); the selection is simply not drawn. A shadow root does
     not take the page's ::selection rule, so this is here, in every element's shared sheet, as
     well as in index.html. Fields a person types in keep a visible selection. */
  ::selection { background: transparent; }
  :is(input, textarea, select, [contenteditable]:not([contenteditable="false"]))::selection,
  [contenteditable]:not([contenteditable="false"]) ::selection {
    background: var(--ga-accent);
    color: var(--ga-accent-text);
  }

  h1, h2, h3, .title {
    margin: 0;
    font-family: "Josefin Sans Variable", system-ui, sans-serif;
    font-weight: 600;
    letter-spacing: 0.02em;
  }
  h1 { font-size: 20px; }
  h2 { font-size: 15px; }

  button, input, select {
    min-height: 22px;
    padding: 0 8px;
    border: 1px solid var(--ga-border-subtle);
    border-radius: 3px;
    background: var(--ga-control-background);
    color: var(--ga-control-text);
    font: inherit;
  }
  button, select { cursor: pointer; }
  button:hover:not(:disabled), select:hover:not(:disabled) { background: var(--ga-control-hover); }
  button:active:not(:disabled), button[aria-pressed="true"] { background: var(--ga-control-active); }
  input { background: var(--ga-surface-inset); }
  input::placeholder { color: var(--ga-text-muted); }
  :is(button, input, select):disabled {
    background: var(--ga-control-disabled);
    color: var(--ga-control-disabled-text);
    cursor: not-allowed;
  }
  :is(button, input, select, a):focus-visible { outline: 2px solid var(--ga-focus); outline-offset: 1px; }

  a { color: inherit; text-decoration: none; }

  .readout {
    display: inline-block;
    min-width: 5em;
    padding: 1px 6px;
    border: 1px solid var(--ga-border-subtle);
    border-radius: 3px;
    background: var(--ga-surface-inset);
    font-variant-numeric: tabular-nums;
  }
  .label { color: var(--ga-text-secondary); font-size: 11px; }
  .muted { color: var(--ga-text-muted); }
  .placeholder { margin: 0; padding: 12px; color: var(--ga-text-muted); }

  .fields {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 6px 12px;
    align-items: center;
    margin: 0;
    padding: 8px 10px;
  }
  .fields dt { color: var(--ga-text-secondary); font-size: 11px; }
  .fields dd { margin: 0; }
`);
