import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, readdir, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  checksum, cleanEnvironment, configureRustToolchain, credentialNames, libraryChanges, nativeFiles,
  notarize, parseCredentials, parseOptions, Runner, selectIdentity, Session,
  validateCredentials, withCleanup
} from '../scripts/macos-release.mjs';

const team = 'ABCDEFGHIJ';
const fingerprint = 'A'.repeat(40);
const credentials = {
  BUILD_CERTIFICATE_BASE64: Buffer.from('fake p12 for tests').toString('base64'),
  P12_PASSWORD: 'export-secret', KEYCHAIN_PASSWORD: 'keychain-secret',
  APPLE_ID: 'builder@example.invalid', APPLE_TEAM_ID: team,
  APPLE_APP_SPECIFIC_PASSWORD: 'app-specific-secret'
};

async function temporary(t) {
  const directory = await mkdtemp(join(tmpdir(), 'napstr-release-test-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return directory;
}

test('credential file is parsed as data without shell expansion', () => {
  const values = parseCredentials('# comment\nP12_PASSWORD="$(touch /tmp/should-not-exist)"\nAPPLE_ID=literal@example.invalid\n');
  assert.equal(values.P12_PASSWORD, '$(touch /tmp/should-not-exist)');
  assert.throws(() => parseCredentials('P12_PASSWORD=a\nP12_PASSWORD=b'), /duplicate/);
  assert.throws(() => parseCredentials('UNEXPECTED=value'), /Invalid/);
  assert.throws(() => parseCredentials('P12_PASSWORD="unclosed'), /Unclosed/);
  assert.throws(() => validateCredentials({}), /Missing signing credentials/);
  assert.throws(() => validateCredentials({ ...credentials, APPLE_TEAM_ID: 'invalid' }), /team ID/);
  assert.throws(() => validateCredentials({ ...credentials, BUILD_CERTIFICATE_BASE64: '!!!' }), /base64/);
  assert.equal(validateCredentials(credentials).toString(), 'fake p12 for tests');
});

test('build processes cannot inherit signing credentials or automatic Tauri signing', () => {
  const environment = cleanEnvironment({
    ...credentials, APPLE_CERTIFICATE: 'certificate', APPLE_PASSWORD: 'password',
    TAURI_SIGNING_PRIVATE_KEY: 'updater key', TAURI_CONFIG: '{}',
    DYLD_LIBRARY_PATH: '/development/lib', DYLD_INSERT_LIBRARIES: '/injected.dylib',
    NODE_OPTIONS: '--require something', CARGO_BUILD_TARGET: 'a-different-target',
    PATH: '/bin', CARGO_HOME: '/cargo'
  });
  assert.deepEqual(environment, { PATH: '/bin', CARGO_HOME: '/cargo' });
  const runner = new Runner(credentials);
  for (const name of credentialNames) assert.equal(runner.environment[name], undefined);
  assert.equal(runner.redact(`failed with ${credentials.P12_PASSWORD}`), 'failed with [REDACTED]');
});

async function fakeRust(directory) {
  await mkdir(directory, { recursive: true });
  await writeFile(join(directory, 'cargo'), '#!/bin/sh\nexec rustc "$@"\n', { mode: 0o755 });
  await writeFile(join(directory, 'rustc'), '#!/bin/sh\nprintf "fixture compiler\\n"\n', { mode: 0o755 });
}

function rustRunner(directory) {
  const runner = new Runner();
  runner.environment = { PATH: directory, HOME: directory, CARGO_HOME: join(directory, 'cargo home') };
  return runner;
}

const posixOnly = { skip: process.platform === 'win32' };

test('Rust discovery preserves an existing Cargo on PATH', posixOnly, async (t) => {
  const directory = await temporary(t);
  await fakeRust(directory);
  const runner = rustRunner(directory);
  await configureRustToolchain(runner, directory);
  assert.equal(runner.environment.PATH, directory);
  assert.equal((await runner.run('Fixture Cargo', 'cargo', ['--version'], { cwd: directory, quiet: true })).stdout.trim(), 'fixture compiler');
});

test('Rust discovery adds CARGO_HOME/bin for Cargo and its compiler subprocesses', posixOnly, async (t) => {
  const directory = await temporary(t);
  const runner = rustRunner(directory);
  await fakeRust(join(runner.environment.CARGO_HOME, 'bin'));
  await configureRustToolchain(runner, directory);
  assert.equal((await runner.run('Fixture Cargo', 'cargo', ['--version'], { cwd: directory, quiet: true })).stdout.trim(), 'fixture compiler');
});

test('missing Cargo proxies are resolved through rustup in the Rust project directory', posixOnly, async (t) => {
  const directory = await temporary(t);
  const project = join(directory, 'project/src-tauri');
  await mkdir(project, { recursive: true });
  const runner = rustRunner(directory);
  runner.environment.RUSTUP_HOME = join(directory, 'rust home');
  runner.environment.RUSTUP_TOOLCHAIN = 'configured-toolchain';
  runner.environment.EXPECTED_PROJECT = await realpath(project);
  const bin = join(runner.environment.RUSTUP_HOME, 'toolchains/configured-toolchain/bin');
  await fakeRust(bin);
  await writeFile(join(directory, 'rustup'), '#!/bin/sh\n[ "$PWD" = "$EXPECTED_PROJECT" ] && [ "$1" = which ] || exit 1\nprintf "%s/toolchains/%s/bin/%s\\n" "$RUSTUP_HOME" "$RUSTUP_TOOLCHAIN" "$2"\n', { mode: 0o755 });
  await configureRustToolchain(runner, project);
  assert.equal((await runner.run('Fixture Cargo', 'cargo', ['--version'], { cwd: project, quiet: true })).stdout.trim(), 'fixture compiler');
  assert.equal((await runner.run('Fixture compiler', 'rustc', ['--version'], { cwd: project, quiet: true })).stdout.trim(), 'fixture compiler');
});

test('Rust discovery reports missing tools and preserves rustup selection failures', posixOnly, async (t) => {
  const directory = await temporary(t);
  const runner = rustRunner(directory);
  // An unexecutable file is not a usable Cargo installation.
  await writeFile(join(directory, 'cargo'), 'not executable', { mode: 0o644 });
  await assert.rejects(configureRustToolchain(runner, directory), /Install Rust from https:\/\/rustup.rs/);
  await writeFile(join(directory, 'rustup'), '#!/bin/sh\nprintf "selected toolchain is missing\\n" >&2\nexit 1\n', { mode: 0o755 });
  await assert.rejects(configureRustToolchain(runner, directory), /selected toolchain is missing/);
});

test('identity selection rejects missing, ambiguous, wrong-team and invalid certificates', () => {
  const identity = `  1) ${fingerprint} "Developer ID Application: Example (${team})"\n`;
  assert.equal(selectIdentity(identity, team), fingerprint);
  assert.throws(() => selectIdentity(identity, 'OTHERTEAM0'), /exactly one/);
  assert.throws(() => selectIdentity(identity + identity.replace(fingerprint, 'B'.repeat(40)), team), /exactly one/);
  assert.throws(() => selectIdentity(identity.replace('Application', 'Installer'), team), /exactly one/);
  assert.throws(() => selectIdentity(identity.trimEnd() + ' (CSSMERR_TP_CERT_EXPIRED)\n', team), /exactly one/);
});

test('native files include executables without extensions, exclude symlinks, and reject signing material', async (t) => {
  const directory = await temporary(t);
  await mkdir(join(directory, 'nested'));
  await writeFile(join(directory, 'nested/tor'), Buffer.from('cffaedfe00000000', 'hex'));
  await writeFile(join(directory, 'lib.dylib'), Buffer.from('cafebabe00000000', 'hex'));
  await writeFile(join(directory, 'readme'), 'not native code');
  // Windows requires extra privileges for symlink creation.
  if (process.platform !== 'win32') {
    await symlink(join(directory, 'nested/tor'), join(directory, 'alias'));
    await symlink(directory, join(directory, 'cycle'));
  }
  assert.deepEqual(await nativeFiles(directory), [join(directory, 'nested/tor'), join(directory, 'lib.dylib')]);
  await writeFile(join(directory, 'leaked.p12'), 'must not ship');
  await assert.rejects(nativeFiles(directory), /Signing material/);
});

test('Tor library references become relative without permitting host dependencies', () => {
  const files = ['/bundle/tor', '/bundle/lib/libcrypto.3.dylib'];
  assert.deepEqual(libraryChanges(files[0], ['/usr/lib/libSystem.B.dylib', '/build/lib/libcrypto.3.dylib'], files), [
    ['/build/lib/libcrypto.3.dylib', '@loader_path/lib/libcrypto.3.dylib']
  ]);
  assert.deepEqual(libraryChanges(files[0], ['@loader_path/lib/libcrypto.3.dylib'], files), []);
  assert.throws(() => libraryChanges(files[0], ['/opt/homebrew/lib/missing.dylib'], files), /Unresolved/);
  assert.throws(() => libraryChanges(files[0], ['@rpath/libcrypto.3.dylib'], [...files, '/other/libcrypto.3.dylib']), /Unresolved/);
});

test('notarization submits once, preserves the ID, and only accepts Accepted', async () => {
  const id = '12345678-1234-1234-1234-123456789012';
  for (const status of ['Accepted', 'Invalid', 'In Progress', undefined]) {
    const calls = [];
    const runner = {
      redact: (text) => text,
      async run(label, command, args) {
        calls.push(args);
        return { stdout: JSON.stringify(args[1] === 'submit' ? { id } : { status }), stderr: '', code: 0 };
      }
    };
    const result = notarize(runner, '/app.zip', '/temporary.keychain');
    if (status === 'Accepted') await result;
    else await assert.rejects(result, /not Accepted/);
    assert.equal(calls.filter((args) => args[1] === 'submit').length, 1);
    assert.equal(calls[1][2], id);
    assert.equal(calls.some((args) => args[1] === 'log'), status !== 'Accepted');
    assert.ok(calls.every((args) => args.includes('--keychain-profile')));
  }
});

test('final checksums cover stapled bytes and are written only after cleanup', async (t) => {
  const directory = await temporary(t);
  const output = join(directory, 'Napstr.dmg');
  const session = { async cleanup() { assert.deepEqual(await readdir(directory), ['Napstr.dmg']); } };
  await withCleanup(session, {}, output, async () => {
    await writeFile(output, 'unsigned image');
    await writeFile(output, 'final signed and stapled image');
  });
  assert.equal(await readFile(`${output}.sha256`, 'utf8'), `${await checksum(output)}  Napstr.dmg\n`);
});

test('build, verification, interruption, and cleanup failures remove both publishable artifacts', async (t) => {
  const directory = await temporary(t);
  const output = join(directory, 'Napstr.dmg');
  for (const stage of ['build', 'verification', 'interrupted', 'cleanup']) {
    let cleaned = false;
    const session = { async cleanup() { cleaned = true; if (stage === 'cleanup') throw new Error('cleanup failed'); } };
    await writeFile(`${output}.sha256`, 'stale checksum');
    await assert.rejects(withCleanup(session, { interrupted: stage === 'interrupted' }, output, async () => {
      await writeFile(output, 'partly finalized image');
      if (stage === 'build' || stage === 'verification') throw new Error(`${stage} failed`);
    }), /failed|interrupted/);
    assert.equal(cleaned, true);
    assert.deepEqual(await readdir(directory), []);
  }
});

test('cleanup preserves recovery state on failure and still attempts keychain deletion', async (t) => {
  const directory = await temporary(t);
  const calls = [];
  const keychain = join(directory, 'signing.keychain-db');
  await writeFile(keychain, 'fake keychain');
  const runner = {
    redact: (value) => value,
    async run(label, command, args) {
      calls.push(args[0]);
      if (args[0] === 'list-keychains') throw new Error('restore failed');
      if (args[0] === 'delete-keychain') await rm(keychain);
      return { stdout: '' };
    }
  };
  const session = new Session(join(directory, 'state.json'), runner);
  await session.begin();
  session.state.keychain = keychain;
  session.state.searchList = ['/login.keychain-db'];
  await session.save();
  await assert.rejects(session.cleanup(), /restore failed/);
  assert.deepEqual(calls, ['list-keychains', 'delete-keychain']);
  assert.equal(JSON.parse(await readFile(session.path, 'utf8')).keychain, null);
  await assert.rejects(session.begin(), /session exists/);
});

test('unmount retries transient EBUSY, but preserves recovery state for a real failure', async (t) => {
  const directory = await temporary(t);
  const mount = join(directory, 'mounted');
  const statuses = [16, 0];
  let attempts = 0;
  const runner = {
    redact: (value) => value,
    async plist() { return { images: [{ 'system-entities': [{ 'mount-point': mount }] }] }; },
    async run(label, command, args) {
      if (args[0] === 'info') return { stdout: '' };
      attempts++;
      return { code: statuses.shift(), stdout: '', stderr: 'unmount diagnostic' };
    }
  };
  const session = new Session(join(directory, 'state.json'), runner);
  await session.begin();
  session.state.mount = mount;
  await session.save();
  await session.detach();
  assert.equal(attempts, 2);
  assert.equal(session.state.mount, null);
  assert.equal(JSON.parse(await readFile(session.path, 'utf8')).mount, null);

  statuses.push(1);
  attempts = 0;
  session.state.mount = mount;
  await session.save();
  await assert.rejects(session.detach(), /Detach final DMG failed \(1\)/);
  assert.equal(attempts, 1);
  assert.equal(JSON.parse(await readFile(session.path, 'utf8')).mount, mount);
});

test('command options reject unknown modes and unsafe release filenames', () => {
  assert.deepEqual(parseOptions(['--app', 'napstrfy', '--signed', '--ci', '--tag', 'v1.2.3-rc1']), {
    app: 'napstrfy', signed: true, ci: true, cleanup: false, tag: 'v1.2.3-rc1'
  });
  assert.throws(() => parseOptions(['--app', 'unknown']), /Choose/);
  assert.throws(() => parseOptions(['--tag', '../some-file']), /Invalid release tag/);
  assert.throws(() => parseOptions(['--tag']), /Missing value/);
  assert.throws(() => parseOptions(['--skip-verification']), /Unknown/);
});
