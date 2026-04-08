import { basename, join, resolve } from 'path';
import { cpSync, existsSync, mkdirSync, readFileSync, unlinkSync, writeFileSync } from 'fs';
import { homedir } from 'os';
import { findGitRoot, gitOk } from './git.js';

export type RunWorkspaceMode = 'attached-git' | 'isolated-copy';

export interface RunWorkspace {
  runDir: string;
  repoDir: string;
  tasksDir: string;
  mode: RunWorkspaceMode;
}

function isoId(d = new Date()): string {
  // safe for filenames
  return d.toISOString().replace(/[:.]/g, '-');
}

function resolveBaseDir(baseDir?: string): string {
  // Prefer a non-$HOME location so sandboxes can safely hide /home and /root.
  // Fall back to ~/.picoagent if /srv isn't writable.
  let base = baseDir ?? join('/srv', 'picoagent', 'workspaces');
  try {
    mkdirSync(base, { recursive: true });
    const probe = join(base, '.write-test');
    writeFileSync(probe, 'ok', 'utf8');
    try { unlinkSync(probe); } catch {}
  } catch {
    base = baseDir ?? join(homedir(), '.picoagent', 'workspaces');
  }

  return base;
}

function shouldCopyPath(src: string): boolean {
  const name = basename(src);
  return !new Set(['.git', 'node_modules', 'dist', '.picoagent']).has(name);
}

function initWorkspaceRepo(repoDir: string): void {
  gitOk(['init', '-q'], { cwd: repoDir });
  gitOk(['config', 'user.email', 'picoagent@local'], { cwd: repoDir });
  gitOk(['config', 'user.name', 'picoagent'], { cwd: repoDir });
}

function seedIsolatedRepo(repoDir: string, controlDir: string): void {
  cpSync(controlDir, repoDir, {
    recursive: true,
    filter: shouldCopyPath,
  });

  if (!existsSync(join(repoDir, '.gitignore'))) {
    writeFileSync(join(repoDir, '.gitignore'), 'node_modules/\ndist/\n.picoagent/\n', 'utf8');
  }

  initWorkspaceRepo(repoDir);
  gitOk(['add', '.'], { cwd: repoDir });
  gitOk(['commit', '-q', '-m', 'chore: initialize execution snapshot'], { cwd: repoDir });
}

/**
 * Create a fresh execution workspace under ~/.picoagent/workspaces/<timestamp>/.
 *
 * If the control directory is inside a git repository, picoagent executes directly
 * against that repository and stores only task worktrees in the run directory.
 * Otherwise it creates an isolated git snapshot under runDir/repo so worker
 * worktrees still have a real repository to branch from.
 */
export function createRunWorkspace(opts: { baseDir?: string; controlDir: string }): RunWorkspace {
  const base = resolveBaseDir(opts.baseDir);
  const controlDir = resolve(opts.controlDir);
  const runDir = join(base, isoId());
  const tasksDir = join(runDir, 'tasks');

  mkdirSync(runDir, { recursive: true });
  mkdirSync(tasksDir, { recursive: true });

  const gitRoot = findGitRoot(controlDir);
  if (gitRoot) {
    return {
      runDir,
      repoDir: gitRoot,
      tasksDir,
      mode: 'attached-git',
    };
  }

  const repoDir = join(runDir, 'repo');
  mkdirSync(repoDir, { recursive: true });
  seedIsolatedRepo(repoDir, controlDir);

  return {
    runDir,
    repoDir,
    tasksDir,
    mode: 'isolated-copy',
  };
}
