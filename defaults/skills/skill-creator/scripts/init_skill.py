#!/usr/bin/env python3
"""
Skill Initializer for picoagent

Usage:
    init_skill.py <skill-name> --path <path> [--resources scripts,references,assets] [--agent]

Examples:
    init_skill.py my-skill --path workspace/skills
    init_skill.py my-skill --path workspace/skills --resources scripts,references
    init_skill.py researcher --path workspace/agents --agent
"""

import argparse
import re
import sys
from pathlib import Path

MAX_NAME_LENGTH = 64
ALLOWED_RESOURCES = {"scripts", "references", "assets"}

SKILL_TEMPLATE = """---
name: {name}
description: "[TODO: What this skill does and when to use it. Be specific about triggers.]"
---

# {title}

## Overview

[TODO: 1-2 sentences explaining what this skill enables]

## Instructions

[TODO: Add instructions, workflows, examples. Keep under 500 lines.
For longer content, move to references/ and link from here.]
"""

AGENT_TEMPLATE = """---
name: {name}
description: "[TODO: What this agent does and when to dispatch it]"
model: "[TODO: model name, e.g. gpt-4o]"
provider: "[TODO: anthropic, openai, or gemini]"
tags: []
---

[TODO: Additional system prompt context for this agent type.
This becomes part of the worker's system prompt when dispatched.]
"""


def normalize_name(raw):
    normalized = raw.strip().lower()
    normalized = re.sub(r"[^a-z0-9]+", "-", normalized)
    normalized = normalized.strip("-")
    normalized = re.sub(r"-{2,}", "-", normalized)
    return normalized


def title_case(name):
    return " ".join(word.capitalize() for word in name.split("-"))


def main():
    parser = argparse.ArgumentParser(description="Initialize a new skill or agent profile.")
    parser.add_argument("name", help="Skill/agent name (normalized to hyphen-case)")
    parser.add_argument("--path", required=True, help="Output directory")
    parser.add_argument("--resources", default="", help="Comma-separated: scripts,references,assets")
    parser.add_argument("--agent", action="store_true", help="Create agent profile instead of skill")
    args = parser.parse_args()

    name = normalize_name(args.name)
    if not name:
        print("[ERROR] Name must include at least one letter or digit.")
        sys.exit(1)
    if len(name) > MAX_NAME_LENGTH:
        print(f"[ERROR] Name '{name}' too long ({len(name)} chars, max {MAX_NAME_LENGTH}).")
        sys.exit(1)
    if name != args.name:
        print(f"Note: Normalized name from '{args.name}' to '{name}'.")

    title = title_case(name)
    path = Path(args.path).resolve()

    if args.agent:
        # Agent profile: single .md file
        agent_file = path / f"{name}.md"
        if agent_file.exists():
            print(f"[ERROR] Agent file already exists: {agent_file}")
            sys.exit(1)
        path.mkdir(parents=True, exist_ok=True)
        agent_file.write_text(AGENT_TEMPLATE.format(name=name, title=title))
        print(f"[OK] Created agent profile: {agent_file}")
        print("\nNext: Edit the file to fill in TODO items.")
        sys.exit(0)

    # Skill: directory with SKILL.md
    skill_dir = path / name
    if skill_dir.exists():
        print(f"[ERROR] Skill directory already exists: {skill_dir}")
        sys.exit(1)

    skill_dir.mkdir(parents=True, exist_ok=False)
    print(f"[OK] Created skill directory: {skill_dir}")

    # Create SKILL.md
    (skill_dir / "SKILL.md").write_text(SKILL_TEMPLATE.format(name=name, title=title))
    print("[OK] Created SKILL.md")

    # Create resource directories
    if args.resources:
        resources = [r.strip() for r in args.resources.split(",") if r.strip()]
        invalid = [r for r in resources if r not in ALLOWED_RESOURCES]
        if invalid:
            print(f"[ERROR] Unknown resource type(s): {', '.join(invalid)}")
            print(f"   Allowed: {', '.join(sorted(ALLOWED_RESOURCES))}")
            sys.exit(1)
        for resource in resources:
            (skill_dir / resource).mkdir(exist_ok=True)
            print(f"[OK] Created {resource}/")

    print(f"\n[OK] Skill '{name}' initialized at {skill_dir}")
    print("\nNext steps:")
    print("1. Edit SKILL.md — fill in description and instructions")
    if args.resources:
        print("2. Add resources to scripts/, references/, assets/ as needed")
    print("3. Run validate_skill.py to check structure")


if __name__ == "__main__":
    main()
