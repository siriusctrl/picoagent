export interface DocMeta {
  path: string;
  frontmatter: Record<string, unknown>;
}

export interface DocFull extends DocMeta {
  body: string;
}

export interface MarkdownDocument {
  path: string;
  content: string;
}

function wildcardPatternToRegExp(pattern: string): RegExp {
  return new RegExp(`^${pattern.split("*").map(RegExp.escape).join(".*")}$`);
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

function matchesPattern(frontmatter: Record<string, unknown>, pattern: Record<string, string>): boolean {
  for (const [key, pat] of Object.entries(pattern)) {
    const val = frontmatter[key];
    if (val === undefined) {
      return false;
    }

    const valStr = String(val);
    if (pat.includes('*')) {
      const regex = wildcardPatternToRegExp(pat);
      if (!regex.test(valStr)) {
        return false;
      }
      continue;
    }

    if (valStr !== pat) {
      return false;
    }
  }

  return true;
}

export function scanMarkdownDocuments(documents: MarkdownDocument[], pattern?: Record<string, string>): DocMeta[] {
  const results: DocMeta[] = [];

  for (const document of documents) {
    try {
      const { frontmatter } = parseFrontmatter(document.content);
      if (!pattern || matchesPattern(frontmatter, pattern)) {
        results.push({
          path: document.path,
          frontmatter,
        });
      }
    } catch {
      continue;
    }
  }

  return results.sort((a, b) => a.path.localeCompare(b.path));
}

/**
 * Scan a directory for markdown files and return their frontmatter.
 * Optionally filter by a pattern matching frontmatter fields.
 */
export async function scan(dir: string, pattern?: Record<string, string>): Promise<DocMeta[]> {
  try {
    const documents: MarkdownDocument[] = [];

    for await (const filePath of new Bun.Glob('**/*.md').scan({
      cwd: dir,
      absolute: true,
      dot: true,
      onlyFiles: true,
      followSymlinks: false,
    })) {
      try {
        documents.push({
          path: filePath,
          content: await Bun.file(filePath).text(),
        });
      } catch {
        continue;
      }
    }

    return scanMarkdownDocuments(documents, pattern);
  } catch {
    return [];
  }
}

/**
 * Load a file fully: frontmatter + body.
 */
export async function load(filePath: string): Promise<DocFull> {
  const content = await Bun.file(filePath).text();
  const { frontmatter, body } = parseFrontmatter(content);
  return {
    path: filePath,
    frontmatter,
    body
  };
}

export function loadMarkdownDocument(document: MarkdownDocument): DocFull {
  const { frontmatter, body } = parseFrontmatter(document.content);
  return {
    path: document.path,
    frontmatter,
    body,
  };
}
