const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { test } = require('node:test');
const vm = require('node:vm');
const { resolveTypescript } = require('../src/resolve_typescript.cjs');
const resolverSource = fs.readFileSync(path.join(__dirname, '../src/resolve_typescript.cjs'), 'utf8');
const resolverBootstrap = fs.readFileSync(path.join(__dirname, '../src/run_resolver.cjs'), 'utf8').trim();

function hostProbe(root, tsdk = '', { throughCommandShell = false, cwd } = {}) {
	const args = [
		'--input-type=commonjs',
		'--eval',
		resolverBootstrap,
		'--',
		'--zed-typescript-resolve',
		root,
		tsdk,
	];
	const pathKey = Object.keys(process.env).find(key => key.toUpperCase() === 'PATH') || 'PATH';
	return spawnSync(
		throughCommandShell ? process.env.ComSpec : process.execPath,
		throughCommandShell ? ['/C', 'node', ...args] : args,
		{
			cwd,
			encoding: 'utf8',
			env: {
				...process.env,
				[pathKey]: `${path.dirname(process.execPath)}${path.delimiter}${process.env[pathKey] || ''}`,
				ZED_TYPESCRIPT_RESOLVER: resolverSource,
			},
		},
	);
}

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
	const output = hostProbe(root, '', { cwd: extensionWorkDirectory });
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

test('canonical TypeScript wins within a section, with sorted aliases as fallback', t => {
	const root = project(t);
	json(root, { devDependencies: { 'z-ts': 'npm:typescript@^7', 'a-ts': 'npm:typescript@^7', typescript: '^7' } });
	pkg(root, 'z-ts');
	const alias = pkg(root, 'a-ts');
	const canonical = pkg(root);
	assert.equal(resolveTypescript(root).packageDirectory, canonical);
	// Invalid metadata, old versions, missing launchers and absent packages must
	// still allow a usable alias to win.
	for (const metadata of [{ name: 'other', version: '7.0.2' }, { name: 'typescript', version: '6.0.2' }]) {
		json(canonical, metadata);
		assert.equal(resolveTypescript(root).packageDirectory, alias);
	}
	json(canonical, { name: 'typescript', version: '7.0.2' });
	fs.rmSync(path.join(canonical, 'bin'), { recursive: true });
	assert.equal(resolveTypescript(root).packageDirectory, alias);
	fs.rmSync(canonical, { recursive: true });
	assert.equal(resolveTypescript(root).packageDirectory, alias);
});

test('dependency section precedence is preserved when preferring the canonical key', t => {
	const root = project(t);
	json(root, { dependencies: { alias: 'npm:typescript@^7' }, devDependencies: { typescript: '^7' } });
	const alias = pkg(root, 'alias');
	pkg(root);
	assert.equal(resolveTypescript(root).packageDirectory, alias);
});

test('host probe reports the specific invalid tsdk error', t => {
	const root = project(t);
	const output = hostProbe(root, 'missing');
	assert.equal(output.status, 1);
	assert.deepEqual(JSON.parse(output.stdout), {
		error: 'tsdk.path must point to an installed TypeScript 7+ package with a launcher for this platform',
	});
	assert.equal(output.stderr, '');
});

test('Windows command shims preserve local discovery, managed fallback and resolver errors', {
	skip: process.platform !== 'win32',
}, t => {
	const root = path.join(project(t), 'project with spaces');
	json(root, { dependencies: { typescript: '^7' } });
	const missing = hostProbe(root, '', { throughCommandShell: true });
	assert.equal(missing.error, undefined);
	assert.equal(missing.status, 0, missing.stderr);
	assert.equal(missing.stderr, '');
	assert.deepEqual(JSON.parse(missing.stdout), {
		platform: { os: process.platform, arch: process.arch },
		package: null,
	});
	const directory = pkg(root);
	const installed = hostProbe(root, '', { throughCommandShell: true });
	assert.equal(installed.status, 0, installed.stderr);
	assert.equal(installed.stderr, '');
	assert.equal(JSON.parse(installed.stdout).package.packageDirectory, directory);
	const invalid = hostProbe(root, 'missing', { throughCommandShell: true });
	assert.equal(invalid.status, 1, invalid.stderr);
	assert.equal(invalid.stderr, '');
	assert.deepEqual(JSON.parse(invalid.stdout), {
		error: 'tsdk.path must point to an installed TypeScript 7+ package with a launcher for this platform',
	});
});

test('unexpected probe errors do not expose arbitrary error details', () => {
	let stdout = '';
	const hostProcess = {
		platform: process.platform,
		arch: process.arch,
		argv: ['node', '--zed-typescript-resolve', '/project', ''],
		stdout: {
			write: text => {
				stdout += text;
			},
		},
		exitCode: 0,
	};
	vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../src/resolve_typescript.cjs'), 'utf8'), {
		require: name =>
			name === 'node:path'
				? {
					resolve() {
						throw new Error('private error details');
					},
				}
				: require(name),
		module: { exports: {} },
		process: hostProcess,
	});
	assert.equal(hostProcess.exitCode, 1);
	assert.deepEqual(JSON.parse(stdout), { error: 'Could not resolve a usable TypeScript 7+ package' });
});

test('host probe works without Object.hasOwn for local and managed resolution', t => {
	for (const installed of [false, true]) {
		const root = project(t);
		json(root, { dependencies: { alias: 'npm:typescript@^7', typescript: '^7' } });
		let canonical;
		if (installed) {
			pkg(root, 'alias');
			canonical = pkg(root);
		}
		let stdout = '';
		const hostProcess = {
			platform: process.platform,
			arch: process.arch,
			argv: ['node', '--zed-typescript-resolve', root, ''],
			stdout: {
				write: text => {
					stdout += text;
				},
			},
			exitCode: 0,
		};
		// Remove the newer API only inside this VM, as on Node before 16.9.
		vm.runInNewContext(
			'Object.hasOwn = undefined;\n' + fs.readFileSync(path.join(__dirname, '../src/resolve_typescript.cjs'), 'utf8'),
			{
				require,
				module: { exports: {} },
				process: hostProcess,
			},
		);
		assert.equal(hostProcess.exitCode, 0);
		const result = JSON.parse(stdout);
		assert.deepEqual(result.platform, { os: process.platform, arch: process.arch });
		if (installed) assert.equal(result.package.packageDirectory, canonical);
		else assert.equal(result.package, null);
	}
});
