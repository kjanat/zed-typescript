# TypeScript for Zed

Zed extension that runs the TypeScript 7 language server.

It attaches to Zed's existing JavaScript, JSX, TypeScript, and TSX languages. It
does not register or replace grammars.

Source:
[Announcing TypeScript 7.0](https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/).

The announcement says TypeScript 7 is installed from the `typescript` npm
package, provides the new `tsc` executable, and adds Language Server Protocol
support for editors.

## Server Resolution

The extension resolves the server in this order:

1. Resolves the latest `typescript` package and requires major version 7 or
   newer.
2. Installs `typescript` with Zed's npm helper.
3. Runs `node node_modules/typescript/bin/tsc --lsp --stdio` with Zed's bundled
   Node.

The server is launched as:

```sh
tsc --lsp --stdio
```

## Dev Install

Prerequisite: Rust installed with `rustup`.

In Zed, run `zed: install dev extension` and select this directory.

After edits, rebuild from the Extensions page. For logs, run Zed with
`zed --foreground` or use `zed: open log`.

## Settings

This extension does not provide a custom settings UI or schema. It forwards raw
Zed LSP settings to the server:

```json
{
  "lsp": {
    "typescript": {
      "initialization_options": {},
      "settings": {}
    }
  }
}
```

<!-- markdownlint-disable-file MD013 -->
