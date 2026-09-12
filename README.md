# TypeScript for Zed

Zed extension that runs the [TypeScript 7] native language server.

It attaches to Zed's existing JavaScript, JSX, TypeScript, and TSX languages.

[TypeScript 7]: https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/ "Announcing TypeScript 7.0"

## How the server runs

TypeScript 7's language server is a native executable that ships inside the platform-specific
`@typescript/typescript-<platform>-<arch>` npm packages, launched in LSP mode:

```sh
tsc --lsp --stdio
```

The extension executes that native binary directly when it can locate it next to the resolved
`typescript` package (npm/bun hoisted layouts, pnpm on Linux/macOS). Otherwise it falls back to the
package's `bin/tsc` Node launcher — Microsoft's own resolution logic — via the worktree's `node`
(volta/fnm shims included) or Zed's bundled Node. No Node process stays in the middle on the native
path.

## Server resolution

The extension resolves the TypeScript 7+ package to run, preferring the project's own copy:

1. `tsdk.path` (explicit, version checked). Accepts the package root, its `lib` dir (VS Code
   `typescript.tsdk` convention), a `bin/tsc` path, or a platform package containing the native
   binary.
2. Any dep in the worktree `package.json` (`dependencies`, `devDependencies`, `peerDependencies`, or
   `optionalDependencies`) whose effective package name is `typescript`: a direct dependency or an
   `npm:` alias under any key (such as `"@typescript/native": "npm:typescript@^7"`). Verifies that
   the installed package is >=7 and has a usable launcher. (Skips `@typescript/typescript6` compat
   aliases.)
3. Otherwise: managed `npm install typescript` into the extension's own directory (version >=7
   enforced).

Project and host discovery run a short-lived helper using Node from the worktree's `PATH`, falling
back to Zed's Node runtime when no Node is found there. The helper uses the extension's
`process:exec` capability. This allows it to inspect installed packages outside the extension's WASM
filesystem. It follows ancestor `node_modules` directories and pnpm symlinks, checks the installed
package name and version, and returns the native binary or Node launcher path. `catalog:`, named
catalogs, and `workspace:`/`link:`/`file:` dependencies are identified from their installed
metadata, including aliases. No catalog parsing or package-manager command is needed. Missing,
outdated, or unusable project installations fall back to the managed install; an invalid explicit
`tsdk.path` reports an error.

Discovery starts from the opened worktree's `package.json`. It does not choose among child
workspaces with different TypeScript versions, and Yarn PnP layouts without `node_modules` are not
supported by automatic discovery. Use `tsdk.path` to select a specific installation.

For a FreeBSD server host, install Node and make `node` available on the worktree's `PATH`. Zed's
managed Node downloader does not support FreeBSD. The helper detects the server host's OS and
architecture, including FreeBSD x64 and arm64, to locate the corresponding native platform package.
Discovery does not use Zed's restricted platform enums. Managed installations additionally need an
npm runtime usable by Zed on the server host.

For managed installs, `version` wins over `updateChannel`:

| Setting                   | Install behavior                                                    |
| ------------------------- | ------------------------------------------------------------------- |
| `version`                 | Install any npm version spec, such as `7.0.2`, `next`, or `^7.0.0`. |
| `updateChannel: "latest"` | Install the latest stable `typescript` package.                     |
| `updateChannel: "next"`   | Install `typescript@next`, matching TypeScript's nightly channel.   |

TypeScript 6 and older are rejected — for those, use Zed's built-in TypeScript support instead.

## Settings

All configuration lives under `lsp.typescript` in Zed settings. The extension owns a small set of
launch/install keys; **everything else in `settings` is forwarded verbatim to the language server**,
which pulls it via `workspace/configuration`.

### Extension-owned settings

