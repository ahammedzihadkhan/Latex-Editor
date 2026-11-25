use crate::backend::{get_gemini_auth, safe_path, walk, GeminiAuth};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct DriveSyncReport {
    pub folder_id: String,
    pub uploaded: usize,
    pub downloaded: usize,
    pub share_url: String,
}

fn require_bearer() -> Result<String, String> {
    match get_gemini_auth()? {
        GeminiAuth::Bearer(tok) => Ok(tok),
        GeminiAuth::ApiKey(_) => Err("Please click 'Sign in with Google' in Settings & Key to use Google Drive Sync & Sharing.".into()),
    }
}

fn drive_get(tok: &str, url: &str) -> Result<Value, String> {
    ureq::get(url)
        .set("Authorization", &format!("Bearer {tok}"))
        .call()
        .map_err(|e| format!("Google Drive error: {e}"))?
        .into_json::<Value>()
        .map_err(|e| format!("Invalid Drive JSON: {e}"))
}

fn find_or_create_folder(tok: &str, name: &str, parent_id: Option<&str>) -> Result<String, String> {
    let escaped = name.replace('\'', "\\'");
    let q = match parent_id {
        Some(pid) => format!("name='{escaped}' and mimeType='application/vnd.google-apps.folder' and '{pid}' in parents and trashed=false"),
        None => format!("name='{escaped}' and mimeType='application/vnd.google-apps.folder' and trashed=false"),
    };
    let enc_q: String = q.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let list_url = format!("https://www.googleapis.com/drive/v3/files?q={enc_q}&fields=files(id,name)");
    let res = drive_get(tok, &list_url)?;
    if let Some(id) = res["files"].as_array().and_then(|a| a.first()).and_then(|f| f["id"].as_str()) {
        return Ok(id.to_string());
    }

    let mut meta = json!({
        "name": name,
        "mimeType": "application/vnd.google-apps.folder"
    });
    if let Some(pid) = parent_id {
        meta["parents"] = json!([pid]);
    }
    let created = ureq::post("https://www.googleapis.com/drive/v3/files?fields=id")
        .set("Authorization", &format!("Bearer {tok}"))
        .send_json(meta)
        .map_err(|e| format!("Failed to create Drive folder: {e}"))?
        .into_json::<Value>()
        .map_err(|e| format!("Invalid folder response: {e}"))?;
    created["id"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| "Drive did not return folder ID".to_string())
}

