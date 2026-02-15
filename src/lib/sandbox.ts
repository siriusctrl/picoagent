import { spawn } from 'child_process';
import { existsSync } from 'fs';
import { relative, resolve, sep } from 'path';

export interface SandboxOptions {
  /** Absolute path to the directory that should be writable (worktree/task dir). */
  writeRoot: string;
  /** Current working directory for the command (must be within writeRoot). */
  cwd: string;
  /** Shell command to run. */
  command: string;
  /** Timeout in ms. */
  timeoutMs?: number;
  /** Max output size to collect (chars). */
  maxOutputChars?: number;
  /** Enable/disable sandbox. */
  enabled?: boolean;
  /** bwrap binary path; defaults to /usr/bin/bwrap when present. */
  bwrapPath?: string;
  /** Whether to hide /home and /root (tmpfs) to avoid credential leakage. */
  hideHome?: boolean;
}

let cachedBwrap: string | null | undefined;

export function detectBwrap(explicitPath?: string): string | null {
  if (explicitPath) return explicitPath;
  if (cachedBwrap !== undefined) return cachedBwrap;
  const candidate = '/usr/bin/bwrap';
  cachedBwrap = existsSync(candidate) ? candidate : null;
  return cachedBwrap;
}

function truncate(text: string, maxLength: number): string {
  if (text.length <= maxLength) return text;
  const keep = Math.max(0, maxLength - 100);
  const half = Math.floor(keep / 2);
  const head = text.substring(0, half);
  const tail = text.substring(text.length - half);
  return `${head}\n... [${text.length - keep} chars truncated] ...\n${tail}`;
}

function assertWithin(rootAbs: string, pathAbs: string): void {
  const rel = relative(rootAbs, pathAbs);
  if (rel === '') return;
  if (rel.startsWith('..' + sep) || rel === '..') {
    throw new Error(`Refusing to run command: cwd is outside writeRoot (cwd=${pathAbs}, writeRoot=${rootAbs})`);
  }
}

export async function runSandboxedShell(opts: SandboxOptions): Promise<{ stdout: string; stderr: string; code: number | null; timedOut: boolean }>
{
  const enabled = opts.enabled !== false;
  const maxChars = opts.maxOutputChars ?? 32000;
  const timeoutMs = opts.timeoutMs ?? 30000;

  const writeRootAbs = resolve(opts.writeRoot);
  const cwdAbs = resolve(opts.cwd);
  assertWithin(writeRootAbs, cwdAbs);

  if (!enabled) {
    return runPlainShell(opts.command, cwdAbs, timeoutMs, maxChars);
  }

  const bwrap = detectBwrap(opts.bwrapPath);
  if (!bwrap) {
    // Fall back to plain exec if sandbox requested but unavailable.
    return runPlainShell(opts.command, cwdAbs, timeoutMs, maxChars);
  }

  const relCwd = relative(writeRootAbs, cwdAbs);
  // Mount writeRoot at /tmp/ws (we can create this mountpoint inside tmpfs /tmp)
  const ws = '/tmp/ws';
  const chdir = relCwd && relCwd !== '' ? `${ws}/${relCwd}` : ws;

  const args: string[] = [
    '--die-with-parent',
    '--unshare-user',
    '--unshare-pid',
    '--unshare-ipc',
    '--unshare-uts',
    '--ro-bind', '/', '/',
    '--dev', '/dev',
    '--proc', '/proc',
    '--tmpfs', '/tmp',
    '--dir', ws,
    '--bind', writeRootAbs, ws,
    '--chdir', chdir,
    '--clearenv',
  ];

  if (opts.hideHome !== false) {
    args.push('--tmpfs', '/home', '--tmpfs', '/root');
  }

  // Minimal environment: keep PATH so common tools run.
  const path = process.env.PATH ?? '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin';
  const lang = process.env.LANG ?? 'C.UTF-8';

  const setenv: Array<[string, string]> = [
    ['PATH', path],
    ['LANG', lang],
    ['HOME', ws],
    ['XDG_CACHE_HOME', `${ws}/.cache`],
    ['XDG_CONFIG_HOME', `${ws}/.config`],
    ['XDG_DATA_HOME', `${ws}/.local/share`],
    ['npm_config_cache', `${ws}/.npm-cache`],
    ['PIP_CACHE_DIR', `${ws}/.pip-cache`],
    ['PYTHONPYCACHEPREFIX', `${ws}/.pycache`],
    ['TMPDIR', '/tmp'],
  ];

  for (const [k, v] of setenv) {
    args.push('--setenv', k, v);
  }

  // Run via bash -lc so existing tool prompts/commands work.
  args.push('bash', '-lc', opts.command);

  return new Promise((resolvePromise) => {
    const child = spawn(bwrap, args, { stdio: ['ignore', 'pipe', 'pipe'] });

    let stdout = '';
    let stderr = '';
    let killedByTimeout = false;

    const timer = setTimeout(() => {
      killedByTimeout = true;
      child.kill('SIGKILL');
    }, timeoutMs);

    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');

    child.stdout.on('data', (chunk: string) => {
      stdout += chunk;
      if (stdout.length > maxChars * 2) stdout = truncate(stdout, maxChars * 2);
    });

    child.stderr.on('data', (chunk: string) => {
      stderr += chunk;
      if (stderr.length > maxChars * 2) stderr = truncate(stderr, maxChars * 2);
    });

    child.on('close', (code) => {
      clearTimeout(timer);
      resolvePromise({
        stdout: truncate(stdout, maxChars),
        stderr: truncate(stderr, maxChars),
        code,
        timedOut: killedByTimeout,
      });
    });
  });
}

async function runPlainShell(command: string, cwd: string, timeoutMs: number, maxChars: number): Promise<{ stdout: string; stderr: string; code: number | null; timedOut: boolean }> {
  // Use spawn instead of exec to avoid shell output buffering issues.
  return new Promise((resolvePromise) => {
    const child = spawn('bash', ['-lc', command], { cwd, stdio: ['ignore', 'pipe', 'pipe'] });

    let stdout = '';
    let stderr = '';
    let killedByTimeout = false;

    const timer = setTimeout(() => {
      killedByTimeout = true;
      child.kill('SIGKILL');
    }, timeoutMs);

    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');

    child.stdout.on('data', (chunk: string) => {
      stdout += chunk;
      if (stdout.length > maxChars * 2) stdout = truncate(stdout, maxChars * 2);
    });

    child.stderr.on('data', (chunk: string) => {
      stderr += chunk;
      if (stderr.length > maxChars * 2) stderr = truncate(stderr, maxChars * 2);
    });

    child.on('close', (code) => {
      clearTimeout(timer);
      resolvePromise({
        stdout: truncate(stdout, maxChars),
        stderr: truncate(stderr, maxChars),
        code,
        timedOut: killedByTimeout,
      });
    });
  });
}
