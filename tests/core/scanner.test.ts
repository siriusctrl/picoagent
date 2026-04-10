import { describe, test, expect } from 'bun:test';
import { join } from "path";
import { parseFrontmatter, scan, load, scanMarkdownDocuments } from "../../src/prompting/frontmatter.ts";

const fixturesDir = join(process.cwd(), "tests/fixtures");

function requireValue<T>(value: T | undefined, message: string): T {
  if (value === undefined) {
    throw new Error(message);
  }

  return value;
}

describe("Scanner", () => {
  describe("parseFrontmatter", () => {
    test("should parse simple frontmatter", () => {
      const content = `---
name: test
value: 123
enabled: true
---
Body content`;
      const { frontmatter, body } = parseFrontmatter(content);
      expect(frontmatter).toEqual({ name: "test", value: 123, enabled: true });
      expect(body).toBe("Body content");
    });

    test("should parse inline arrays", () => {
      const content = `---
tags: [a, b, c]
---
Body`;
      const { frontmatter } = parseFrontmatter(content);
      expect(frontmatter).toEqual({ tags: ["a", "b", "c"] });
    });

    test("should handle empty frontmatter", () => {
      const content = `Body only`;
      const { frontmatter, body } = parseFrontmatter(content);
      expect(frontmatter).toEqual({});
      expect(body).toBe("Body only");
    });

    test("should handle strings with quotes", () => {
        const content = `---
title: "Hello World"
desc: 'Single quotes'
---
Body`;
        const { frontmatter } = parseFrontmatter(content);
        expect(frontmatter).toEqual({ title: "Hello World", desc: "Single quotes" });
    });
  });

  describe("scan", () => {
    test("should scan markdown documents without filesystem access", () => {
      const results = scanMarkdownDocuments([
        {
          path: "memory/doc.md",
          content: `---
name: doc
category: test
---
Body`,
        },
        {
          path: "memory/skip.md",
          content: `---
name: other
category: misc
---
Body`,
        },
      ], { category: "test" });

      expect(results).toHaveLength(1);
      expect(results[0]!.path).toBe("memory/doc.md");
      expect(results[0]!.frontmatter.name).toBe("doc");
    });

    test("should find markdown files in directory", async () => {
      const results = await scan(fixturesDir);
      // We expect doc1.md, doc2.md, subdir/doc3.md
      // no-front.md also is found but has empty frontmatter
      expect(results.length).toBeGreaterThanOrEqual(4);
      const doc1 = requireValue(results.find(r => r.path.endsWith("doc1.md")), "expected doc1 fixture");
      expect(doc1.frontmatter.name).toBe("doc1");
    });

    test("should filter by pattern", async () => {
      const results = await scan(fixturesDir, { category: "test" });
      expect(results).toHaveLength(1);
      expect(results[0]!.path).toMatch(/doc2\.md$/);
    });

    test("should support wildcard pattern", async () => {
        const results = await scan(fixturesDir, { category: "*" });
        // matches "test" and "nested"
        const paths = results.map(r => r.path);
        expect(paths.filter(p => p.endsWith("doc2.md"))).toHaveLength(1);
        expect(paths.filter(p => p.endsWith("subdir/doc3.md"))).toHaveLength(1); // absolute path
    });

    test("should treat regex metacharacters literally in wildcard patterns", () => {
      const results = scanMarkdownDocuments([
        {
          path: "memory/doc.md",
          content: `---
name: a+b*c?.md
---
Body`,
        },
        {
          path: "memory/skip.md",
          content: `---
name: abZZcXmd
---
Body`,
        },
      ], { name: "a+b*c?.md" });

      expect(results).toHaveLength(1);
      expect(results[0]!.path).toBe("memory/doc.md");
    });
  });

  describe("load", () => {
    test("should load full document", async () => {
      const docPath = join(fixturesDir, "doc1.md");
      const doc = await load(docPath);
      expect(doc.frontmatter.name).toBe("doc1");
      expect(doc.body).toContain("This is the body of doc1.");
    });
  });
});
