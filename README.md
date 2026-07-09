# TypeScript for Zed

Zed extension that runs the TypeScript 7 language server.

It attaches to Zed's existing JavaScript, JSX, TypeScript, and TSX languages. It
does not register or replace grammars.

## Server Resolution

The extension resolves the server in this order:

1. `lsp.typescript.binary.path` from Zed settings.
2. `tsgo` on the worktree `PATH` for native preview users.
3. Installs `typescript` and runs `tsc` through Zed's bundled Node.

The server is launched as:

```sh
tsc --lsp --stdio
```

For native preview builds on `PATH`, it launches:

```sh
tsgo --lsp --stdio
```

## Dev Install

Prerequisite: Rust installed with `rustup`.

In Zed, run `zed: install dev extension` and select this directory.

After edits, rebuild from the Extensions page. For logs, run Zed with
`zed --foreground` or use `zed: open log`.

## Settings Example

```json
{
	"lsp": {
		"typescript": {
			"binary": {
				"path": "/path/to/tsc",
				"arguments": ["--lsp", "--stdio"]
			}
		}
	}
}
```
