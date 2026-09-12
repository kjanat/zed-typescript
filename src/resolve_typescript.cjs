// Runs on the host through Zed's process API: WASI cannot read project packages.
const fs = require('node:fs');
const path = require('node:path');

const platformPackage = `@typescript/typescript-${process.platform}-${process.arch}`;
const executable = process.platform === 'win32' ? 'tsc.exe' : 'tsc';

function readJson(filename) {
	try {
		return JSON.parse(fs.readFileSync(filename, 'utf8'));
	} catch {
		return null;
	}
}

function isFile(filename) {
	try {
		return fs.statSync(filename).isFile();
	} catch {
		return false;
	}
}

// Resolve metadata without loading package code or depending on package exports.
// Follow node_modules ancestor lookup without global module directories.
// realpath handles pnpm's links into its virtual store.
function packageDirectory(from, name) {
	if (!/^(?:@[^/\\]+\/)?[^@./\\][^/\\]*$/.test(name)) return null;
	for (let directory = from;; directory = path.dirname(directory)) {
		if (path.basename(directory) !== 'node_modules') {
			const candidate = path.join(directory, 'node_modules', name);
			if (isFile(path.join(candidate, 'package.json'))) {
				try {
					return fs.realpathSync(candidate);
				} catch {
					return null;
				}
			}
		}
		if (directory === path.dirname(directory)) break;
	}
	return null;
}

function inspectPackage(directory, explicit = false) {
	try {
		directory = fs.realpathSync(directory);
	} catch {
		return null;
	}
	const metadata = readJson(path.join(directory, 'package.json'));
	if (!metadata || (metadata.name !== 'typescript' && !(explicit && metadata.name === platformPackage))) {
		return null;
	}
	if (typeof metadata.version !== 'string' || !/^(?:[7-9]|[1-9]\d+)\.\d+\.\d+(?:[-+].*)?$/.test(metadata.version)) {
		return null;
	}
	const platformDirectory = metadata.name === platformPackage
		? directory
		: packageDirectory(directory, platformPackage);
	const native = [
		path.join(directory, 'lib', executable),
		platformDirectory && path.join(platformDirectory, 'lib', executable),
	].find(candidate => candidate && isFile(candidate)) || null;
	const shim = path.join(directory, 'bin', 'tsc');
	const result = {
		packageDirectory: directory,
		native,
		shim: isFile(shim) ? shim : null,
	};
	return result.native || result.shim ? result : null;
}

function isCandidate(key, spec) {
	if (typeof spec !== 'string') return false;
	spec = spec.trim();
	if (spec.startsWith('npm:')) return /^npm:typescript(?:@|$)/.test(spec);
	// Catalogs and workspace links hide package identity. Check the installed
	// metadata instead of trying to interpret a package manager's catalog format.
	return key === 'typescript' || /^(?:catalog|workspace|link|file):/.test(spec);
}

function resolveTypescript(root, tsdk = '') {
	root = path.resolve(root);
	if (tsdk) {
		let directory = path.resolve(root, tsdk.trim());
		if (/^tsc(?:\.js)?$/.test(path.basename(directory)) && path.basename(path.dirname(directory)) === 'bin') {
			directory = path.dirname(path.dirname(directory));
		} else if (['lib', 'bin'].includes(path.basename(directory))) {
			directory = path.dirname(directory);
		}
		const result = inspectPackage(directory, true);
		if (!result) {
			throw new Error('tsdk.path must point to an installed TypeScript 7+ package with a launcher for this platform');
		}
		return result;
	}
	const manifest = readJson(path.join(root, 'package.json'));
	if (!manifest) return null;
	for (const section of ['dependencies', 'devDependencies', 'peerDependencies', 'optionalDependencies']) {
		const dependencies = manifest[section];
		if (!dependencies || typeof dependencies !== 'object' || Array.isArray(dependencies)) continue;
		for (const key of Object.keys(dependencies).sort()) {
			if (!isCandidate(key, dependencies[key])) continue;
			const directory = packageDirectory(root, key);
			const result = directory && inspectPackage(directory);
			if (result) return result;
		}
	}
	return null;
}

module.exports = { resolveTypescript };

if (process.argv[1] === '--zed-typescript-resolve') {
	try {
		process.stdout.write(JSON.stringify({
			platform: { os: process.platform, arch: process.arch },
			package: resolveTypescript(process.argv[2], process.argv[3]),
		}));
	} catch {
		// Do not expose subprocess environment or arbitrary package contents.
		process.stdout.write(JSON.stringify({ error: 'Could not resolve a usable TypeScript 7+ package' }));
		process.exitCode = 1;
	}
}
