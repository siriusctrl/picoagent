---
name: coding
description: "Coding conventions and best practices for TypeScript projects"
tags: [coding, typescript]
---

## TypeScript Conventions
- Use `strict: true` in tsconfig
- Prefer `unknown` over `any`
- Use `const` by default, `let` only when reassignment needed
- Explicit return types on exported functions
- Use template literals over string concatenation

## Error Handling
- Use typed errors (extend Error class)
- Always handle promise rejections
- Prefer early returns over deeply nested conditions
- Log errors with context (what was being attempted)

## File Organization
- One exported concept per file
- Group imports: external → internal → relative
- Keep files under 300 lines; split if larger

## Naming
- `camelCase` for variables and functions
- `PascalCase` for types, interfaces, classes
- `UPPER_SNAKE` for constants
- Descriptive names over abbreviations
