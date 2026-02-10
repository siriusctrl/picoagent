#!/usr/bin/env python3
"""
Skill Validator for picoagent

Usage:
    validate_skill.py <path/to/skill-name>
"""

import sys
import re
from pathlib import Path

ALLOWED_RESOURCES = {"scripts", "references", "assets"}
MAX_NAME_LENGTH = 64
MAX_SKILL_LINES = 500


def parse_frontmatter(content):
    """Simple frontmatter parser matching picoagent's format."""
    if not content.startswith("---"):
        return None
    end = content.find("---", 3)
    if end == -1:
        return None
    fm = {}
    for line in content[3:end].strip().split("\n"):
        if ":" in line:
            key, _, value = line.partition(":")
            fm[key.strip()] = value.strip().strip('"')
    return fm


def validate_skill(skill_path):
    errors = []
    warnings = []
    skill_dir = Path(skill_path).resolve()

    if not skill_dir.is_dir():
        print(f"[ERROR] Not a directory: {skill_dir}")
        sys.exit(1)

    # Check SKILL.md exists
    skill_md = skill_dir / "SKILL.md"
    if not skill_md.exists():
        errors.append("Missing SKILL.md")
        print_results(skill_dir.name, errors, warnings)
        sys.exit(1)

    content = skill_md.read_text()

    # Check frontmatter
    fm = parse_frontmatter(content)
    if fm is None:
        errors.append("Missing or invalid frontmatter (must start with ---)")
    else:
        if not fm.get("name"):
            errors.append("Frontmatter missing 'name' field")
        elif len(fm["name"]) > MAX_NAME_LENGTH:
            errors.append(f"Name too long ({len(fm['name'])} chars, max {MAX_NAME_LENGTH})")
        elif not re.match(r"^[a-z0-9-]+$", fm["name"]):
            errors.append(f"Name must be lowercase letters, digits, hyphens: '{fm['name']}'")

        if not fm.get("description"):
            errors.append("Frontmatter missing 'description' field")
        elif "TODO" in fm["description"]:
            warnings.append("Description contains TODO placeholder")
        elif len(fm["description"]) < 20:
            warnings.append("Description seems short — include when to trigger this skill")

    # Check body
    lines = content.split("\n")
    body_lines = len(lines)
    if body_lines > MAX_SKILL_LINES:
        warnings.append(f"SKILL.md is {body_lines} lines (recommended max {MAX_SKILL_LINES})")

    if "TODO" in content:
        warnings.append("SKILL.md body contains TODO placeholders")

    # Check for unexpected files/dirs
    for item in skill_dir.iterdir():
        if item.name == "SKILL.md":
            continue
        if item.is_dir() and item.name not in ALLOWED_RESOURCES:
            warnings.append(f"Unexpected directory: {item.name}/ (allowed: {', '.join(sorted(ALLOWED_RESOURCES))})")
        if item.is_file() and item.name != "SKILL.md":
            warnings.append(f"Unexpected file in skill root: {item.name}")

    # Check directory name matches frontmatter name
    if fm and fm.get("name") and skill_dir.name != fm["name"]:
        warnings.append(f"Directory name '{skill_dir.name}' doesn't match frontmatter name '{fm['name']}'")

    print_results(skill_dir.name, errors, warnings)
    sys.exit(1 if errors else 0)


def print_results(name, errors, warnings):
    if errors:
        print(f"\n❌ Skill '{name}' has {len(errors)} error(s):")
        for e in errors:
            print(f"  [ERROR] {e}")
    if warnings:
        print(f"\n⚠️  {len(warnings)} warning(s):")
        for w in warnings:
            print(f"  [WARN] {w}")
    if not errors and not warnings:
        print(f"\n✅ Skill '{name}' looks good!")
    elif not errors:
        print(f"\n✅ Skill '{name}' is valid (with warnings)")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("Usage: validate_skill.py <path/to/skill-name>")
        sys.exit(1)
    validate_skill(sys.argv[1])
