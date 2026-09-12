const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { test } = require('node:test');
const vm = require('node:vm');
const { resolveTypescript } = require('../src/resolve_typescript.cjs');

function project(t) {
	const root = fs.mkdtempSync(path.join(os.tmpdir(), 'zed-typescript-'));
	t.after(() => fs.rmSync(root, { recursive: true, force: true }));
	return root;
}

function json(directory, value) {
	fs.mkdirSync(directory, { recursive: true });
	fs.writeFileSync(path.join(directory, 'package.json'), JSON.stringify(value));
}

function pkg(root, key = 'typescript', { name = 'typescript', version = '7.0.2', shim = true } = {}) {
	const directory = path.join(root, 'node_modules', key);
	json(directory, { name, version });
	if (shim) {
		fs.mkdirSync(path.join(directory, 'bin'));
		fs.writeFileSync(path.join(directory, 'bin', 'tsc'), '');
	}
	return directory;
}

function native(root, platform = process.platform, arch = process.arch) {
	const name = `@typescript/typescript-${platform}-${arch}`;
	const directory = pkg(root, name, { name, shim: false });
	fs.mkdirSync(path.join(directory, 'lib'));
	const binary = path.join(directory, 'lib', platform === 'win32' ? 'tsc.exe' : 'tsc');
	fs.writeFileSync(binary, '');
	return { directory, binary };
}

test('host probe finds an external project from an unrelated extension working directory', t => {
	const root = project(t);
	const extensionWorkDirectory = project(t);
	json(root, { devDependencies: { typescript: '^7' } });
	const directory = pkg(root);
	const { binary } = native(root);
	const output = spawnSync(process.execPath, [
		'--input-type=commonjs',
		'--eval',
		fs.readFileSync(path.join(__dirname, '../src/resolve_typescript.cjs'), 'utf8'),
		'--',
		'--zed-typescript-resolve',
		root,
		'',
	], { cwd: extensionWorkDirectory, encoding: 'utf8' });
	assert.equal(output.status, 0);
	assert.deepEqual(JSON.parse(output.stdout), {
		platform: { os: process.platform, arch: process.arch },
		package: {
			packageDirectory: directory,
			native: binary,
			shim: path.join(directory, 'bin', 'tsc'),
		},
	});
});

test('FreeBSD host discovery reports its architecture with or without a local package', t => {
	for (const arch of ['x64', 'arm64']) {
		const root = project(t);
		const probe = () => {
			let stdout = '';
			const hostProcess = {
				platform: 'freebsd',
				arch,
				argv: ['node', '--zed-typescript-resolve', root, ''],
				stdout: {
					write: text => {
						stdout += text;
					},
				},
				exitCode: 0,
			};
			vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../src/resolve_typescript.cjs'), 'utf8'), {
				require,
				module: { exports: {} },
				process: hostProcess,
			});
			assert.equal(hostProcess.exitCode, 0);
			return JSON.parse(stdout);
		};
		assert.deepEqual(probe(), { platform: { os: 'freebsd', arch }, package: null });
		json(root, { dependencies: { typescript: '^7' } });
		const directory = pkg(root);
		const { binary } = native(root, 'freebsd', arch);
		const result = probe();
		assert.deepEqual(result.platform, { os: 'freebsd', arch });
		assert.equal(result.package.packageDirectory, directory);
		assert.equal(result.package.native, binary);
	}
});

test('npm aliases use the installed identity and skip TypeScript 6', t => {
	const root = project(t);
	json(root, {
		dependencies: {
			'a-old': 'npm:typescript@6',
			'z-ts7': 'npm:typescript@next',
			typescript: 'npm:@typescript/typescript6@6',
		},
	});
	pkg(root, 'a-old', { version: '6.0.2' });
	pkg(root, 'typescript', { name: '@typescript/typescript6' });
	const directory = pkg(root, 'z-ts7');
	assert.equal(resolveTypescript(root).packageDirectory, directory);
});

test('scoped alias keys and prerelease versions are supported', t => {
	const root = project(t);
	json(root, { devDependencies: { '@typescript/native': ' npm:typescript@next ' } });
	const directory = pkg(root, '@typescript/native', { version: '7.0.0-dev.20260912' });
	assert.equal(resolveTypescript(root).packageDirectory, directory);
});

test('catalog, named catalog and workspace aliases use installed metadata', t => {
	const root = project(t);
	pkg(root, 'aaa-other', { name: 'other' });
	const directory = pkg(root, 'ts7');
	for (
		const spec of ['catalog:', 'catalog:tools', 'workspace:typescript@*', 'link:../typescript', 'file:../typescript']
	) {
		json(root, { devDependencies: { 'aaa-other': spec, ts7: spec } });
		assert.equal(resolveTypescript(root).packageDirectory, directory, spec);
	}
});

