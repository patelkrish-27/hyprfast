//! Task state tracking — todo-like steps for any multi-step action.
//! Persists to `$XDG_RUNTIME_DIR/hyprfast-tasks.json` (fallback `/tmp`)
//! AI flow: breakdown goal → task_init {goal, steps} → per step task_update {index, status} → auto-clear when all completed.

use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TaskStep {
    pub id: usize,
    pub description: String,
    pub status: String, // pending | in_progress | completed | failed | skipped
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TaskList {
    pub goal: String,
    pub steps: Vec<TaskStep>,
    pub created_at: String,
    pub updated_at: String,
}

fn task_path() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", nix::unistd::getuid()));
    PathBuf::from(runtime).join("hyprfast-tasks.json")
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn read_task() -> Option<TaskList> {
    let p = task_path();
    if !p.exists() { return None; }
    let data = std::fs::read_to_string(&p).unwrap_or_default();
    if data.trim().is_empty() { return None; }
    // support legacy empty array
    if data.trim() == "[]" { return None; }
    serde_json::from_str::<TaskList>(&data).ok()
}

fn write_task(list: &TaskList) -> Result<()> {
    let p = task_path();
    if let Some(parent) = p.parent() { let _ = std::fs::create_dir_all(parent); }
    let data = serde_json::to_string_pretty(list)?;
    std::fs::write(&p, data).with_context(|| format!("write {}", p.display()))?;
    Ok(())
}

fn write_empty() -> Result<()> {
    let p = task_path();
    // remove file to indicate empty
    if p.exists() { let _ = std::fs::remove_file(&p); }
    Ok(())
}

pub fn init(goal: &str, steps: Vec<String>) -> Result<serde_json::Value> {
    let _g = LOCK.lock().unwrap();
    if steps.is_empty() {
        anyhow::bail!("init needs at least 1 step");
    }
    let now = now_iso();
    let task_steps: Vec<TaskStep> = steps.into_iter().enumerate().map(|(i, desc)| TaskStep {
        id: i + 1,
        description: desc,
        status: "pending".to_string(),
        created_at: now.clone(),
        updated_at: now.clone(),
    }).collect();
    let list = TaskList {
        goal: goal.to_string(),
        steps: task_steps,
        created_at: now.clone(),
        updated_at: now,
    };
    write_task(&list)?;
    Ok(to_status_value(&list))
}

pub fn add(description: &str) -> Result<serde_json::Value> {
    let _g = LOCK.lock().unwrap();
    let mut list = read_task().ok_or_else(|| anyhow::anyhow!("no active task list — run task_init first"))?;
    let now = now_iso();
    let next_id = list.steps.iter().map(|s| s.id).max().unwrap_or(0) + 1;
    list.steps.push(TaskStep {
        id: next_id,
        description: description.to_string(),
        status: "pending".to_string(),
        created_at: now.clone(),
        updated_at: now.clone(),
    });
    list.updated_at = now;
    write_task(&list)?;
    Ok(to_status_value(&list))
}

pub fn update(index: Option<usize>, id: Option<usize>, status: &str) -> Result<serde_json::Value> {
    let _g = LOCK.lock().unwrap();
    let mut list = read_task().ok_or_else(|| anyhow::anyhow!("no active task list — run task_init first"))?;
    let valid = ["pending","in_progress","completed","failed","skipped","cancelled"];
    if !valid.contains(&status) {
        anyhow::bail!("invalid status '{}' — must be one of {:?}", status, valid);
    }
    // resolve target index (0-based) from either id (1-based) or index (0-based or 1-based?)
    // CLI passes index as 0-based? We'll support both: if index provided, treat as 0-based if < len, else 1-based-1
    // If id provided, find by id
    let target_pos: Option<usize> = if let Some(id_val) = id {
        list.steps.iter().position(|s| s.id == id_val)
    } else if let Some(idx) = index {
        if idx < list.steps.len() {
            Some(idx)
        } else if idx > 0 && idx - 1 < list.steps.len() {
            Some(idx - 1)
        } else {
            None
        }
    } else {
        None
    };
    let pos = target_pos.ok_or_else(|| anyhow::anyhow!("step not found — provide index (0-based) or id (1-based)"))?;
    list.steps[pos].status = status.to_string();
    list.steps[pos].updated_at = now_iso();
    list.updated_at = now_iso();

    // check auto-clear: all completed/skipped
    let all_done = list.steps.iter().all(|s| s.status == "completed" || s.status == "skipped");
    if all_done && !list.steps.is_empty() {
        let goal_clone = list.goal.clone();
        let count = list.steps.len();
        write_empty()?;
        return Ok(serde_json::json!({
            "goal": goal_clone,
            "steps_completed": count,
            "auto_cleared": true,
            "message": "all steps completed — task list auto-cleared",
            "status": "empty"
        }));
    }

    write_task(&list)?;
    Ok(to_status_value(&list))
}

pub fn status() -> serde_json::Value {
    let _g = LOCK.lock().unwrap();
    if let Some(list) = read_task() {
        to_status_value(&list)
    } else {
        serde_json::json!({
            "goal": null,
            "steps": [],
            "total": 0,
            "pending": 0,
            "in_progress": 0,
            "completed": 0,
            "failed": 0,
            "status": "empty",
            "message": "no active task — run task_init to start"
        })
    }
}

pub fn clear() -> Result<serde_json::Value> {
    let _g = LOCK.lock().unwrap();
    let before = read_task();
    let count = before.as_ref().map(|l| l.steps.len()).unwrap_or(0);
    let goal = before.map(|l| l.goal).unwrap_or_default();
    write_empty()?;
    Ok(serde_json::json!({
        "cleared": true,
        "goal": if goal.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(goal) },
        "steps_cleared": count,
        "status": "empty"
    }))
}