fn upload_multipart(tok: &str, existing_id: Option<&str>, name: &str, parent_id: &str, bytes: &[u8]) -> Result<String, String> {
    let boundary = "inktex_drive_boundary_987654";
    let meta = if existing_id.is_some() {
        json!({ "name": name })
    } else {
        json!({ "name": name, "parents": [parent_id] })
    };

    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{}\r\n", meta).as_bytes());
    body.extend_from_slice(format!("--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let (method, url) = match existing_id {
        Some(id) => ("PATCH", format!("https://www.googleapis.com/upload/drive/v3/files/{id}?uploadType=multipart&fields=id,modifiedTime")),
        None => ("POST", "https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart&fields=id,modifiedTime".to_string()),
    };

    let resp = ureq::request(method, &url)
        .set("Authorization", &format!("Bearer {tok}"))
        .set("Content-Type", &format!("multipart/related; boundary={boundary}"))
        .send_bytes(&body)
        .map_err(|e| format!("Drive upload failed for {name}: {e}"))?
        .into_json::<Value>()
        .map_err(|e| format!("Invalid upload response: {e}"))?;

    resp["id"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| "Missing file id after upload".to_string())
}

fn download_file(tok: &str, file_id: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let resp = ureq::get(&format!("https://www.googleapis.com/drive/v3/files/{file_id}?alt=media"))
        .set("Authorization", &format!("Bearer {tok}"))
        .call()
        .map_err(|e| format!("Drive download failed: {e}"))?;
    let mut out = Vec::new();
    resp.into_reader().read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

pub fn load_drive_meta(root: &Path) -> Value {
    fs::read_to_string(root.join(".gdrive.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

pub fn link_drive_folder(root: &Path, folder_id_or_url: &str) -> Result<String, String> {
    let raw = folder_id_or_url.trim();
    let id = if let Some(pos) = raw.find("/folders/") {
        raw[pos + 9..].split(['?', '/', '#']).next().unwrap_or("").to_string()
    } else {
        raw.to_string()
    };
    if id.is_empty() {
        return Err("Invalid Google Drive folder ID or URL".into());
    }
    let mut meta = load_drive_meta(root);
    meta["folder_id"] = json!(id);
    fs::write(root.join(".gdrive.json"), meta.to_string()).map_err(|e| e.to_string())?;
    Ok(id)
}

/// Two-way sync of a project folder with Google Drive.
pub fn sync_project_with_drive(root: &Path, project_name: &str) -> Result<DriveSyncReport, String> {
    let tok = require_bearer()?;
    let mut state = load_drive_meta(root);

    let folder_id = if let Some(id) = state["folder_id"].as_str().filter(|s| !s.is_empty()) {
        id.to_string()
    } else {
        let root_folder = find_or_create_folder(&tok, "inkTeX Projects", None)?;
        find_or_create_folder(&tok, project_name, Some(&root_folder))?
    };

    // List remote files in this folder
    let list_url = format!(
        "https://www.googleapis.com/drive/v3/files?q='{}'+in+parents+and+trashed=false&fields=files(id,name,modifiedTime,mimeType)&pageSize=1000",
        folder_id
    );
    let remote_res = drive_get(&tok, &list_url)?;
    let mut remote_map: HashMap<String, (String, String)> = HashMap::new(); // rel_name -> (id, modifiedTime)
    for f in remote_res["files"].as_array().into_iter().flatten() {
        if f["mimeType"].as_str() == Some("application/vnd.google-apps.folder") {
            continue;
        }
        if let (Some(id), Some(name)) = (f["id"].as_str(), f["name"].as_str()) {
            let mtime = f["modifiedTime"].as_str().unwrap_or("").to_string();
            remote_map.insert(name.to_string(), (id.to_string(), mtime));
        }
    }

    let mut local_files = Vec::new();
    walk(root, root, &mut local_files);

    let mut synced_files = state["files"].as_object().cloned().unwrap_or_default();
    let mut uploaded = 0usize;
    let mut downloaded = 0usize;

    // 1. Upload new or locally modified files
    for rel in &local_files {
        let Ok(abs) = safe_path(root, rel) else { continue };
        let local_mtime = fs::metadata(&abs)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let last_local_sync = synced_files.get(rel).and_then(|v| v["local_mtime"].as_u64()).unwrap_or(0);
        let remote_entry = remote_map.get(rel);

        let should_upload = match remote_entry {
            None => true,
            Some((_, remote_mtime)) => {
                let last_remote_mtime = synced_files.get(rel).and_then(|v| v["remote_mtime"].as_str()).unwrap_or("");
                local_mtime > last_local_sync && remote_mtime == last_remote_mtime
            }
        };

        if should_upload {
            if let Ok(bytes) = fs::read(&abs) {
                let existing_id = remote_entry.map(|(id, _)| id.as_str());
                let fid = upload_multipart(&tok, existing_id, rel, &folder_id, &bytes)?;
                uploaded += 1;
                synced_files.insert(
                    rel.clone(),
                    json!({
                        "id": fid,
                        "local_mtime": local_mtime,
                        "remote_mtime": remote_entry.map(|(_, m)| m.as_str()).unwrap_or(""),
                    }),
                );
            }
        }
    }

    // 2. Download new or remotely updated files
    for (rel, (fid, remote_mtime)) in &remote_map {
        let last_remote_mtime = synced_files.get(rel).and_then(|v| v["remote_mtime"].as_str()).unwrap_or("");
        let Ok(abs) = safe_path(root, rel) else { continue };
        let local_exists = abs.is_file();

        if !local_exists || remote_mtime != last_remote_mtime {
            // Avoid overwriting if we just uploaded this file in step 1
            let local_mtime = fs::metadata(&abs)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let last_local_sync = synced_files.get(rel).and_then(|v| v["local_mtime"].as_u64()).unwrap_or(0);
            if !local_exists || local_mtime <= last_local_sync {
                if let Ok(bytes) = download_file(&tok, fid) {
                    if let Some(p) = abs.parent() {
                        let _ = fs::create_dir_all(p);
                    }
                    if fs::write(&abs, &bytes).is_ok() {
                        downloaded += 1;
                        let new_local_mtime = fs::metadata(&abs)
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        synced_files.insert(
                            rel.clone(),
                            json!({
                                "id": fid,
                                "local_mtime": new_local_mtime,
                                "remote_mtime": remote_mtime,
                            }),
                        );
                    }
                }
            }
        }
    }

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    state["folder_id"] = json!(folder_id);
    state["last_sync"] = json!(now);
    state["files"] = Value::Object(synced_files);
    let _ = fs::write(root.join(".gdrive.json"), state.to_string());

    Ok(DriveSyncReport {
        share_url: format!("https://drive.google.com/drive/folders/{folder_id}"),
        folder_id,
        uploaded,
        downloaded,
    })
}

/// Share the project's Google Drive folder with a collaborator email or anyone with link.
pub fn share_drive_folder(folder_id: &str, email: &str, role: &str) -> Result<String, String> {
    let tok = require_bearer()?;
    let email = email.trim();
    let perm = if email.is_empty() {
        json!({ "type": "anyone", "role": role })
    } else {
        json!({ "type": "user", "role": role, "emailAddress": email })
    };
    ureq::post(&format!("https://www.googleapis.com/drive/v3/files/{folder_id}/permissions"))
        .set("Authorization", &format!("Bearer {tok}"))
        .send_json(perm)
        .map_err(|e| format!("Failed to share folder: {e}"))?;
    Ok(format!("https://drive.google.com/drive/folders/{folder_id}"))
}