test('installed versions decide eligibility for unions, hyphens and upper bounds', t => {
	const root = project(t);
	const directory = pkg(root);
	for (const spec of ['>=6 <7 || >=7', '6 - 8', '<7.1.0', 'next']) {
		json(root, { dependencies: { typescript: spec } });
		assert.equal(resolveTypescript(root).packageDirectory, directory);
	}
});

test('workspace member resolves an ancestor-hoisted package and prefers its own install', t => {
	const root = project(t);
	const child = path.join(root, 'packages', 'app');
	json(child, { dependencies: { typescript: '^7' } });
	const hoisted = pkg(root);
	assert.equal(resolveTypescript(child).packageDirectory, hoisted);
	const local = pkg(child, 'typescript', { version: '8.0.0' });
	assert.equal(resolveTypescript(child).packageDirectory, local);
});

test('pnpm symlinks resolve the platform package beside the real TypeScript directory', t => {
	const root = project(t);
	json(root, { dependencies: { typescript: '^7' } });
	const store = path.join(root, 'node_modules', '.pnpm', 'typescript@7.0.2');
	const directory = pkg(store);
	const { binary } = native(store);
	fs.symlinkSync(directory, path.join(root, 'node_modules', 'typescript'), 'junction');
	assert.equal(resolveTypescript(root).native, binary);
	assert.equal(resolveTypescript(root).packageDirectory, directory);
	assert.equal(resolveTypescript(root, 'node_modules/typescript/lib').native, binary);
});

test('a package containing the native executable needs no Node shim', t => {
	const root = project(t);
	json(root, { dependencies: { typescript: '^7' } });
	const directory = pkg(root, 'typescript', { shim: false });
	fs.mkdirSync(path.join(directory, 'lib'));
	const binary = path.join(directory, 'lib', process.platform === 'win32' ? 'tsc.exe' : 'tsc');
	fs.writeFileSync(binary, '');
	assert.deepEqual(resolveTypescript(root), { packageDirectory: directory, native: binary, shim: null });
});

test('no manifest, no installed package or no usable launcher allows managed fallback', t => {
	const root = project(t);
	assert.equal(resolveTypescript(root), null);
	json(root, { dependencies: { typescript: '^7' } });
	assert.equal(resolveTypescript(root), null);
	pkg(root, 'typescript', { shim: false });
	assert.equal(resolveTypescript(root), null);
});

test('wrong identity, malformed metadata and old versions are never selected', t => {
	const root = project(t);
	json(root, { dependencies: { typescript: '^7' } });
	const directory = pkg(root);
	for (
		const metadata of [{ name: 'other', version: '7.0.2' }, { name: 'typescript', version: '6.0.2' }, {
			name: 'typescript',
			version: '7garbage',
		}, null]
	) {
		json(directory, metadata);
		assert.equal(resolveTypescript(root), null);
	}
	fs.writeFileSync(path.join(directory, 'package.json'), '{');
	assert.equal(resolveTypescript(root), null);
});

test('explicit tsdk accepts root, lib and bin paths outside the worktree', t => {
	const root = project(t);
	const external = project(t);
	const directory = pkg(external);
	for (const suffix of ['', 'lib', 'bin', 'bin/tsc', 'bin/tsc.js']) {
		assert.equal(resolveTypescript(root, path.join(directory, suffix)).packageDirectory, directory);
	}
	assert.equal(resolveTypescript(root, path.relative(root, directory)).packageDirectory, directory);
	assert.throws(() => resolveTypescript(root, 'missing'), /tsdk.path/);
});

test('explicit platform package can launch directly without a Node shim', t => {
	const root = project(t);
	const { directory, binary } = native(root);
	assert.deepEqual(resolveTypescript(root, directory), { packageDirectory: directory, native: binary, shim: null });
});

test('optional dependencies participate in local discovery', t => {
	const root = project(t);
	json(root, { optionalDependencies: { typescript: '^7' } });
	assert.equal(resolveTypescript(root), null);
	const directory = pkg(root);
	assert.equal(resolveTypescript(root).packageDirectory, directory);
});

test('opening a monorepo root does not select an arbitrary child workspace version', t => {
	const root = project(t);
	json(root, { workspaces: ['packages/*'] });
	const child = path.join(root, 'packages', 'app');
	json(child, { dependencies: { typescript: '^7' } });
	pkg(child);
	assert.equal(resolveTypescript(root), null);
});
