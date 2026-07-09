# TypeScript for Zed

Zed extension that runs the TypeScript 7 language server.

It attaches to Zed's existing JavaScript, JSX, TypeScript, and TSX languages.

Further read: [Announcing TypeScript 7.0].

[Announcing TypeScript 7.0]: https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/

## Server Resolution

The extension resolves the server in this order:

1. Reads `lsp.typescript.settings.updateChannel`, defaulting to `latest`.
2. Installs `typescript` with Zed's npm helper.
3. Runs `node node_modules/typescript/bin/tsc --lsp --stdio` with Zed's bundled Node.

Supported update channels:

| Value    | Install behavior                                                  |
| -------- | ----------------------------------------------------------------- |
| `latest` | Install the latest stable `typescript` package.                   |
| `next`   | Install `typescript@next`, matching TypeScript's nightly channel. |

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
It reads `updateChannel` from `lsp.typescript.settings` for extension install behavior and removes
that key before forwarding the remaining settings to TypeScript.

```jsonc
{
  "lsp": {
    "typescript": {
      "initialization_options": {},
      "settings": {
        "updateChannel": "next" // optional, defaults to "latest"
      }
    }
  }
}
```

<!-- markdownlint-disable-file MD013 -->
