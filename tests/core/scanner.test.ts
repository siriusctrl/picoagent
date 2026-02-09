import { describe, it } from "node:test";
import assert from "node:assert";
import { join } from "path";
import { parseFrontmatter, scan, load } from "../../src/core/scanner.js";

const fixturesDir = join(process.cwd(), "tests/fixtures");

describe("Scanner", () => {
  describe("parseFrontmatter", () => {
    it("should parse simple frontmatter", () => {
      const content = `---
name: test
value: 123
enabled: true
---
Body content`;
      const { frontmatter, body } = parseFrontmatter(content);
      assert.deepStrictEqual(frontmatter, { name: "test", value: 123, enabled: true });
      assert.strictEqual(body, "Body content");
    });

    it("should parse inline arrays", () => {
      const content = `---
tags: [a, b, c]
---
Body`;
      const { frontmatter } = parseFrontmatter(content);
      assert.deepStrictEqual(frontmatter, { tags: ["a", "b", "c"] });
    });

    it("should handle empty frontmatter", () => {
      const content = `Body only`;
      const { frontmatter, body } = parseFrontmatter(content);
      assert.deepStrictEqual(frontmatter, {});
      assert.strictEqual(body, "Body only");
    });

    it("should handle strings with quotes", () => {
        const content = `---
title: "Hello World"
desc: 'Single quotes'
---
Body`;
        const { frontmatter } = parseFrontmatter(content);
        assert.deepStrictEqual(frontmatter, { title: "Hello World", desc: "Single quotes" });
    });
  });

  describe("scan", () => {
    it("should find markdown files in directory", () => {
      const results = scan(fixturesDir);
      // We expect doc1.md, doc2.md, subdir/doc3.md
      // no-front.md also is found but has empty frontmatter
      assert.ok(results.length >= 4);
      const doc1 = results.find(r => r.path.endsWith("doc1.md"));
      assert.ok(doc1);
      assert.strictEqual(doc1.frontmatter.name, "doc1");
    });

    it("should filter by pattern", () => {
      const results = scan(fixturesDir, { category: "test" });
      assert.strictEqual(results.length, 1);
      assert.ok(results[0].path.endsWith("doc2.md"));
    });

    it("should support wildcard pattern", () => {
        const results = scan(fixturesDir, { category: "*" });
        // matches "test" and "nested"
        const paths = results.map(r => r.path);
        const hasDoc2 = paths.some(p => p.endsWith("doc2.md"));
        const hasDoc3 = paths.some(p => p.endsWith("subdir/doc3.md")); // absolute path
        assert.ok(hasDoc2);
        assert.ok(hasDoc3);
    });
  });

  describe("load", () => {
    it("should load full document", () => {
      const docPath = join(fixturesDir, "doc1.md");
      const doc = load(docPath);
      assert.strictEqual(doc.frontmatter.name, "doc1");
      assert.ok(doc.body.includes("This is the body of doc1."));
    });
  });
});
