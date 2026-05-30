// Minimal ambient declarations for the few Node built-ins used by tests
// that read source files at runtime (e.g. the layout-matrix suite reads
// the source stylesheets via `node:fs`). The `@types/node` package is
// intentionally not a dependency of the UI workspace, so we declare only
// the tiny surface actually used here. vitest runs under Node, so these
// resolve at runtime; this file only satisfies `svelte-check`/`tsc`.

declare module "node:fs" {
  export function readFileSync(path: string, encoding: "utf8"): string;
}

declare module "node:path" {
  export function dirname(p: string): string;
  export function resolve(...segments: string[]): string;
  export function join(...segments: string[]): string;
}

declare module "node:url" {
  export function fileURLToPath(url: string | URL): string;
}
