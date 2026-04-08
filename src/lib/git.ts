import { spawnSync } from 'child_process';

export interface GitResult {
  code: number;
  stdout: string;
  stderr: string;
}

export function git(args: string[], opts: { cwd: string }): GitResult {
  const res = spawnSync('git', args, {
    cwd: opts.cwd,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });

  return {
    code: res.status ?? 0,
    stdout: res.stdout ?? '',
    stderr: res.stderr ?? '',
  };
}

export function gitOk(args: string[], opts: { cwd: string }): GitResult {
  const res = git(args, opts);
  if (res.code !== 0) {
    const msg = `git ${args.join(' ')} failed (code ${res.code})\nstdout:\n${res.stdout}\nstderr:\n${res.stderr}`;
    throw new Error(msg);
  }
  return res;
}

export function findGitRoot(cwd: string): string | null {
  const res = git(['rev-parse', '--show-toplevel'], { cwd });
  if (res.code !== 0) {
    return null;
  }

  return res.stdout.trim() || null;
}
