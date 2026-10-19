// The unit-test builder type-checks specs, but tsconfig.spec.json pins
// `types: ["vitest/globals"]`, so @types/node is not in scope. Declare only
// what the stylesheet contract test needs instead of pulling the whole Node
// surface into the test project. tsconfig.app.json excludes this file so the
// browser program stays free of Node types.
declare module 'node:fs' {
  export function readFileSync(path: URL | string, encoding: 'utf8'): string;
}
