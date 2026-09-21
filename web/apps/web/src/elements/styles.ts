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

    /* How big a field is, in one place. A control picked its own height and padding before, so a
       text field, a menu and a readout side by side were three different heights and nothing lined
       up with its label. These are the numbers every control reads instead. The height here is the
       toolbar one; a block of fields a person reads and fills turns it up for itself below. */
    --ga-field-height: 22px;
    --ga-field-pad-x: 8px;
    --ga-field-radius: 3px;
    --ga-field-gap: 8px;      /* between controls sharing a line */
    --ga-label-gap: 12px;     /* between a label and the field it names */
    --ga-field-row-gap: 6px;  /* between one row of fields and the next */
    --ga-field-max: 320px;    /* a field that fills its column stops here, so a row stays readable */
  }
  *, *::before, *::after { box-sizing: inherit; }
  [hidden] { display: none !important; }

  /* A drag across the UI must not light up labels, readouts and headings. Text can still be
     selected (on purpose, or while testing); the selection is simply not drawn. A shadow root
     cannot be relied on to take the page's ::selection rule (Chromium passes it down by highlight
     inheritance; that is newer than shadow DOM and not universal), so this is here, in every
     element's shared sheet, as well as in index.html. Fields a person types in keep theirs. */
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
    min-height: var(--ga-field-height);
    padding: 0 var(--ga-field-pad-x);
    border: 1px solid var(--ga-border-subtle);
    border-radius: var(--ga-field-radius);
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
    max-width: 100%;
    padding: 1px var(--ga-field-pad-x);
    border: 1px solid var(--ga-border-subtle);
    border-radius: var(--ga-field-radius);
    background: var(--ga-surface-inset);
    font-variant-numeric: tabular-nums;
    /* A long value (a file path, a refusal) wraps inside its column rather than dragging the row
       sideways and taking the page with it. */
    overflow-wrap: anywhere;
  }
  .label { color: var(--ga-text-secondary); font-size: 11px; }
  .muted { color: var(--ga-text-muted); }
  .placeholder { margin: 0; padding: 12px; color: var(--ga-text-muted); }

  /* ---------------------------------------------------------------------------------------------
     A label and the field it names, one way for the whole app.

     A block of fields is one grid. Its children alternate label, field, so every row shares the
     same label column and every value starts at the same place: a person can run their eye down a
     column because there really is one. \`.pairs\` puts two of those label-and-field pairs on a
     line, still on the one grid, and gives up the second pair when the window is too narrow for it.
     Nesting a little grid per field was what made cards read as ragged pseudo columns.

     \`.fields\` is the same thing written as a definition list (dt, dd), which several pages use.
     \`.field-row\` is a run of controls sharing one line inside a field. \`.field-line\` claims only
     the sizes, for a row that lays itself out some other way.
     --------------------------------------------------------------------------------------------- */
  .fields, .field-grid {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: var(--ga-field-row-gap) var(--ga-label-gap);
    align-items: center;
    margin: 0;
  }
  .fields { padding: 8px 10px; }
  .field-grid.pairs { grid-template-columns: max-content minmax(0, 1fr) max-content minmax(0, 1fr); }
  .fields > *, .field-grid > * { min-width: 0; }
  .fields dt, .field-grid > .label { color: var(--ga-text-secondary); font-size: 11px; }
  .fields dd { margin: 0; }
  /* Something that belongs to no column: a note, or a part of its own under the fields. */
  .field-grid > .wide { grid-column: 1 / -1; }
  .field-row { display: flex; flex-wrap: wrap; align-items: center; gap: var(--ga-field-gap); min-width: 0; }
  .field-row > :is(select, input:not([type="number"])) { flex: 1 1 12em; min-width: 0; max-width: var(--ga-field-max); }

  /* Fields a person reads and fills are taller than a toolbar button, and everything beside a
     label takes that one height: the label is then centred on its field, and a menu, a text field,
     a switch and a readout on one row sit on one line. */
  .fields, .field-grid, .field-row, .field-line { --ga-field-height: 24px; }
  :is(.fields, .field-grid, .field-row, .field-line) .readout {
    display: inline-flex;
    align-items: center;
    min-height: var(--ga-field-height);
    padding: 0 var(--ga-field-pad-x);
  }
  /* A menu or a text field fills its column, so values share a left and a right edge; a number, a
     switch or a chip stays the size it needs. */
  .fields dd > :is(select, input:not([type="number"])), .field-grid > :is(select, input:not([type="number"])) {
    width: 100%;
    max-width: var(--ga-field-max);
  }
  .field-grid > :is(button, input[type="number"]) { justify-self: start; }
  .fields dd > .readout:only-child, .field-grid > .readout { display: flex; max-width: var(--ga-field-max); }

  /* Too narrow for two pairs on a line: they stack, still on the grid and still one label column.
     On a phone the label goes above its field instead, which is the only way a long label and a
     usable field both fit; every label still starts at the same edge. */
  @media (max-width: 560px) {
    .field-grid.pairs { grid-template-columns: max-content minmax(0, 1fr); }
  }
  @media (max-width: 480px) {
    .fields, .field-grid, .field-grid.pairs { grid-template-columns: minmax(0, 1fr); }
    .fields dd > :is(select, input:not([type="number"])), .field-grid > :is(select, input:not([type="number"])) { max-width: none; }
    .fields dd > .readout:only-child, .field-grid > .readout { max-width: none; }
  }
`);
