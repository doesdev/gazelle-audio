//! User themes for the web UI: JSON files in the themes directory (`--themes-dir`, by default
//! `themes` in the config directory). Each file is listed with its parsed theme, or with the
//! reason it cannot be used; the web UI validates the theme itself. The directory holds a few
//! small files, so reading it inline in the handler is fine.

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::AppState;

pub async fn list_themes(State(state): State<AppState>) -> Json<Value> {
    let Some(dir) = state.themes_dir.as_deref() else {
        return Json(json!([]));
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Json(json!([]));
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    let themes = paths
        .iter()
        .map(|path| {
            let file = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let parsed = std::fs::read_to_string(path)
                .map_err(|e| e.to_string())
                .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()));
            match parsed {
                Ok(theme) if theme.is_object() => json!({ "file": file, "theme": theme }),
                Ok(_) => json!({ "file": file, "error": "a theme must be a JSON object" }),
                Err(error) => json!({ "file": file, "error": error }),
            }
        })
        .collect();
    Json(Value::Array(themes))
}
