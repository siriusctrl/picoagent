import { readFileSync, readdirSync, statSync } from "fs";
import { join, extname } from "path";

export interface DocMeta {
  path: string;
  frontmatter: Record<string, unknown>;
}

export interface DocFull extends DocMeta {
  body: string;
}

/**
 * Parse YAML frontmatter from markdown content.
 * Frontmatter is between --- delimiters at the start of the file.
 */
export function parseFrontmatter(content: string): { frontmatter: Record<string, unknown>; body: string } {
  const frontmatter: Record<string, unknown> = {};
  let body = content;

  // Check if content starts with frontmatter delimiter
  if (content.startsWith("---\n") || content.startsWith("---\r\n")) {
    const endDelimIndex = content.indexOf("\n---", 4);
    if (endDelimIndex !== -1) {
      const rawFrontmatter = content.slice(4, endDelimIndex);
      body = content.slice(endDelimIndex + 5).trim(); // +5 for \n--- and newline after

      const lines = rawFrontmatter.split(/\r?\n/);
      for (const line of lines) {
        const trimmed = line.trim();
        if (!trimmed || trimmed.startsWith("#")) continue;

        const colonIndex = trimmed.indexOf(":");
        if (colonIndex !== -1) {
          const key = trimmed.slice(0, colonIndex).trim();
          let valueStr = trimmed.slice(colonIndex + 1).trim();
          let value: unknown = valueStr;

          // Parse values
          if (valueStr === "true") {
            value = true;
          } else if (valueStr === "false") {
            value = false;
          } else if (!isNaN(Number(valueStr)) && valueStr !== "") {
             // Handle numbers (simple integer/float check)
             // Note: !isNaN("") is true in JS, so we check empty string
             value = Number(valueStr);
          } else if (valueStr.startsWith("[") && valueStr.endsWith("]")) {
            // Inline array: [a, b, c]
            const inner = valueStr.slice(1, -1);
            if (inner.trim() === "") {
              value = [];
            } else {
              value = inner.split(",").map(v => {
                const vTrim = v.trim();
                // Strip quotes if present
                if ((vTrim.startsWith('"') && vTrim.endsWith('"')) || (vTrim.startsWith("'") && vTrim.endsWith("'"))) {
                   return vTrim.slice(1, -1);
                }
                return vTrim;
              });
            }
          } else {
             // String value - strip quotes if present
             if ((valueStr.startsWith('"') && valueStr.endsWith('"')) || (valueStr.startsWith("'") && valueStr.endsWith("'"))) {
                value = valueStr.slice(1, -1);
             }
          }
          
          frontmatter[key] = value;
        }
      }
    }
  }

  return { frontmatter, body };
}

/**
 * Scan a directory for markdown files and return their frontmatter.
 * Optionally filter by a pattern matching frontmatter fields.
 */
export function scan(dir: string, pattern?: Record<string, string>): DocMeta[] {
  const results: DocMeta[] = [];

  try {
    const files = readdirSync(dir);

    for (const file of files) {
      const fullPath = join(dir, file);
      const stat = statSync(fullPath);

      if (stat.isDirectory()) {
        results.push(...scan(fullPath, pattern));
      } else if (stat.isFile() && extname(file) === ".md") {
        try {
          const content = readFileSync(fullPath, "utf-8");
          const { frontmatter } = parseFrontmatter(content);
          
          // Apply filter if pattern is provided
          let match = true;
          if (pattern) {
            for (const [key, pat] of Object.entries(pattern)) {
              const val = frontmatter[key];
              if (val === undefined) {
                match = false;
                break;
              }
              
              const valStr = String(val);
              // Simple wildcard matching
              if (pat.includes("*")) {
                const regex = new RegExp("^" + pat.replace(/\*/g, ".*") + "$");
                if (!regex.test(valStr)) {
                  match = false;
                  break;
                }
              } else if (valStr !== pat) {
                match = false;
                break;
              }
            }
          }

          if (match) {
            results.push({
              path: fullPath,
              frontmatter
            });
          }
        } catch (e) {
          // Ignore read errors
        }
      }
    }
  } catch (e) {
    // Ignore directory errors (e.g. not found)
  }

  // Sort by path for determinism
  return results.sort((a, b) => a.path.localeCompare(b.path));
}

/**
 * Load a file fully: frontmatter + body.
 */
export function load(filePath: string): DocFull {
  const content = readFileSync(filePath, "utf-8");
  const { frontmatter, body } = parseFrontmatter(content);
  return {
    path: filePath,
    frontmatter,
    body
  };
}
