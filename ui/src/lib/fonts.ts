// the dashboard's two typefaces, vendored from node_modules by fontsource so
// they load with no network at all. `main.tsx` and the storybook preview both
// import this module rather than the packages, so a story is drawn in the faces
// the product ships and the two lists cannot drift apart (#2051).
// `lib/fonts.test.ts` fails when either entry stops importing it, or when a
// family here stops being the first one `--font-sans` / `--font-mono` name
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
