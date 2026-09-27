#!/usr/bin/env node

// Shared local/CI packaging. This command never creates or uploads a release.
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, createReadStream } from 'node:fs';
import { access, chmod, lstat, mkdir, mkdtemp, open, readFile, readdir, readlink, realpath, rename, rm, stat, symlink, writeFile } from 'node:fs/promises';
import { arch, homedir, platform, tmpdir } from 'node:os';
import { basename, delimiter, dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const credentialNames = [
  'BUILD_CERTIFICATE_BASE64', 'P12_PASSWORD', 'KEYCHAIN_PASSWORD',
  'APPLE_ID', 'APPLE_TEAM_ID', 'APPLE_APP_SPECIFIC_PASSWORD'
];
const products = {
  napstr: { directory: root, name: 'Napstr', binary: 'napstr', identifier: 'social.napstr.desktop' },
  napstrfy: { directory: join(root, 'android'), name: 'Napstrfy', binary: 'napstrfy', identifier: 'net.napstr.nostrfy' }
};
const profile = 'napstr-notarization';
const machMagic = new Set(['feedface', 'cefaedfe', 'feedfacf', 'cffaedfe', 'cafebabe', 'bebafeca', 'cafebabf', 'bfbafeca']);

export function parseCredentials(contents) {
  const values = {};
  for (const line of contents.split(/\r?\n/)) {
    if (!line.trim() || line.trimStart().startsWith('#')) continue;
    const match = /^\s*([A-Z_][A-Z_0-9]*)\s*=\s*(.*?)\s*$/.exec(line);
    if (!match || !credentialNames.includes(match[1]) || Object.hasOwn(values, match[1])) {
      throw new Error('Invalid or duplicate key in .env.macos-release');
    }
    let value = match[2];
    if (value.startsWith('"') || value.startsWith("'")) {
      if (value.length < 2 || value.at(-1) !== value[0]) throw new Error('Unclosed credential quote');
      value = value.slice(1, -1);
    }
    // Parse data only: never source the file or expand shell expressions.
    values[match[1]] = value;
  }
  return values;
}

export function validateCredentials(values) {
  const missing = credentialNames.filter((name) => !values[name]?.trim());
  if (missing.length) throw new Error(`Missing signing credentials: ${missing.join(', ')}`);
  if (!/^[A-Z0-9]{10}$/.test(values.APPLE_TEAM_ID)) throw new Error('APPLE_TEAM_ID must be a ten-character team ID');
  const encoded = values.BUILD_CERTIFICATE_BASE64.replace(/\s/g, '');
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(encoded) || encoded.length % 4 !== 0) {
    throw new Error('BUILD_CERTIFICATE_BASE64 must contain a base64-encoded P12');
  }
  const certificate = Buffer.from(encoded, 'base64');
  if (certificate.toString('base64') !== encoded) throw new Error('Invalid base64 certificate');
  return certificate;
}

export function cleanEnvironment(environment = process.env) {
  return Object.fromEntries(Object.entries(environment).filter(([name]) =>
    !credentialNames.includes(name) && !name.startsWith('APPLE_') &&
    !name.startsWith('DYLD_') && !name.startsWith('TAURI_SIGNING_') &&
    !['TAURI_CONFIG', 'NODE_OPTIONS', 'CARGO_BUILD_TARGET'].includes(name)
  ));
}

