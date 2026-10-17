# Home page mocks

Three directions for the home page, kept outside `src/pages` so the site build and its link check never see them. They are not deployed.

- `manpage.astro`: the page as `man 1 niri-computer-use`. Text only.
- `homepage.astro`: a late-90s project home page on the browser's grey, with one small still (`errand-thumb.jpg`) that links to the 19-second clip.
- `menuconfig.astro`: the page as `make menuconfig`, with the guardrails and policy keys as config entries.

To look at them, copy them into the site and run the dev server from `site/`:

```sh
mkdir -p src/pages/mock public/mock
cp mocks/*.astro src/pages/mock/
cp mocks/errand-thumb.jpg public/mock/
npm run dev
```

Then open `http://localhost:4321/niri-computer-use/mock/manpage/`, `…/homepage/` or `…/menuconfig/`. Remove `src/pages/mock` and `public/mock` before building: `npm run build` fails its link check on pages without a Markdown version.
