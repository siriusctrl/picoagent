import { mkdirSync, writeFileSync, readFileSync, existsSync, readdirSync, statSync } from "fs";
import { join } from "path";
import { parseFrontmatter } from "./scanner.js";

export interface TaskConfig {
  name: string;
  description: string;
  instructions: string;
  model?: string;
  tags?: string[];
}

export interface TaskInfo {
  id: string;
  dir: string;
  name: string;
  description: string;
  status: "pending" | "running" | "completed" | "failed" | "aborted";
  created: string;
  started?: string;
  completed?: string;
  model?: string;
  tags: string[];
}

/**
 * Create a task directory under tasksRoot with task.md, empty progress.md, etc.
 * Returns the TaskInfo with id and directory path.
 */
export function createTask(tasksRoot: string, config: TaskConfig): TaskInfo {
  // Ensure tasks root exists
  if (!existsSync(tasksRoot)) {
    mkdirSync(tasksRoot, { recursive: true });
  }

  // Generate short ID like t_001, t_002, etc (based on existing dirs)
  const dirs = readdirSync(tasksRoot).filter(d => /^t_\d+$/.test(d));
  let nextId = 1;
  if (dirs.length > 0) {
    const ids = dirs.map(d => parseInt(d.slice(2), 10));
    nextId = Math.max(...ids) + 1;
  }
  const id = `t_${String(nextId).padStart(3, "0")}`;
  const taskDir = join(tasksRoot, id);

  mkdirSync(taskDir);

  const now = new Date().toISOString();
  
  const tags = config.tags || [];
  const tagsStr = tags.length > 0 ? `[${tags.join(", ")}]` : "[]";
  const modelStr = config.model ? `\nmodel: ${config.model}` : "";

  // Write task.md with frontmatter (status: pending)
  const taskContent = `---
id: ${id}
name: "${config.name.replace(/"/g, '\\"')}"
description: "${config.description.replace(/"/g, '\\"')}"
status: pending
created: ${now}
started: null
completed: null${modelStr}
tags: ${tagsStr}
---

${config.instructions}
`;

  writeFileSync(join(taskDir, "task.md"), taskContent);

  // Write empty progress.md
  writeFileSync(join(taskDir, "progress.md"), "");

  return {
    id,
    dir: taskDir,
    name: config.name,
    description: config.description,
    status: "pending",
    created: now,
    tags
  };
}

/**
 * Read task info from a task directory (parsing task.md frontmatter)
 */
export function readTask(taskDir: string): TaskInfo {
  const taskPath = join(taskDir, "task.md");
  const content = readFileSync(taskPath, "utf-8");
  const { frontmatter } = parseFrontmatter(content);

  return {
    id: String(frontmatter.id),
    dir: taskDir,
    name: String(frontmatter.name),
    description: String(frontmatter.description),
    status: frontmatter.status as TaskInfo["status"],
    created: String(frontmatter.created),
    started: frontmatter.started ? String(frontmatter.started) : undefined,
    completed: frontmatter.completed ? String(frontmatter.completed) : undefined,
    model: frontmatter.model ? String(frontmatter.model) : undefined,
    tags: Array.isArray(frontmatter.tags) ? frontmatter.tags as string[] : []
  };
}

/**
 * Update task status in task.md frontmatter
 */
export function updateTaskStatus(taskDir: string, status: TaskInfo["status"]): void {
  const taskPath = join(taskDir, "task.md");
  const content = readFileSync(taskPath, "utf-8");
  const { frontmatter, body } = parseFrontmatter(content);

  const now = new Date().toISOString();

  frontmatter.status = status;
  if (status === "running" && !frontmatter.started) {
    frontmatter.started = now;
  }
  if ((status === "completed" || status === "failed" || status === "aborted") && !frontmatter.completed) {
    frontmatter.completed = now;
  }

  // Reconstruct file
  let newContent = "---\n";
  for (const [key, value] of Object.entries(frontmatter)) {
    if (value === null || value === undefined) {
      newContent += `${key}: null\n`;
    } else if (Array.isArray(value)) {
      newContent += `${key}: [${value.join(", ")}]\n`;
    } else {
       // Quote strings if they contain spaces or special chars, but for simplicity we can check type
       // The scanner handles unquoted strings, but let's be safe for writing
       // actually scanner.ts handles quotes stripping, so we should add them if string
       if (typeof value === 'string') {
           newContent += `${key}: "${value.replace(/"/g, '\\"')}"\n`;
       } else {
           newContent += `${key}: ${value}\n`;
       }
    }
  }
  newContent += "---\n\n" + body;

  writeFileSync(taskPath, newContent);
}

/**
 * List all tasks by scanning the tasks root directory
 */
export function listTasks(tasksRoot: string): TaskInfo[] {
  if (!existsSync(tasksRoot)) return [];
  
  const dirs = readdirSync(tasksRoot);
  const tasks: TaskInfo[] = [];

  for (const dir of dirs) {
    const taskDir = join(tasksRoot, dir);
    // Only check directories that look like tasks (t_XXX)
    if (!/^t_\d+$/.test(dir)) continue;
    
    try {
      if (statSync(taskDir).isDirectory() && existsSync(join(taskDir, "task.md"))) {
        tasks.push(readTask(taskDir));
      }
    } catch (e) {
      // Ignore invalid or unreadable tasks
    }
  }

  // Sort by creation time (descending or ascending? Prompt says "sorted by creation time". Let's do ascending/oldest first as per t_001, t_002)
  // Actually IDs are chronological, so sorting by ID is sufficient
  return tasks.sort((a, b) => a.id.localeCompare(b.id));
}