export async function configureRustToolchain(runner, cwd) {
  const environment = runner.environment;
  const path = environment.PATH || '';
  const cargoBin = resolve(cwd, environment.CARGO_HOME || join(environment.HOME || homedir(), '.cargo'), 'bin');
  async function findExecutable(name, searchPath) {
    for (const directory of searchPath.split(delimiter)) {
      const candidate = resolve(cwd, directory, name);
      try {
        await access(candidate, constants.X_OK);
        if ((await stat(candidate)).isFile()) return candidate;
      } catch (error) {
        if (!['ENOENT', 'ENOTDIR', 'EACCES'].includes(error.code)) throw error;
      }
    }
  }
  const prepend = (directories) => {
    // Keep npm hooks on this Node installation when adding the Rust tools.
    environment.PATH = [dirname(process.execPath), ...new Set(directories), path].join(delimiter);
  };
  if (await findExecutable('cargo', path)) return;
  if (await findExecutable('cargo', cargoBin)) {
    prepend([cargoBin]);
    return;
  }
  const rustup = await findExecutable('rustup', [path, cargoBin].join(delimiter));
  if (!rustup) {
    throw new Error('Rust/Cargo was not found. Install Rust from https://rustup.rs, then reopen your terminal and retry. For an existing installation, add its bin directory to PATH or set CARGO_HOME.');
  }
  // Ask rustup to honor the active toolchain and project overrides, including
  // installations that have rustup but are missing the cargo/rustc proxies.
  const directories = [];
  for (const tool of ['cargo', 'rustc']) {
    const result = await runner.run(`Locate Rust ${tool}`, rustup, ['which', tool], { cwd });
    const executable = result.stdout.trim();
    if (!executable || !await findExecutable(basename(executable), dirname(executable))) {
      throw new Error(`Rustup could not locate an executable ${tool}. Repair the active Rust toolchain and retry.`);
    }
    directories.push(dirname(executable));
  }
  prepend(directories);
}

export function selectIdentity(output, team) {
  const matches = [...output.matchAll(/^\s*\d+\) ([A-Fa-f0-9]{40}) "Developer ID Application: [^"\n]+ \(([A-Z0-9]{10})\)"\s*$/gm)];
  const identities = new Set(matches.filter((match) => match[2] === team).map((match) => match[1].toUpperCase()));
  if (identities.size !== 1) {
    throw new Error('Expected exactly one valid Developer ID Application identity with a private key for APPLE_TEAM_ID');
  }
  return [...identities][0];
}

export class Runner {
  constructor(credentials = {}) {
    this.secrets = Object.values(credentials).filter(Boolean).sort((a, b) => b.length - a.length);
    this.environment = cleanEnvironment();
    // npm hooks and Tauri must use the same Node installation as this helper.
    this.environment.PATH = `${dirname(process.execPath)}:${this.environment.PATH || ''}`;
    this.child = null;
    this.interrupted = false;
  }

  redact(value) {
    let text = String(value);
    for (const secret of this.secrets) text = text.split(secret).join('[REDACTED]');
    return text;
  }

  async run(label, command, args = [], { cwd = root, input, timeout = 120_000, check = true, env = {}, quiet = false } = {}) {
    if (this.interrupted) throw new Error('Build interrupted');
    if (!quiet) console.log(label);
    return await new Promise((accept, reject) => {
      const child = spawn(command, args, { cwd, env: { ...this.environment, ...env }, stdio: ['pipe', 'pipe', 'pipe'], detached: true });
      this.child = child;
      let stdout = '', stderr = '', timedOut = false;
      // Keep commands with credentials out of logs and bound diagnostic memory.
      child.stdout.on('data', (chunk) => { stdout = (stdout + chunk).slice(-2_000_000); });
      child.stderr.on('data', (chunk) => { stderr = (stderr + chunk).slice(-2_000_000); });
      const timer = setTimeout(() => { timedOut = true; this.stopChild(); }, timeout);
      const progress = quiet ? null : setInterval(() => console.log(`${label} is still running…`), 60_000);
      child.on('error', (error) => { clearTimeout(timer); clearInterval(progress); this.child = null; reject(new Error(`${label}: ${this.redact(error.message)}`)); });
      child.on('close', (code, signal) => {
        clearTimeout(timer);
        clearInterval(progress);
        this.child = null;
        if (this.interrupted || timedOut || (check && code !== 0)) {
          reject(new Error(`${label} failed (${timedOut ? 'timeout' : signal || code}).\n${this.redact(stdout + stderr).slice(-12_000)}`));
        } else accept({ stdout, stderr, code });
      });
      child.stdin.on('error', () => {});
      child.stdin.end(input);
    });
  }