pub fn next_pending() -> Result<serde_json::Value> {
    let _g = LOCK.lock().unwrap();
    let list = read_task().ok_or_else(|| anyhow::anyhow!("no active task list"))?;
    if let Some(step) = list.steps.iter().find(|s| s.status == "pending") {
        Ok(serde_json::json!({
            "next": step,
            "progress": progress(&list)
        }))
    } else if let Some(step) = list.steps.iter().find(|s| s.status == "in_progress") {
        Ok(serde_json::json!({
            "next": step,
            "note": "no pending, but in_progress exists",
            "progress": progress(&list)
        }))
    } else {
        Ok(serde_json::json!({
            "next": null,
            "message": "no pending steps — all done or failed",
            "progress": progress(&list)
        }))
    }
}

fn progress(list: &TaskList) -> serde_json::Value {
    let total = list.steps.len();
    let pending = list.steps.iter().filter(|s| s.status == "pending").count();
    let in_progress = list.steps.iter().filter(|s| s.status == "in_progress").count();
    let completed = list.steps.iter().filter(|s| s.status == "completed").count();
    let failed = list.steps.iter().filter(|s| s.status == "failed").count();
    let skipped = list.steps.iter().filter(|s| s.status == "skipped").count();
    serde_json::json!({
        "total": total,
        "pending": pending,
        "in_progress": in_progress,
        "completed": completed,
        "failed": failed,
        "skipped": skipped,
        "percent": if total==0 { 0 } else { (completed*100)/total }
    })
}

fn to_status_value(list: &TaskList) -> serde_json::Value {
    let prog = progress(list);
    serde_json::json!({
        "goal": list.goal,
        "steps": list.steps,
        "progress": prog,
        "created_at": list.created_at,
        "updated_at": list.updated_at,
        "status": if prog["pending"].as_u64().unwrap_or(0)==0 && prog["in_progress"].as_u64().unwrap_or(0)==0 && prog["completed"].as_u64().unwrap_or(0)==prog["total"].as_u64().unwrap_or(0) { "completed" } else { "active" },
        "task_file": task_path().to_string_lossy().to_string()
    })
}

// helpers for CLI parsing steps string
pub fn parse_steps_arg(s: &str) -> Vec<String> {
    let trimmed = s.trim();
    // try JSON array
    if trimmed.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<Vec<String>>(trimmed) {
            return v;
        }
        if let Ok(v) = serde_json::from_str::<Vec<serde_json::Value>>(trimmed) {
            return v.into_iter().map(|x| x.as_str().unwrap_or("").to_string()).filter(|x| !x.is_empty()).collect();
        }
    }
    // comma or newline separated
    if trimmed.contains(',') {
        return trimmed.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
    }
    if trimmed.contains('\n') {
        return trimmed.split('\n').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
    }
    // single step? treat as one
    if !trimmed.is_empty() { vec![trimmed.to_string()] } else { vec![] }
}
