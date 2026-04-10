import { basenamePath, joinPath } from '../../src/fs/path.ts';

function tmpRoot(): string {
  return process.env.TMPDIR || Bun.env.TMPDIR || '/tmp';
}

export function basename(filePath: string): string {
  return basenamePath(filePath);
}

export async function ensureDir(dirPath: string): Promise<void> {
  const markerPath = joinPath(dirPath, '.keep');
  await Bun.write(markerPath, '');
  await Bun.file(markerPath).delete();
}

export async function makeTempDir(prefix: string): Promise<string> {
  const dirPath = joinPath(tmpRoot(), `${prefix}${crypto.randomUUID()}`);
  await ensureDir(dirPath);
  return dirPath;
}

export async function removeDir(dirPath: string): Promise<void> {
  const process = Bun.spawn({
    cmd: ['rm', '-rf', dirPath],
    stdin: 'ignore',
    stdout: 'ignore',
    stderr: 'pipe',
  });
  const exitCode = await process.exited;
  if (exitCode !== 0) {
    const error = await process.stderr.text();
    throw new Error(error || `Failed to remove directory: ${dirPath}`);
  }
}

export async function writeTextFile(filePath: string, content: string): Promise<void> {
  await Bun.write(filePath, content);
}

export async function readTextFile(filePath: string): Promise<string> {
  return Bun.file(filePath).text();
}
