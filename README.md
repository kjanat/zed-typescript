# TypeScript for Zed

Zed extension that runs the TypeScript 7 language server.

It attaches to Zed's existing JavaScript, JSX, TypeScript, and TSX languages.

Further read: [Announcing TypeScript 7.0].

[Announcing TypeScript 7.0]: https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/#editor-experience "Editor Experience"

## Server Resolution

The extension resolves the server in this order:

1. Uses `lsp.typescript.settings.tsdk.path` / `tsdk.path` when set.
2. Otherwise installs `typescript` with Zed's npm helper.
3. Runs `node node_modules/typescript/bin/tsc --lsp --stdio` with Zed's bundled Node.

For managed installs, `version` wins over `updateChannel`:

| Setting                   | Install behavior                                                    |
| ------------------------- | ------------------------------------------------------------------- |
| `version`                 | Install any npm version spec, such as `7.0.2`, `next`, or `^7.0.0`. |
| `updateChannel: "latest"` | Install the latest stable `typescript` package.                     |
| `updateChannel: "next"`   | Install `typescript@next`, matching TypeScript's nightly channel.   |

The installed package must resolve to TypeScript 7 or newer.

The server is launched as:

```sh
tsc --lsp --stdio
```

## Dev Install

Prerequisite: Rust installed with `rustup`.

In Zed, run `zed: install dev extension` and select this directory.

After edits, rebuild from the Extensions page.\
For logs, run Zed with `zed --foreground` or use `zed: open log`.

## Settings

This extension does not provide a custom settings UI or schema.\
It reads extension-owned keys from `lsp.typescript.settings` and removes them before forwarding the
remaining settings to TypeScript.

Use `initialization_options` for upstream TypeScript LSP initialization options.\
The upstream type is [`InitializationOptions`], and Microsoft's VS Code extension sets
`codeLensShowLocationsCommandName`, `enableTelemetry`, and `logVerbosity` during client startup in
[`client.ts`]. The server reads these options during `initialize` in [`server.go`].

Known upstream initialization options:

| Option                             | Upstream meaning                                                     |
| ---------------------------------- | -------------------------------------------------------------------- |
| `disablePushDiagnostics`           | Disable automatic diagnostic pushes.                                 |
| `codeLensShowLocationsCommandName` | Client command used by resolved references/implementations CodeLens. |
| `userPreferences`                  | TypeScript user preferences and/or formatting options.               |
| `enableTelemetry`                  | Enable server telemetry events.                                      |
| `logVerbosity`                     | Initial server log verbosity.                                        |

Example:

```jsonc
{
  "lsp": {
    "typescript": {
      "initialization_options": {
        "disablePushDiagnostics": false,
        "enableTelemetry": false,
        "logVerbosity": 3,
        "userPreferences": {
          "includeInlayParameterNameHints": "all"
        }
      }
    }
  }
}
```

Use `settings` for this Zed extension's launch/install settings:

VS Code-style dotted keys are accepted because TypeScript docs and ecosystem examples often use
them:

```jsonc
{
  "lsp": {
    "typescript": {
      "initialization_options": {},
      "settings": {
        "version": "7.0.2",
        "tsdk.path": "./node_modules/typescript",
        "server.pprofDir": "./.typescript-pprof",
        "server.goMemLimit": "2048MiB"
      }
    }
  }
}
```

Nested settings are accepted too:

```jsonc
{
  "lsp": {
    "typescript": {
      "settings": {
        "updateChannel": "next",
        "tsdk": {
          "path": "./node_modules/typescript"
        },
        "server": {
          "pprofDir": "./.typescript-pprof",
          "goMemLimit": "2048MiB"
        }
      }
    }
  }
}
```

If both forms are set for the same option, the nested setting wins. Example:

```jsonc
{
  "lsp": {
    "typescript": {
      "settings": {
        "server.pprofDir": "./ignored",
        "server": {
          "pprofDir": "./wins"
        }
      }
    }
  }
}
```

Here `./wins` is used.

[`InitializationOptions`]: https://github.com/microsoft/typescript-go/blob/main/internal/lsp/lsproto/lsp_generated.go#L8774-L8790
[`client.ts`]: https://github.com/microsoft/typescript-go/blob/main/_extension/src/client.ts#L80-L87
[`server.go`]: https://github.com/microsoft/typescript-go/blob/main/internal/lsp/server.go#L1028-L1040

<!-- markdownlint-disable-file MD013 -->
