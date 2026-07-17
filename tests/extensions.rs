use std::{fs, path::Path, sync::Arc};

use picoagent::{
    hooks::{CommandHook, HookEvent, HookPipeline},
    memory::MemoryPaths,
    skills::{LoadSkillTool, SkillRegistry},
    tools::{Tool, ToolContext, WriteTool},
};
use serde_json::json;
use tempfile::TempDir;

#[tokio::test]
async fn skills_load_on_demand_and_memory_uses_ordinary_paths() {
    let workspace = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let skill_dir = workspace.path().join("skills/research");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: research\ndescription: Research carefully.\n---\n# Instructions\nUse primary sources.",
    )
    .unwrap();

    let registry = Arc::new(SkillRegistry::discover(workspace.path(), Some(home.path())).unwrap());
    assert_eq!(registry.prompt_index(), "- research: Research carefully.");
    let loaded = LoadSkillTool::new(registry)
        .execute(
            ToolContext {
                run_id: "run".into(),
                call_id: "call".into(),
                workspace: workspace.path().to_path_buf(),
            },
            json!({ "name": "research" }),
        )
        .await
        .unwrap();
    let loaded = String::from_utf8(loaded.content).unwrap();
    assert!(loaded.contains("Use primary sources."));
    assert!(loaded.contains(skill_dir.join("SKILL.md").to_string_lossy().as_ref()));
    assert!(loaded.contains(skill_dir.to_string_lossy().as_ref()));
    assert!(!loaded.contains("name: research"));
    assert!(!loaded.contains("Research carefully."));

    let memory = MemoryPaths::new(home.path(), workspace.path());
    assert_eq!(memory.user, home.path().join("memory/user"));
    assert_eq!(
        memory.project,
        workspace.path().join(".pico/memory/project")
    );
    let prompt = memory.runtime_reminder_section();
    assert!(prompt.contains(memory.user.to_string_lossy().as_ref()));
    assert!(!prompt.contains("memory_update"));
    assert_eq!(prompt.lines().count(), 2);

    WriteTool::default()
        .execute(
            ToolContext {
                run_id: "run".into(),
                call_id: "write-memory".into(),
                workspace: workspace.path().to_path_buf(),
            },
            json!({
                "path": memory.user.join("profile.md"),
                "content": "# Preferences\n\n- Keep it simple.\n"
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(memory.user.join("profile.md")).unwrap(),
        "# Preferences\n\n- Keep it simple.\n"
    );
}

#[tokio::test]
async fn command_hook_uses_json_stdin_and_output() {
    let mut pipeline = HookPipeline::new();
    pipeline.register(CommandHook::new(
        "rewrite",
        HookEvent::RunStart,
        "sh",
        vec![
            "-c".into(),
            "cat >/dev/null; printf '%s' '{\"payload\":{\"ready\":true}}'".into(),
        ],
    ));
    let result = pipeline
        .run(HookEvent::RunStart, json!({}), Path::new("."))
        .await
        .unwrap();
    assert_eq!(result.payload, json!({ "ready": true }));
}