  stopChild() {
    if (this.child?.pid) {
      try { process.kill(-this.child.pid, 'SIGKILL'); } catch (error) { if (error.code !== 'ESRCH') throw error; }
    }
  }

  async plist(contents) {
    const result = await this.run('Read Apple property list', '/usr/bin/plutil', ['-convert', 'json', '-o', '-', '-'], { input: contents, quiet: true });
    return JSON.parse(result.stdout);
  }
}

export class Session {
  constructor(path, runner) {
    this.path = path;
    this.runner = runner;
    this.state = { directory: null, keychain: null, searchList: null, mount: null };
  }

  async begin() {
    await mkdir(dirname(this.path), { recursive: true });
    let handle;
    try { handle = await open(this.path, 'wx', 0o600); }
    catch (error) {
      if (error.code === 'EEXIST') throw new Error('A macOS build session exists. Run npm run macos-build -- --cleanup before retrying.');
      throw error;
    }
    await handle.writeFile(JSON.stringify(this.state));
    await handle.close();
  }

  async save() {
    await writeFile(`${this.path}.tmp`, JSON.stringify(this.state), { mode: 0o600 });
    await rename(`${this.path}.tmp`, this.path);
  }

  async prepare() {
    this.state.directory = await mkdtemp(join(tmpdir(), 'napstr-macos-'));
    await chmod(this.state.directory, 0o700);
    await this.save();
  }

  async setupKeychain(credentials) {
    const runner = this.runner;
    const original = await runner.run('Read keychain search list', '/usr/bin/security', ['list-keychains', '-d', 'user']);
    this.state.searchList = original.stdout.split('\n').filter((line) => line.trim()).map((line) => JSON.parse(line.trim()));
    const keychain = join(this.state.directory, 'signing.keychain-db');
    this.state.keychain = keychain;
    await this.save();
    const p12 = join(this.state.directory, 'certificate.p12');
    await writeFile(p12, validateCredentials(credentials), { mode: 0o600, flag: 'wx' });
    const password = credentials.KEYCHAIN_PASSWORD;
    try {
      await runner.run('Create temporary keychain', '/usr/bin/security', ['create-keychain', '-p', password, keychain]);
      await runner.run('Unlock temporary keychain', '/usr/bin/security', ['unlock-keychain', '-p', password, keychain]);
      await runner.run('Set signing timeout', '/usr/bin/security', ['set-keychain-settings', '-lut', '21600', keychain]);
      await runner.run('Add temporary keychain', '/usr/bin/security', ['list-keychains', '-d', 'user', '-s', keychain, ...this.state.searchList]);
      await runner.run('Import Developer ID certificate', '/usr/bin/security', ['import', p12, '-k', keychain, '-P', credentials.P12_PASSWORD, '-T', '/usr/bin/codesign', '-T', '/usr/bin/security']);
    } finally { await rm(p12, { force: true }); }
    await runner.run('Allow unattended signing', '/usr/bin/security', ['set-key-partition-list', '-S', 'apple-tool:,apple:,codesign:', '-s', '-k', password, keychain]);
    const result = await runner.run('Select signing identity', '/usr/bin/security', ['find-identity', '-v', '-p', 'codesigning', keychain]);
    const identity = selectIdentity(result.stdout, credentials.APPLE_TEAM_ID);
    await runner.run('Validate notarization credentials', '/usr/bin/xcrun', ['notarytool', 'store-credentials', profile, '--keychain', keychain, '--apple-id', credentials.APPLE_ID, '--team-id', credentials.APPLE_TEAM_ID, '--password', credentials.APPLE_APP_SPECIFIC_PASSWORD]);
    return identity;
  }

