# TypeScript for Zed

Zed extension that runs the TypeScript 7 language server.

It attaches to Zed's existing JavaScript, JSX, TypeScript, and TSX languages. It
does not register or replace grammars.

## Server Resolution

The extension resolves the server in this order:

1. Installs `typescript` with Zed's npm helper.
2. Runs `node node_modules/typescript/bin/tsc --lsp --stdio` with Zed's bundled Node.

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

`lsp.typescript.initialization_options` and `lsp.typescript.settings` are forwarded to the server.

<!-- markdownlint-disable-file MD013 -->
