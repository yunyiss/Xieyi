use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::AppHandle;

use crate::paths;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub title: String,
    pub model: String,
    pub updated_at: u64,
    pub messages: Vec<SessionMessage>,
}

fn sessions_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = paths::app_data(app)?.join("sessions");
    paths::ensure(&dir)?;
    Ok(dir)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 列出所有会话，按更新时间倒序。
#[tauri::command]
pub fn list_sessions(app: AppHandle) -> Vec<Session> {
    let Ok(dir) = sessions_dir(&app) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut out: Vec<Session> = entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|raw| serde_json::from_str::<Session>(&raw).ok())
        .collect();

    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

#[tauri::command]
pub fn save_session(app: AppHandle, mut session: Session) -> Result<(), String> {
    if session.id.trim().is_empty() {
        return Err("会话 id 为空".into());
    }
    session.updated_at = now_secs();

    let path = sessions_dir(&app)?.join(format!("{}.json", session.id));
    let json = serde_json::to_string_pretty(&session).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("保存会话失败: {e}"))
}

#[tauri::command]
pub fn delete_session(app: AppHandle, id: String) -> Result<(), String> {
    let path = sessions_dir(&app)?.join(format!("{id}.json"));
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| format!("删除会话失败: {e}"))?;
    }
    Ok(())
}
