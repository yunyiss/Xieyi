use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::AppHandle;

use crate::paths;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    /// 思考型模型的思维链。只在对话页展示用，永远不参与请求上下文。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
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
    // 只允许用 id 拼文件名，防住 ../ 之类的越界
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return Err("会话 id 不合法".into());
    }

    let path = sessions_dir(&app)?.join(format!("{id}.json"));
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| format!("删除会话失败: {e}"))?;
    }
    Ok(())
}

/// 删除全部会话，返回实际删掉的数量。
///
/// 上下文只存在本地，用户要能一键清干净，不该出现"删不掉的历史"。
#[tauri::command]
pub fn delete_all_sessions(app: AppHandle) -> Result<usize, String> {
    let dir = sessions_dir(&app)?;
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("读取会话目录失败: {e}"))?;

    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_session_without_reasoning_still_loads() {
        // 加字段之前写下的会话文件必须还能读：这是兼容性的底线
        let raw = r#"{
            "id": "abc",
            "title": "旧对话",
            "model": "m.gguf",
            "updatedAt": 1,
            "messages": [{ "role": "user", "content": "hi" }]
        }"#;
        let session: Session = serde_json::from_str(raw).expect("旧格式应能解析");
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].reasoning, None);
    }

    #[test]
    fn reasoning_round_trips_and_is_omitted_when_empty() {
        let session = Session {
            id: "abc".into(),
            title: "带思考".into(),
            model: "m.gguf".into(),
            updated_at: 1,
            messages: vec![
                SessionMessage {
                    role: "user".into(),
                    content: "问".into(),
                    reasoning: None,
                },
                SessionMessage {
                    role: "assistant".into(),
                    content: "答".into(),
                    reasoning: Some("想了一下".into()),
                },
            ],
        };

        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("\"reasoning\":\"想了一下\""));

        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(back.messages[1].reasoning.as_deref(), Some("想了一下"));
        assert_eq!(back.messages[0].reasoning, None);
        // 没有思维链的消息不该写出这个字段，避免会话文件臃肿
        assert!(!json.contains("\"reasoning\":null"));
    }
}