| Setting             | Meaning                                                                                                     |
| ------------------- | ----------------------------------------------------------------------------------------------------------- |
| `version`           | npm version spec to install (wins over `updateChannel`).                                                    |
| `updateChannel`     | `"latest"` or `"next"`.                                                                                     |
| `tsdk.path`         | Explicit TypeScript package location (see [Server resolution](#server-resolution)).                         |
| `server.pprofDir`   | Passes `--pprofDir` so the server writes pprof CPU/memory profiles there.                                   |
| `server.goMemLimit` | Sets `GOMEMLIMIT` for the server. Integer bytes with optional `B`/`KiB`/`MiB`/`GiB`/`TiB` suffix, or `off`. |
| `server.args`       | Extra CLI args appended to `--lsp --stdio` — forwards future server flags without an extension update.      |
| `server.env`        | Extra environment variables for the server process (e.g. `GOGC`, debug vars).                               |

Dotted (`"server.pprofDir": …`) and nested (`"server": {"pprofDir": …}`) forms are both accepted;
the nested form wins when both are set.

```jsonc
{
  "lsp": {
    "typescript": {
      "settings": {
        "updateChannel": "next",
        "server": {
          "pprofDir": "./.typescript-pprof",
          "goMemLimit": "2048MiB",
          "env": { "GOGC": "50" }
        }
      }
    }
  }
}
```

The current server flag surface in `--lsp` mode is `--stdio`, `--pipe <name>`, `--socket <addr>`,
and `--pprofDir <dir>`; anything new lands via `server.args`.

### Custom binary

`lsp.typescript.binary` overrides how the server is launched entirely:

```jsonc
{
  "lsp": {
    "typescript": {
      "binary": {
        // a path ending in tsc/tsc.js/tsc.exe is launched as the server itself;
        // any other path is treated as a custom node to run the resolved tsc with
        "path": "/path/to/tsc",
        "arguments": ["--lsp", "--stdio"], // optional: full args override
        "env": { "GOMEMLIMIT": "4GiB" } // optional: highest-precedence env
      }
    }
  }
}
```

Environment precedence, lowest to highest: shell environment, `server.env`, `server.goMemLimit`,
`binary.env`.

### Language server configuration (forwarded)

The server requests the configuration sections `js/ts`, `typescript`, `javascript`, and `editor`,
and merges them with ascending precedence `editor` → `javascript` → `typescript` → `js/ts`. Put
those sections directly in `settings` — they are forwarded to the server:

```jsonc
{
  "lsp": {
    "typescript": {
      "settings": {
        "typescript": {
          "preferences": {
            "quoteStyle": "single",
            "importModuleSpecifier": "non-relative",
            "preferTypeOnlyAutoImports": true
          }
        }
      }
    }
  }
}
```

Matching Zed's built-in TypeScript support, the extension enables all inlay hint kinds and both code
lens kinds server-side by default (Zed's own `inlay_hints` setting still controls whether hints are
displayed). Your settings deep-merge over these defaults and win at leaf level, so e.g.
`"typescript": {"inlayHints": {"parameterNames": {"enabled": "none"}}}` turns one hint kind off
while the rest keep their defaults.

VS Code-style dotted keys are also accepted and expanded, so configuration copied from TypeScript
docs works as-is (nested form wins on conflict):

```jsonc
{
  "lsp": {
    "typescript": {
      "settings": {
        "typescript.inlayHints.parameterNames.enabled": "all",
        "js/ts.implicitProjectConfig.checkJs": true
      }
    }
  }
}
```

Option groups the server reads (same names as the VS Code TypeScript settings, minus the
`typescript.`/`javascript.` prefix): `inlayHints.*`, `preferences.*` (quote style, auto-imports,
module specifiers, organize imports), `suggest.*`, `format.*`, `referencesCodeLens.*` /
`implementationsCodeLens.*`, `validate.*`, `workspaceSymbols.*`, `autoClosingTags`, and
`implicitProjectConfig.*` under `js/ts`. The authoritative list is [`UserPreferences`] in the server
source.

[`UserPreferences`]: https://github.com/microsoft/typescript-go/blob/168e7015edf98244febc8f4ae450b673b5d195d7/internal/ls/lsutil/userpreferences.go#L37-L193

### Initialization options (forwarded)

`initialization_options` are passed to the server verbatim at `initialize`.\
Known upstream options ([`InitializationOptions`], read in [`server.go`]):

| Option                             | Upstream meaning                                                     |
| ---------------------------------- | -------------------------------------------------------------------- |
| `disablePushDiagnostics`           | Disable automatic diagnostic pushes.                                 |
| `codeLensShowLocationsCommandName` | Client command used by resolved references/implementations CodeLens. |
| `userPreferences`                  | Initial user preferences (same shape as the sections above).         |
| `enableTelemetry`                  | Enable server telemetry events.                                      |
| `logVerbosity`                     | Initial server log verbosity.                                        |

```jsonc
{
  "lsp": {
    "typescript": {
      "initialization_options": {
        "enableTelemetry": false,
        "logVerbosity": 2
      }
    }
  }
}
```

Preferences set via the configuration sections update live on `workspace/didChangeConfiguration`;
`initialization_options` only apply at server startup.

[`InitializationOptions`]: https://github.com/microsoft/typescript-go/blob/168e7015edf98244febc8f4ae450b673b5d195d7/internal/lsp/lsproto/lsp_generated.go#L8774-L8790
[`server.go`]: https://github.com/microsoft/typescript-go/blob/main/internal/lsp/server.go

## Known limits

- TypeScript 7 has no stable programmatic API yet, so embedded-language workflows (Vue, MDX, Astro,
  Svelte, Angular templates) are not supported by the upstream server.
- `codeLensShowLocationsCommandName` requires client-side command support that Zed extensions cannot
  register; code lenses resolve, but the "show locations" command is editor-dependent.

## Dev Install

Prerequisite: Rust installed with `rustup`.

In Zed, run `zed: install dev extension` and select this directory.

Run `cargo test` for the Rust tests and `node --test tests/resolve_typescript.test.cjs` for the host
resolver tests. After changing the extension, use **Rebuild** on Zed's Extensions page.

After edits, rebuild from the Extensions page.\
For logs, run Zed with `zed --foreground` or use `zed: open log`.

<!-- markdownlint-disable-file MD013 -->
