// Vite's ?raw imports: a file's text, for tests that check what ships (test/skins.test.ts reads globals.css).
declare module "*?raw" {
  const text: string;
  export default text;
}
