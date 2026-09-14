// Entry point. Fonts are bundled from Fontsource (SIL OFL) so the UI works offline and makes no
// third-party requests: Josefin Sans for headings, titles and large values; Inter for labels.

import "@fontsource-variable/inter";
import "@fontsource-variable/josefin-sans";

const title = document.createElement("h1");
title.textContent = "Gazelle";
title.style.fontFamily = "'Josefin Sans Variable', system-ui, sans-serif";
document.body.append(title);