  async detach() {
    if (!this.state.mount) return;
    const result = await this.runner.run('Inspect mounted images', '/usr/bin/hdiutil', ['info', '-plist']);
    const info = await this.runner.plist(result.stdout);
    const mounted = (info.images || []).some((image) => (image['system-entities'] || []).some((entity) => entity['mount-point'] === this.state.mount));
    if (mounted) {
      // Gatekeeper/dyld can briefly retain a handle after the final smoke check.
      // Retry EBUSY normally; never force-detach an image or hide other failures.
      for (let attempt = 0; attempt < 5; attempt++) {
        const result = await this.runner.run('Detach final DMG', '/usr/bin/hdiutil', ['detach', this.state.mount], { check: false });
        if (result.code === 0) break;
        if (result.code !== 16 || attempt === 4) throw new Error(`Detach final DMG failed (${result.code}).\n${this.runner.redact(result.stdout + result.stderr)}`);
        console.log('macOS is still using the verified image; retrying unmount…');
        await new Promise((done) => setTimeout(done, 2_000));
      }
    }
    this.state.mount = null;
    await this.save();
  }

  async cleanup() {
    const errors = [];
    const attempt = async (action) => { try { await action(); } catch (error) { errors.push(this.runner.redact(error.message)); } };
    await attempt(() => this.detach());
    await attempt(async () => {
      if (!this.state.searchList) return;
      await this.runner.run('Restore keychain search list', '/usr/bin/security', ['list-keychains', '-d', 'user', '-s', ...this.state.searchList]);
      const result = await this.runner.run('Verify restored keychain search list', '/usr/bin/security', ['list-keychains', '-d', 'user']);
      const restored = result.stdout.split('\n').filter((line) => line.trim()).map((line) => JSON.parse(line.trim()));
      if (JSON.stringify(restored) !== JSON.stringify(this.state.searchList)) throw new Error('Keychain search list was not restored');
      this.state.searchList = null;
      await this.save();
    });
    await attempt(async () => {
      if (!this.state.keychain) return;
      if (await exists(this.state.keychain)) await this.runner.run('Delete temporary keychain', '/usr/bin/security', ['delete-keychain', this.state.keychain]);
      if (await exists(this.state.keychain)) throw new Error('Temporary keychain still exists');
      this.state.keychain = null;
      await this.save();
    });
    if (errors.length) throw new Error(`Cleanup failed; recovery state retained. ${errors.join('\n')}`);
    if (this.state.directory) await rm(this.state.directory, { recursive: true, force: true });
    await rm(this.path, { force: true });
  }
}

async function exists(path) {
  try { await lstat(path); return true; } catch (error) { if (error.code === 'ENOENT') return false; throw error; }
}

export async function nativeFiles(directory) {
  const found = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) found.push(...await nativeFiles(path));
    else if (entry.isFile()) {
      if (entry.name === '.env.macos-release' || /\.(p12|keychain-db)$/.test(entry.name)) throw new Error('Signing material must never be bundled');
      const handle = await open(path, 'r');
      const magic = Buffer.alloc(4);
      try { await handle.read(magic, 0, 4, 0); } finally { await handle.close(); }
      if (machMagic.has(magic.toString('hex'))) found.push(path);
    }
  }
  return found.sort((a, b) => b.split(sep).length - a.split(sep).length || a.localeCompare(b));
}

export function libraryChanges(binary, dependencies, files) {
  return dependencies.map((dependency) => {
    if (dependency.startsWith('/usr/lib/') || dependency.startsWith('/System/Library/')) return null;
    const matches = files.filter((path) => basename(path) === basename(dependency));
    if (matches.length !== 1) throw new Error(`Unresolved bundled Tor library: ${dependency}`);
    return [dependency, `@loader_path/${relative(dirname(binary), matches[0]).split(sep).join('/')}`];
  }).filter((change) => change && change[0] !== change[1]);
}

