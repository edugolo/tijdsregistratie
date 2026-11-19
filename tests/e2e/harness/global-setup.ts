import { spawn, spawnSync, ChildProcess } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import * as fs from 'node:fs';
import * as net from 'node:net';
import * as os from 'node:os';
import * as path from 'node:path';
import { Client } from 'pg';

const REPO = path.resolve(__dirname, '../../..');
const CARGO_TARGET_DIR = process.env.CARGO_TARGET_DIR ?? '/srv/cache/cargo-build/tijdsregistratie';
const SERVER_BIN = path.join(CARGO_TARGET_DIR, 'dx/app/debug/web/server');

/** Minimal KEY=VALUE reader for the repository's .env (values are never printed). */
function readDotEnv(): Record<string, string> {
  const file = path.join(REPO, '.env');
  const out: Record<string, string> = {};
  if (!fs.existsSync(file)) return out;
  for (const line of fs.readFileSync(file, 'utf8').split('\n')) {
    const m = line.match(/^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)=(.*)$/);
    if (!m) continue;
    out[m[1]] = m[2].replace(/^(['"])(.*)\1$/, '$2');
  }
  return out;
}

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.once('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const { port } = srv.address() as net.AddressInfo;
      srv.close(() => resolve(port));
    });
  });
}

function keyLine(): string {
  return `v1:${randomBytes(32).toString('base64')}\n`;
}

function stripAnsi(s: string): string {
  return s.replace(/\x1b\[[0-9;]*m/g, '');
}

export default async function globalSetup() {
  const dotenv = readDotEnv();
  const baseUrl = process.env.DATABASE_URL ?? dotenv.DATABASE_URL;
  if (!baseUrl) throw new Error('DATABASE_URL is not set (environment or .env)');

  // 1. Build once (incremental; set E2E_SKIP_BUILD=1 to reuse the existing binary).
  if (!process.env.E2E_SKIP_BUILD) {
    console.log('[e2e] dx build --package app');
    const build = spawnSync('dx', ['build', '--package', 'app'], {
      cwd: REPO,
      env: { ...process.env, ...dotenv, CARGO_TARGET_DIR },
      stdio: ['ignore', 'inherit', 'inherit'],
    });
    if (build.status !== 0) throw new Error('dx build failed');
  }
  if (!fs.existsSync(SERVER_BIN)) throw new Error(`server binary missing: ${SERVER_BIN}`);

  // 2. Throwaway database.
  const dbName = `tijd_e2e_${randomBytes(5).toString('hex')}`;
  const adminUrl = new URL(baseUrl);
  adminUrl.pathname = '/postgres';
  const e2eUrl = new URL(baseUrl);
  e2eUrl.pathname = `/${dbName}`;
  const admin = new Client({ connectionString: adminUrl.toString() });
  await admin.connect();
  await admin.query(`CREATE DATABASE "${dbName}"`);
  await admin.end();

  // 3. Throwaway keys.
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'tijd-e2e-'));
  const dataKey = path.join(tmp, 'data.key');
  const blindKey = path.join(tmp, 'blind.key');
  fs.writeFileSync(dataKey, keyLine(), { mode: 0o600 });
  fs.writeFileSync(blindKey, keyLine(), { mode: 0o600 });

  // 4. Server.
  const port = await freePort();
  const origin = `http://localhost:${port}`;
  const logFile = path.join(__dirname, '../artifacts/server.log');
  fs.mkdirSync(path.dirname(logFile), { recursive: true });
  const log = fs.createWriteStream(logFile);
  let output = '';
  const server: ChildProcess = spawn(SERVER_BIN, [], {
    cwd: path.dirname(SERVER_BIN),
    env: {
      PATH: process.env.PATH ?? '',
      DATABASE_URL: e2eUrl.toString(),
      APP_ORIGIN: origin,
      RP_ID: 'localhost',
      COOKIE_SECURE: 'false',
      DATA_KEY_FILE: dataKey,
      BLIND_INDEX_KEY_FILE: blindKey,
      TRUST_PROXY_HEADERS: 'false',
      AUTH_RATE_LIMIT_PER_MINUTE: '100000',
      IP: '127.0.0.1',
      PORT: String(port),
      RUST_LOG: process.env.RUST_LOG ?? 'info',
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const onData = (chunk: Buffer) => {
    output += stripAnsi(chunk.toString());
    // Never write the setup code or the URL into the retained log.
    log.write(stripAnsi(chunk.toString()).replace(/(Setup code:\s*)\S+/g, '$1<redacted>'));
  };
  server.stdout!.on('data', onData);
  server.stderr!.on('data', onData);
  let exited = false;
  server.on('exit', () => {
    exited = true;
  });

  const teardown = async () => {
    if (!exited) {
      server.kill('SIGTERM');
      await new Promise<void>((resolve) => {
        const t = setTimeout(() => {
          server.kill('SIGKILL');
          resolve();
        }, 5000);
        server.on('exit', () => {
          clearTimeout(t);
          resolve();
        });
      });
    }
    log.end();
    const a = new Client({ connectionString: adminUrl.toString() });
    await a.connect();
    await a.query(`DROP DATABASE IF EXISTS "${dbName}" WITH (FORCE)`);
    await a.end();
    fs.rmSync(tmp, { recursive: true, force: true });
  };

  try {
    // Wait for the server to answer.
    const deadline = Date.now() + 60_000;
    for (;;) {
      if (exited) throw new Error(`server exited early:\n${output.slice(-2000).replace(/Setup code:\s*\S+/g, 'Setup code: <redacted>')}`);
      try {
        const res = await fetch(`${origin}/`);
        if (res.status < 500) break;
      } catch {
        /* not up yet */
      }
      if (Date.now() > deadline) throw new Error('server did not start in time');
      await new Promise((r) => setTimeout(r, 250));
    }

    // 5. Setup code: logged at boot (R1) or lazily on the first page load (older builds).
    const codeRe = /Setup code:\s*([A-Za-z0-9-]+)/;
    const codeDeadline = Date.now() + 15_000;
    let match = codeRe.exec(output);
    while (!match && Date.now() < codeDeadline) {
      // The server function `setup_required` announces a code when none exists yet.
      await fetch(`${origin}/api/setup/required`).catch(() => undefined);
      await new Promise((r) => setTimeout(r, 300));
      match = codeRe.exec(output);
    }
    if (!match) throw new Error('no "Setup code:" line in the server output');

    process.env.E2E_ORIGIN = origin;
    process.env.E2E_PORT = String(port);
    process.env.E2E_SETUP_CODE = match[1];
    process.env.E2E_DB_URL = e2eUrl.toString();
  } catch (e) {
    await teardown();
    throw e;
  }
  return teardown;
}
