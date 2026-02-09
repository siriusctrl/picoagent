import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert";
import { join } from "path";
import { mkdtempSync, rmSync, existsSync, readFileSync, readdirSync } from "fs";
import { tmpdir } from "os";
import { createTask, readTask, updateTaskStatus, listTasks } from "../../src/lib/task.js";

describe("Task Management", () => {
  let tasksRoot: string;

  beforeEach(() => {
    tasksRoot = mkdtempSync(join(tmpdir(), "picoagent-test-"));
  });

  afterEach(() => {
    rmSync(tasksRoot, { recursive: true, force: true });
  });

  it("should create a task directory with correct structure", () => {
    const config = {
      name: "Test Task",
      description: "A test task",
      instructions: "Do this",
      tags: ["test"]
    };

    const task = createTask(tasksRoot, config);
    
    assert.strictEqual(task.id, "t_001");
    assert.strictEqual(task.name, "Test Task");
    assert.strictEqual(task.status, "pending");
    assert.deepStrictEqual(task.tags, ["test"]);
    
    assert.ok(existsSync(join(task.dir, "task.md")));
    assert.ok(existsSync(join(task.dir, "progress.md")));
    
    const content = readFileSync(join(task.dir, "task.md"), "utf-8");
    assert.ok(content.includes('name: "Test Task"'));
    assert.ok(content.includes('status: pending'));
    assert.ok(content.includes('tags: [test]'));
  });

  it("should generate sequential IDs", () => {
    const t1 = createTask(tasksRoot, { name: "T1", description: "D1", instructions: "I1" });
    const t2 = createTask(tasksRoot, { name: "T2", description: "D2", instructions: "I2" });
    
    assert.strictEqual(t1.id, "t_001");
    assert.strictEqual(t2.id, "t_002");
  });

  it("should read task info correctly", () => {
    const created = createTask(tasksRoot, { name: "Read Me", description: "Desc", instructions: "Instr" });
    const read = readTask(created.dir);
    
    assert.strictEqual(read.id, created.id);
    assert.strictEqual(read.name, created.name);
    assert.strictEqual(read.description, created.description);
    assert.strictEqual(read.status, created.status);
    assert.strictEqual(read.created, created.created);
  });

  it("should update task status", () => {
    const task = createTask(tasksRoot, { name: "Update Me", description: "Desc", instructions: "Instr" });
    
    updateTaskStatus(task.dir, "running");
    let updated = readTask(task.dir);
    assert.strictEqual(updated.status, "running");
    assert.ok(updated.started);
    
    updateTaskStatus(task.dir, "completed");
    updated = readTask(task.dir);
    assert.strictEqual(updated.status, "completed");
    assert.ok(updated.completed);
  });


  it("should list tasks", () => {
    createTask(tasksRoot, { name: "T1", description: "D1", instructions: "I1" });
    createTask(tasksRoot, { name: "T2", description: "D2", instructions: "I2" });
    
    const tasks = listTasks(tasksRoot);
    assert.strictEqual(tasks.length, 2);
    assert.strictEqual(tasks[0].id, "t_001");
    assert.strictEqual(tasks[1].id, "t_002");
  });
});
