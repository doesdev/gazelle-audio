# Community themes

Themes in this folder are bundled with the web UI and listed in its theme picker as
`community:<file name>`. To add one, put a JSON file here and open a pull request.

A theme only lists what it changes: start from a built-in with `extends`. Everything else
comes from that theme, and finally from `gazelle-dark`.

```json
{
  "$schema": "../apps/web/themes/theme.schema.json",
  "name": "My Theme",
  "extends": "gazelle-dark",
  "colors": { "accent": "#e08a2e" },
  "meter": {
    "gradient": [
      { "at": -60, "color": "#2e7d32" },
      { "at": -6, "color": "#fbc02d" },
      { "at": 0, "color": "#e53935" }
    ]
  }
}
```

With `$schema` set, editors such as VS Code autocomplete the keys and flag mistakes.
`apps/web/themes/theme.schema.json` lists and describes every key. Meter gradient stops are
placed by level in dBFS, on a scale from -60 to 0.

For a theme of your own that isn't shared, put the file in the `themes` folder of the
Gazelle config directory, or wherever `--themes-dir` points. It appears as `user:<file name>`.
