import { existsSync, mkdirSync, writeFileSync, readFileSync } from 'fs';
import { homedir } from 'os';
import { join } from 'path';
import { gitOk } from './git.js';

export interface RunWorkspace {
  runDir: string;
  repoDir: string;
  tasksDir: string;
}

function isoId(d = new Date()): string {
  // safe for filenames
  return d.toISOString().replace(/[:.]/g, '-');
}

/**
 * Create a fresh workspace under ~/.picoagent/workspaces/<timestamp>/
 * containing:
 *   repo/  - a git repo (main/orchestrator checkout)
 *   tasks/ - worktree directories for subagents
 */
export function createRunWorkspace(opts?: { baseDir?: string; copyConfigFrom?: string }): RunWorkspace {
  const base = opts?.baseDir ?? join(homedir(), '.picoagent', 'workspaces');
  const runDir = join(base, isoId());
  const repoDir = join(runDir, 'repo');
  const tasksDir = join(runDir, 'tasks');

  mkdirSync(repoDir, { recursive: true });
  mkdirSync(tasksDir, { recursive: true });

  // init git repo with a seed commit so worktrees can be created
  if (!existsSync(join(repoDir, '.git'))) {
    gitOk(['init', '-q'], { cwd: repoDir });
    gitOk(['config', 'user.email', 'picoagent@local'], { cwd: repoDir });
    gitOk(['config', 'user.name', 'picoagent'], { cwd: repoDir });

    writeFileSync(join(repoDir, '.gitignore'), 'node_modules\n.venv\n__pycache__\n*.pyc\n.DS_Store\n', 'utf8');
    writeFileSync(join(repoDir, 'README.md'), `# picoagent run workspace\n\nCreated: ${new Date().toISOString()}\n`, 'utf8');

    gitOk(['add', '.'], { cwd: repoDir });
    gitOk(['commit', '-q', '-m', 'chore: init run workspace'], { cwd: repoDir });
  }

  // Optional: copy config.md so tools that expect it in workspace can find it.
  if (opts?.copyConfigFrom) {
    const src = join(opts.copyConfigFrom, 'config.md');
    const dst = join(repoDir, 'config.md');
    if (existsSync(src) && !existsSync(dst)) {
      writeFileSync(dst, readFileSync(src, 'utf8'));
      gitOk(['add', 'config.md'], { cwd: repoDir });
      gitOk(['commit', '-q', '-m', 'chore: add config.md'], { cwd: repoDir });
    }
  }

  return { runDir, repoDir, tasksDir };
}
