/// <reference types="vite/client" />

// @fontsource-variable/* packages ship css side-effect entrypoints with no
// type declarations; typescript 7 flags untyped side-effect imports, so
// declare them as ambient modules
declare module "@fontsource-variable/*";

// injected by vite `define` from package.json at build time
declare const __APP_VERSION__: string;