export async function prepareTor(runner, app) {
  const directory = join(app, 'Contents/Resources/resources/tor/macos/tor');
  const files = await nativeFiles(directory);
  if (!files.includes(join(directory, 'tor'))) throw new Error('The app is missing its bundled Tor executable');
  for (const path of files) {
    const linked = await runner.run('Inspect bundled Tor libraries', '/usr/bin/otool', ['-L', path]);
    const dependencies = linked.stdout.split('\n').slice(1).map((line) => /^\s+(.+) \(compatibility version /.exec(line)?.[1]).filter(Boolean);
    const changes = libraryChanges(path, dependencies, files);
    const args = changes.flatMap(([before, after]) => ['-change', before, after]);
    if (path.endsWith('.dylib')) args.push('-id', `@loader_path/${basename(path)}`);
    if (args.length) await runner.run('Make Tor library paths relocatable', '/usr/bin/install_name_tool', [...args, path]);
  }
}

async function codeTargets(app) {
  const files = await nativeFiles(app);
  if (!files.length) throw new Error('No native code found in application');
  const bundles = new Set();
  for (const file of files) {
    for (let parent = dirname(file); parent !== app; parent = dirname(parent)) {
      if (/\.(framework|app|xpc|bundle)$/.test(parent)) bundles.add(parent);
    }
  }
  return [...new Set([...files, ...bundles])].sort((a, b) => b.split(sep).length - a.split(sep).length || a.localeCompare(b)).concat(app);
}

async function sign(runner, path, identity, keychain, runtime = true) {
  const args = ['--force', '--sign', identity];
  if (identity === '-') args.push('--timestamp=none');
  else {
    args.push('--keychain', keychain, '--timestamp');
    if (runtime) args.push('--options', 'runtime');
  }
  await runner.run(runtime ? 'Sign bundled code' : 'Sign final DMG', '/usr/bin/codesign', [...args, path]);
}

async function verifyCode(runner, path, team, { deep = false, runtime = true, identifier } = {}) {
  const args = ['--verify', '--strict', '--all-architectures'];
  if (deep) args.push('--deep');
  if (team) {
    let requirement = `anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = "${team}"`;
    if (identifier) requirement += ` and identifier "${identifier}"`;
    args.push('-R', `=${requirement}`);
  }
  await runner.run('Verify code signature', '/usr/bin/codesign', [...args, path]);
  if (!team) return;
  const details = await runner.run('Inspect Developer ID signature', '/usr/bin/codesign', ['--display', '--verbose=4', path]);
  const output = details.stdout + details.stderr;
  if (!output.includes(`TeamIdentifier=${team}\n`) || !/^Timestamp=.+/m.test(output) || (runtime && !/flags=.*\bruntime\b/.test(output))) {
    throw new Error('Signature is missing its expected team, secure timestamp, or hardened runtime');
  }
  if (runtime) {
    const entitlements = await runner.run('Verify minimal entitlements', '/usr/bin/codesign', ['--display', '--entitlements', ':-', path]);
    if (entitlements.stdout.trim() && Object.keys(await runner.plist(entitlements.stdout)).length) throw new Error('Unexpected entitlements in signed code');
  }
}

async function verifyApp(runner, app, product, expectedArch, team) {
  const info = await runner.plist(await readFile(join(app, 'Contents/Info.plist'), 'utf8'));
  if (info.CFBundleIdentifier !== product.identifier || info.CFBundleExecutable !== product.binary) throw new Error('Unexpected app bundle identity');
  if (info.CFBundleShortVersionString !== product.bundleVersion || info.CFBundleVersion !== product.bundleVersion) throw new Error('App metadata does not match the release version');
  for (const path of await nativeFiles(app)) {
    const result = await runner.run('Verify native architecture', '/usr/bin/lipo', ['-archs', path]);
    if (!result.stdout.trim().split(/\s+/).includes(expectedArch)) throw new Error(`Wrong architecture: ${relative(app, path)}`);
  }
  for (const path of await codeTargets(app)) await verifyCode(runner, path, team, { deep: path === app, identifier: path === app ? product.identifier : undefined });
}

export async function notarize(runner, artifact, keychain) {
  const authentication = ['--keychain', keychain, '--keychain-profile', profile];
  const submission = await runner.run('Submit notarization', '/usr/bin/xcrun', ['notarytool', 'submit', artifact, ...authentication, '--output-format', 'json'], { timeout: 900_000 });
  const id = JSON.parse(submission.stdout).id;
  if (typeof id !== 'string' || !/^[a-f0-9-]{36}$/i.test(id)) throw new Error('Notarization did not return a submission ID');
  console.log(`Notarization submission: ${id}`);
  const result = await runner.run('Wait for notarization', '/usr/bin/xcrun', ['notarytool', 'wait', id, ...authentication, '--timeout', '30m', '--output-format', 'json'], { timeout: 1_860_000, check: false });
  let status;
  try { status = JSON.parse(result.stdout).status; } catch { status = 'Unknown'; }
  if (result.code !== 0 || status !== 'Accepted') {
    const log = await runner.run('Read notarization diagnostic', '/usr/bin/xcrun', ['notarytool', 'log', id, ...authentication], { check: false });
    throw new Error(`Notarization ${id} was not Accepted (${status}).\n${runner.redact(result.stdout + result.stderr + log.stdout + log.stderr)}`);
  }
}

async function staple(runner, artifact) {
  await runner.run('Staple notarization ticket', '/usr/bin/xcrun', ['stapler', 'staple', artifact]);
  await runner.run('Validate notarization ticket', '/usr/bin/xcrun', ['stapler', 'validate', artifact]);
}

async function verifyImage(runner, session, output, product, expectedArch, team) {
  await runner.run('Verify final DMG integrity', '/usr/bin/hdiutil', ['verify', output], { timeout: 600_000 });
  if (team) {
    await verifyCode(runner, output, team, { runtime: false });
    await runner.run('Validate DMG ticket', '/usr/bin/xcrun', ['stapler', 'validate', output]);
    await runner.run('Assess DMG with Gatekeeper', '/usr/sbin/spctl', ['--assess', '--type', 'open', '--context', 'context:primary-signature', '--verbose=2', output]);
  }
  const mount = join(session.state.directory, 'mounted');
  await mkdir(mount);
  session.state.mount = await realpath(mount);
  await session.save();
  await runner.run('Mount final DMG read-only', '/usr/bin/hdiutil', ['attach', output, '-readonly', '-nobrowse', '-mountpoint', session.state.mount]);
  if (await readlink(join(mount, 'Applications')) !== '/Applications') throw new Error('Final DMG is missing its Applications shortcut');
  const app = join(mount, `${product.name}.app`);
  await verifyApp(runner, app, product, expectedArch, team);
  if (team) {
    await runner.run('Validate app ticket from final DMG', '/usr/bin/xcrun', ['stapler', 'validate', app]);
    await runner.run('Assess app with Gatekeeper', '/usr/sbin/spctl', ['--assess', '--type', 'execute', '--verbose=2', app]);
  }
  if (product.name === 'Napstr') {
    // No DYLD overrides: this exercises the exact hardened runtime users get.
    await runner.run('Smoke-test bundled Tor from final DMG', join(app, 'Contents/Resources/resources/tor/macos/tor/tor'), ['--version']);
  }
}

export async function checksum(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}

// Cleanup is a release gate, including when build/signing/verification fails.
export async function withCleanup(session, runner, output, operation) {
  let failure;
  try { await operation(); } catch (error) { failure = error; }
  if (runner.interrupted && !failure) failure = new Error('Build interrupted');
  runner.interrupted = false;
  try { await session.cleanup(); }
  catch (error) { failure = new Error([failure?.message, error.message].filter(Boolean).join('\n')); }
  if (failure) {
    await rm(output, { force: true });
    await rm(`${output}.sha256`, { force: true });
    throw failure;
  }
  try {
    const digest = await checksum(output);
    await writeFile(`${output}.sha256`, `${digest}  ${basename(output)}\n`);
    if (await checksum(output) !== digest) throw new Error('Final DMG checksum verification failed');
  } catch (error) {
    await rm(output, { force: true });
    await rm(`${output}.sha256`, { force: true });
    throw error;
  }
}

export function parseOptions(args) {
  const options = { app: 'napstr', signed: false, ci: false, cleanup: false };
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (['--signed', '--ci', '--cleanup', '--help'].includes(arg)) options[arg.slice(2)] = true;
    else if (arg === '--app' || arg === '--tag') {
      if (!args[i + 1] || args[i + 1].startsWith('--')) throw new Error(`Missing value for ${arg}`);
      options[arg.slice(2)] = args[++i];
    } else throw new Error(`Unknown macOS build option: ${arg}`);
  }
  if (!Object.hasOwn(products, options.app)) throw new Error('Choose --app napstr or --app napstrfy');
  if (options.tag && !/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?$/.test(options.tag)) throw new Error('Invalid release tag');
  return options;
}

export async function main(args = process.argv.slice(2)) {
  const options = parseOptions(args);
  if (options.help) {
    console.log('Usage: npm run macos-build[:signed] -- [--ci] [--tag v1.2.3] [--cleanup]\nBuilds the current Mac architecture. Signed local builds read the root .env.macos-release; --ci reads Actions secrets. No upload.');
    return;
  }
  if (platform() !== 'darwin' || !['arm64', 'x64'].includes(arch())) throw new Error('Build on a native Apple Silicon or Intel Mac');
  const product = { ...products[options.app] };
  let credentials = {};
  if (options.signed && !options.cleanup) {
    if (options.ci) credentials = Object.fromEntries(credentialNames.map((name) => [name, process.env[name] || '']));
    else {
      const path = join(root, '.env.macos-release');
      const stat = await lstat(path);
      if (!stat.isFile() || (stat.mode & 0o777) !== 0o600) throw new Error('Use a regular root .env.macos-release file with permissions 0600');
      credentials = parseCredentials(await readFile(path, 'utf8'));
    }
    validateCredentials(credentials);
  }
  const runner = new Runner(credentials);
  const targetDir = join(product.directory, 'src-tauri/target');
  const session = new Session(join(targetDir, 'macos-release-state.json'), runner);
  if (options.cleanup) {
    if (await exists(session.path)) {
      session.state = JSON.parse(await readFile(session.path, 'utf8'));
      await session.cleanup();
    }
    return;
  }
  const native = await runner.run('Check native macOS architecture', '/usr/sbin/sysctl', ['-in', 'sysctl.proc_translated'], { check: false });
  if (native.stdout.trim() === '1') throw new Error('Rosetta builds are not supported; use native Node and a native terminal');
  const expectedArch = arch() === 'arm64' ? 'arm64' : 'x86_64';
  const target = arch() === 'arm64' ? 'aarch64-apple-darwin' : 'x86_64-apple-darwin';
  const rustDirectory = join(product.directory, 'src-tauri');
  await configureRustToolchain(runner, rustDirectory);
  await runner.run('Check Rust toolchain', 'cargo', ['--version'], { cwd: rustDirectory });
  await runner.run('Check Rust compiler', 'rustc', ['--version'], { cwd: rustDirectory });
  const config = JSON.parse(await readFile(join(product.directory, 'src-tauri/tauri.conf.json'), 'utf8'));
  const version = options.tag ? options.tag.slice(1) : config.version;
  if (typeof version !== 'string' || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?$/.test(version)) throw new Error('Invalid application version');
  product.bundleVersion = version.split('-')[0];
  const outputDir = join(targetDir, 'macos-release', options.signed ? 'signed' : 'unsigned');
  const output = join(outputDir, `${product.name}_${version}_${expectedArch}.dmg`);
  await session.begin();
  const interrupt = () => { runner.interrupted = true; runner.stopChild(); };
  process.on('SIGINT', interrupt);
  process.on('SIGTERM', interrupt);
  try {
    await withCleanup(session, runner, output, async () => {
      await mkdir(outputDir, { recursive: true });
      await rm(output, { force: true });
      await rm(`${output}.sha256`, { force: true });
      await session.prepare();
      const tauri = join(product.directory, 'node_modules/@tauri-apps/cli/tauri.js');
      if (!await exists(tauri)) throw new Error('Install dependencies with npm ci before building');
      if (product.name === 'Napstr') await runner.run('Prepare pinned Tor bundle', process.execPath, [join(root, 'scripts/prepare-tor-bundle.mjs')], { timeout: 600_000 });
      const override = { version, bundle: { macOS: { signingIdentity: null, hardenedRuntime: options.signed } } };
      // Other platforms may also have local Tor resources; never ship them on Mac.
      if (product.name === 'Napstr') override.bundle.resources = ['resources/tor/macos/**/*'];
      await runner.run('Build Tauri application', process.execPath, [tauri, 'build', '--ci', '--no-sign', '--target', target, '--bundles', 'app', '--config', JSON.stringify(override), '--', '--locked'], {
        cwd: product.directory, env: { CARGO_TARGET_DIR: targetDir }, timeout: 7_200_000
      });
      const app = join(targetDir, target, 'release/bundle/macos', `${product.name}.app`);
      // Apple bundle versions are numeric; preserve the full RC tag in the filename.
      for (const key of ['CFBundleShortVersionString', 'CFBundleVersion']) {
        await runner.run('Finalize macOS bundle version', '/usr/bin/plutil', ['-replace', key, '-string', product.bundleVersion, join(app, 'Contents/Info.plist')]);
      }
      if (product.name === 'Napstr') await prepareTor(runner, app);
      // All bundle mutations finish before the final signatures are made.
      await runner.run('Remove packaging extended attributes', '/usr/bin/xattr', ['-cr', app]);
      const identity = options.signed ? await session.setupKeychain(credentials) : '-';
      for (const path of await codeTargets(app)) await sign(runner, path, identity, session.state.keychain);
      await verifyApp(runner, app, product, expectedArch, options.signed ? credentials.APPLE_TEAM_ID : undefined);
      if (options.signed) {
        const zip = join(session.state.directory, 'app.zip');
        await runner.run('Archive signed app', '/usr/bin/ditto', ['-c', '-k', '--sequesterRsrc', '--keepParent', app, zip], { timeout: 600_000 });
        await notarize(runner, zip, session.state.keychain);
        await staple(runner, app);
        await rm(zip);
      }
      const staging = join(session.state.directory, 'payload');
      await mkdir(staging);
      await runner.run('Stage app with its notarization ticket', '/usr/bin/ditto', [app, join(staging, `${product.name}.app`)], { timeout: 600_000 });
      await symlink('/Applications', join(staging, 'Applications'));
      const intermediate = join(session.state.directory, 'uncompressed.dmg');
      await runner.run('Create uncompressed DMG', '/usr/bin/hdiutil', ['create', '-volname', product.name, '-srcfolder', staging, '-fs', 'HFS+', '-format', 'UDRW', '-nospotlight', '-verbose', intermediate], { timeout: 1_200_000 });
      await runner.run('Compress final DMG', '/usr/bin/hdiutil', ['convert', intermediate, '-format', 'UDZO', '-tasks', '2', '-verbose', '-o', output], { timeout: 1_200_000 });
      if (options.signed) {
        await sign(runner, output, identity, session.state.keychain, false);
        await notarize(runner, output, session.state.keychain);
        await staple(runner, output);
      }
      await verifyImage(runner, session, output, product, expectedArch, options.signed ? credentials.APPLE_TEAM_ID : undefined);
    });
    console.log(`Verified ${options.signed ? 'signed and notarized' : 'ad-hoc community'} DMG: ${output}\nSHA-256: ${output}.sha256`);
  } catch (error) { throw new Error(runner.redact(error.message)); }
  finally {
    process.off('SIGINT', interrupt);
    process.off('SIGTERM', interrupt);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error) => { console.error(error.message); process.exitCode = 1; });
}
