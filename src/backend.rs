use crate::nn;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs};

pub const ICON: &[u8] = include_bytes!("../app.ico");
pub const LOGO: &[u8] = include_bytes!("../logo.png");
pub const TEMPLATE: &str = "\\documentclass{article}\n\\title{Untitled}\n\\author{}\n\\begin{document}\n\\maketitle\n\n\
\\section{Introduction}\nStart writing here.\n\n\\end{document}\n";
/// From https://github.com/sb2nov/resume (MIT, templates/LICENSE-resume).
pub const RESUME: &str = include_str!("../templates/resume.tex");
pub const HISTORY_KEEP: usize = 10;
const SYSTEM: &str = "You are an expert LaTeX editor helping fix an academic paper. You receive the full \
source of one .tex file, the latest compile log errors, and an instruction. Apply the instruction (fix \
compile errors, grammar, clarity, etc.) while preserving the author's meaning, citations, labels and \
structure. Reply with ONLY the complete revised file content - no explanations, no markdown code fences.";

/// Frozen for the whole chat: the open file, selection and errors travel in each user message instead,
/// so the history stays append-only (Claude rejects replayed thinking whose earlier context changed).
const CHAT_SYSTEM: &str = "You are the AI assistant inside inkTeX, a professional local LaTeX editor. \
You have FULL ACCESS and permission to inspect, debug, and edit ALL files and chapters in the user's project. \
All chapters, frontmatter, abstract, abbreviations, tables, appendices, and references are provided in <file name=\"...\"> blocks. \
You MUST read and analyze these files directly to answer questions and debug errors. \
NEVER tell the user that a file is missing or that you lack permission. NEVER ask the user to open or paste file contents, because all project files are already provided to you. \
When the user asks you to fix, debug, or edit one or more project files, output the complete updated content for each modified file inside an <edit file=\"relative/path.tex\">...</edit> block (one block per file). \
For small inline snippets, you may also use a ```latex fenced block. \
Keep answers direct, academic, and practical.";

/// "Help me write" (like Gemini in Google Docs): writes at the cursor or rewrites a selection, given nearby text.
pub const WRITE_SYSTEM: &str = "You are the writing assistant inside a LaTeX editor, like Help me write in Google Docs. \
You get a request plus the text around the cursor: <before>, then either <cursor/> (write new text there) or <selection> \
(rewrite it), then <after>. Reply with ONLY the LaTeX to put at the cursor or in place of the selection: no explanation, \
no code fences, and do not repeat the surrounding text. Match the document's language, tone and LaTeX conventions; keep \
existing \\cite, \\ref and \\label commands; never invent citations, data or results (write \\cite{TODO} where a source is needed).";

pub const CLAUDE_MODELS: &[&str] = &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-2"];
pub const GEMINI_MODELS: &[&str] = &["gemini-3.8-flash", "gemini-flash-lite-latest", "gemini-flash-latest", "gemini-pro-latest", "gemini-3.5-flash"];

/// Models offered in the picker: the env choice first, then the known ones.
pub fn model_list(var: &str, known: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = vec![];
    for m in env::var(var).into_iter().chain(known.iter().map(|m| m.to_string())) {
        if !v.contains(&m) {
            v.push(m)
        }
    }
    v
}

/// The model the browser picked, else the env default. Plain ids only: Gemini's goes into the URL path.
fn model_of(b: &Value, var: &str, default: &str) -> Result<String, String> {
    match b["model"].as_str().filter(|m| !m.is_empty()) {
        Some(m) if m.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) => Ok(m.into()),
        Some(_) => Err("bad model name".into()),
        None => Ok(env::var(var).unwrap_or_else(|_| default.into())),
    }
}

/// Haiku has no server-side refusal fallback; send it the plain request (thinking stays at its adaptive default).
fn for_model(mut body: Value, model: &str) -> Value {
    if model.contains("haiku") {
        let o = body.as_object_mut().unwrap();
        o.remove("fallbacks");
        o.remove("thinking");
    }
    body
}

pub static WORKSPACE: OnceLock<PathBuf> = OnceLock::new();

/// The folder that holds all projects; each project is one folder directly inside it.
pub fn workspace() -> &'static Path {
    WORKSPACE.get().unwrap()
}

pub fn project_dir(p: &str) -> Result<PathBuf, String> {
    let mut c = Path::new(p).components();
    let dir = match (c.next(), c.next()) {
        (Some(Component::Normal(_)), None) if !p.starts_with('.') => workspace().join(p),
        _ => return Err("bad project name".into()),
    };
    dir.is_dir().then_some(dir).ok_or_else(|| "project not found".into())
}

/// Resolve a user-supplied relative name inside `root`, refusing absolute paths and `..`.
pub fn safe_path(root: &Path, name: &str) -> Result<PathBuf, String> {
    let rel = Path::new(name);
    if name.is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err("path outside project".into());
    }
    Ok(root.join(rel))
}

/// Newest modification time (unix secs) of the .tex files at the top of `dir`; None = not a LaTeX project.
pub fn newest_tex(dir: &Path) -> Option<u64> {
    fs::read_dir(dir).ok()?.flatten()
        .filter(|f| f.file_name().to_string_lossy().ends_with(".tex") && f.path().is_file())
        .filter_map(|f| f.metadata().ok()?.modified().ok())
        .map(|t| t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()))
        .max()
}

pub fn list_projects() -> Vec<Value> {
    let mut v: Vec<(u64, String)> = fs::read_dir(workspace()).into_iter().flatten().flatten()
        .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| Some((newest_tex(&e.path())?, e.file_name().to_string_lossy().to_string())))
        .collect();
    v.sort_by(|a, b| b.cmp(a));
    v.into_iter().map(|(t, n)| {
        let hist: Vec<Value> = history(&workspace().join(&n)).into_iter()
            .map(|(f, secs)| json!({"file": format!(".history/{f}"), "name": f.split_once('-').map_or("", |x| x.1), "time": secs}))
            .collect();
        let pdf = newest_pdf(&workspace().join(&n));
        json!({"name": n, "modified": t, "history": hist,
               "pdf": pdf.as_ref().map(|p| p.0.to_string_lossy().to_string()), "pdf_modified": pdf.map_or(0, |p| p.1)})
    }).collect()
}

/// Most recently written PDF directly inside `dir`, with its mtime in unix seconds.
fn newest_pdf(dir: &Path) -> Option<(PathBuf, u64)> {
    fs::read_dir(dir).ok()?.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")))
        .filter_map(|e| {
            let t = e.metadata().ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
            Some((e.path(), t))
        })
        .max_by_key(|p| p.1)
}

/// Past builds in `<project>/.history`, named `<unix secs>-<file>.pdf`; newest first.
pub fn history(root: &Path) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = fs::read_dir(root.join(".history")).into_iter().flatten().flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            let secs = n.split_once('-')?.0.parse().ok()?;
            Some((n, secs))
        })
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
    v
}

/// Keep a copy of a successful build; only the newest HISTORY_KEEP copies stay, to bound disk use.
fn save_history(root: &Path, pdf: &Path) -> std::io::Result<()> {
    let dir = root.join(".history");
    fs::create_dir_all(&dir)?;
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let name = pdf.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    fs::copy(root.join(pdf), dir.join(format!("{secs}-{name}")))?;
    for (old, _) in history(root).into_iter().skip(HISTORY_KEEP) {
        let _ = fs::remove_file(dir.join(old));
    }
    Ok(())
}

/// A Windows-safe folder name from what the user typed or the zip's file name.
fn clean_name(s: &str) -> String {
    let n: String = s.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).take(80).collect();
    n.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_string()
}

/// `base`, or `base (2)`, `base (3)`â€¦ if a project with that name already exists.
fn free_name(base: &str) -> Result<String, String> {
    let base = clean_name(base);
    if base.is_empty() {
        return Err("Enter a project name.".into());
    }
    (1..1000).map(|i| if i == 1 { base.clone() } else { format!("{base} ({i})") })
        .find(|n| !workspace().join(n).exists()).ok_or_else(|| "name taken".into())
}

pub fn new_project(name: &str, template: &str) -> Result<String, String> {
    let name = free_name(name)?;
    let dir = workspace().join(&name);
    let p = profile();
    let field = |k: &str| p[k].as_str().unwrap_or("").to_string();
    let text = if template == "resume" {
        let (n, e) = (field("name"), field("email"));
        let (n, e) = (if n.is_empty() { "Your Name".into() } else { n }, if e.is_empty() { "you@example.com".into() } else { e });
        // hyperref takes the URL nearly verbatim; only % and # need escaping there
        RESUME.replace("{{NAME}}", &tex_escape(&n)).replace("{{EMAIL}}", &tex_escape(&e))
            .replace("{{EMAIL_URL}}", &e.replace('%', "\\%").replace('#', "\\#"))
    } else {
        let mut author = tex_escape(&field("name"));
        if !field("affiliation").is_empty() {
            author += &format!(" \\\\ {}", tex_escape(&field("affiliation")));
        }
        TEMPLATE.replace("\\author{}", &format!("\\author{{{author}}}"))
    };
    fs::create_dir(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join("main.tex"), text).map_err(|e| e.to_string())?;
    Ok(name)
}

/// Write `diagrams/<name>.tex` from the layer list (plus the shapes it needs) and build it to a PDF.
pub fn make_diagram(root: &Path, name: &str, spec: &str) -> Result<Value, String> {
    let name: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    let name = if name.is_empty() { "network".into() } else { name };
    let tex = nn::build(spec, "layers/")?;
    let dir = root.join("diagrams");
    fs::create_dir_all(dir.join("layers")).map_err(|e| e.to_string())?;
    for (f, body) in nn::LAYERS {
        if !dir.join("layers").join(f).exists() {
            fs::write(dir.join("layers").join(f), body).map_err(|e| e.to_string())?;
        }
    }
    let file = dir.join(format!("{name}.tex"));
    if file.exists() {
        fs::copy(&file, dir.join(format!("{name}.tex.bak"))).map_err(|e| e.to_string())?; // same one-step backup as Save
    }
    fs::write(&file, tex).map_err(|e| e.to_string())?;
    compile_tex(root, &format!("diagrams/{name}.tex"))
}

fn tex_escape(s: &str) -> String {
    s.chars().map(|c| match c {
        '\\' => r"\textbackslash{}".into(),
        '~' => r"\textasciitilde{}".into(),
        '^' => r"\textasciicircum{}".into(),
        '&' | '%' | '$' | '#' | '_' | '{' | '}' => format!("\\{c}"),
        c => c.to_string(),
    }).collect()
}

/// The user's details, kept in `<projects>/.profile.json`.
pub fn profile() -> Value {
    fs::read_to_string(workspace().join(".profile.json")).ok()
        .and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({}))
}

pub fn get_gemini_key() -> Result<String, String> {
    if let Ok(k) = env::var("GEMINI_API_KEY") {
        let k = k.trim();
        if !k.is_empty() {
            return Ok(k.to_string());
        }
    }
    if let Some(k) = profile().get("gemini_api_key").and_then(|v| v.as_str()) {
        let k = k.trim();
        if !k.is_empty() {
            return Ok(k.to_string());
        }
    }
    let key_file = app_dir().join("gemini_key.txt");
    if let Ok(s) = fs::read_to_string(&key_file) {
        let s = s.trim();
        if !s.is_empty() {
            return Ok(s.to_string());
        }
    }
    Err("Gemini API key is not set. Please configure your Gemini API key in Settings / Profile.".into())
}

pub fn save_gemini_key(key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("API key cannot be empty".into());
    }
    let _ = fs::create_dir_all(app_dir());
    let _ = fs::write(app_dir().join("gemini_key.txt"), key);
    let p = profile();
    let mut map = p.as_object().cloned().unwrap_or_default();
    map.insert("gemini_api_key".into(), json!(key));
    let p = Value::Object(map);
    let _ = fs::write(workspace().join(".profile.json"), p.to_string());
    env::set_var("GEMINI_API_KEY", key);
    Ok(())
}

/// Claude API key: `ANTHROPIC_API_KEY` wins, else the key saved from the sign-in page.
pub fn get_claude_key() -> Result<String, String> {
    env::var("ANTHROPIC_API_KEY").ok()
        .or_else(|| fs::read_to_string(app_dir().join("claude_key.txt")).ok())
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .ok_or_else(|| "Not signed in to Claude. Use AI > Sign in, then Continue with Claude.".into())
}

pub fn save_claude_key(key: &str) -> Result<(), String> {
    let key = key.trim();
    if !key.starts_with("sk-ant-") || key.len() < 20 {
        return Err("That doesn't look like a Claude API key (it should start with sk-ant-).".into());
    }
    fs::create_dir_all(app_dir()).map_err(|e| e.to_string())?;
    fs::write(app_dir().join("claude_key.txt"), key).map_err(|e| e.to_string())
}

pub fn sign_out_claude() {
    let _ = fs::remove_file(app_dir().join("claude_key.txt"));
}

/// Google OAuth Desktop client baked in at build time (set INKTEX_GOOGLE_CLIENT_ID / _SECRET when running
/// `cargo build`), so users get one-click "Continue with Google" without creating their own client.
pub fn builtin_google_client() -> Option<(String, String)> {
    let id = option_env!("INKTEX_GOOGLE_CLIENT_ID").filter(|s| !s.trim().is_empty())?;
    Some((id.to_string(), option_env!("INKTEX_GOOGLE_CLIENT_SECRET").unwrap_or("").to_string()))
}

/// 256-bit hex token for OAuth state / PKCE verifier. std's RandomState is seeded from the OS RNG.
fn random_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    (0..4).map(|i| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() ^ i);
        format!("{:016x}", h.finish())
    }).collect()
}

pub fn save_profile(b: &Value) -> Result<Value, String> {
    let f = |k: &str| b[k].as_str().unwrap_or("").trim().chars().take(200).collect::<String>();
    let p = profile();
    let mut map = p.as_object().cloned().unwrap_or_default();
    map.insert("name".into(), json!(f("name")));
    map.insert("email".into(), json!(f("email")));
    map.insert("affiliation".into(), json!(f("affiliation")));
    if let Some(t) = b["theme"].as_str().filter(|s| !s.is_empty()) {
        map.insert("theme".into(), json!(t));
    }
    if let Some(gk) = b["gemini_api_key"].as_str() {
        let gk = gk.trim();
        if !gk.is_empty() {
            let _ = save_gemini_key(gk);
            map.insert("gemini_api_key".into(), json!(gk));
        }
    }
    let p = Value::Object(map);
    fs::write(workspace().join(".profile.json"), p.to_string()).map_err(|e| e.to_string())?;
    Ok(p)
}

pub enum GeminiAuth {
    ApiKey(String),
    Bearer(String),
}

fn oauth_path() -> PathBuf {
    app_dir().join("google_oauth.json")
}

pub fn google_oauth_info() -> Option<(String, String)> {
    let txt = fs::read_to_string(oauth_path()).ok()?;
    let v: Value = serde_json::from_str(&txt).ok()?;
    let cid = v["client_id"].as_str().unwrap_or("").to_string();
    let email = v["email"].as_str().unwrap_or("Google Account").to_string();
    if v["refresh_token"].as_str().is_some() || v["access_token"].as_str().is_some() {
        Some((email, cid))
    } else {
        None
    }
}

pub fn sign_out_google() {
    let _ = fs::remove_file(oauth_path());
}

pub fn get_gemini_auth() -> Result<GeminiAuth, String> {
    if let Ok(txt) = fs::read_to_string(oauth_path()) {
        if let Ok(mut v) = serde_json::from_str::<Value>(&txt) {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
            let exp = v["expires_at"].as_u64().unwrap_or(0);
            if let Some(tok) = v["access_token"].as_str().filter(|t| !t.is_empty() && now + 60 < exp) {
                return Ok(GeminiAuth::Bearer(tok.to_string()));
            }
            if let (Some(rt), Some(cid)) = (v["refresh_token"].as_str(), v["client_id"].as_str()) {
                let csec = v["client_secret"].as_str().unwrap_or("");
                let mut params = vec![
                    ("client_id", cid),
                    ("refresh_token", rt),
                    ("grant_type", "refresh_token"),
                ];
                if !csec.is_empty() {
                    params.push(("client_secret", csec));
                }
                if let Ok(resp) = ureq::post("https://oauth2.googleapis.com/token").send_form(&params) {
                    if let Ok(new_v) = resp.into_json::<Value>() {
                        if let Some(new_tok) = new_v["access_token"].as_str() {
                            let expires_in = new_v["expires_in"].as_u64().unwrap_or(3600);
                            v["access_token"] = json!(new_tok);
                            v["expires_at"] = json!(now + expires_in);
                            let _ = fs::write(oauth_path(), v.to_string());
                            return Ok(GeminiAuth::Bearer(new_tok.to_string()));
                        }
                    }
                }
            }
        }
    }
    get_gemini_key().map(GeminiAuth::ApiKey)
}

pub fn start_google_oauth_login(client_id: &str, client_secret: &str) -> Result<String, String> {
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::time::Instant;

    let cid = client_id.trim();
    if cid.is_empty() {
        return Err("Enter your Google OAuth Desktop Client ID below, or use 'Quick Login via AI Studio'.".into());
    }
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("Failed to start local login callback: {e}"))?;
    let _ = listener.set_nonblocking(true);
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let code_verifier = random_token();
    let state = random_token();

    let enc = |s: &str| -> String {
        s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
    };
    let scope = "https://www.googleapis.com/auth/generative-language.retriever https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/drive.file https://www.googleapis.com/auth/userinfo.email";
    let auth_url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&code_challenge={}&code_challenge_method=plain&state={}",
        enc(cid),
        enc(&redirect_uri),
        enc(scope),
        enc(&code_verifier),
        enc(&state),
    );
    let _ = Command::new("cmd").args(["/C", "start", "", &auth_url]).spawn();

    let deadline = Instant::now() + Duration::from_secs(90);
    let mut stream = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("Google login timed out (90s). Please try again.".into());
                }
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(e) => return Err(format!("Login listener failed: {e}")),
        }
    };
    let _ = stream.set_nonblocking(false);
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    let req_str = String::from_utf8_lossy(&buf[..n]);
    let first_line = req_str.lines().next().unwrap_or("");
    let path_part = first_line.split_whitespace().nth(1).unwrap_or("");

    if let Some(err_param) = path_part.split_once('?').and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("error="))) {
        let html = format!("<html><body style='font-family:sans-serif;text-align:center;padding:48px'><h2>Login failed: {}</h2><p>You can close this tab.</p></body></html>", decode(err_param));
        let resp_http = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", html.len(), html);
        let _ = stream.write_all(resp_http.as_bytes());
        return Err(format!("Google OAuth error: {}", decode(err_param)));
    }

    let returned_state = path_part.split_once('?').and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("state="))).map(decode);
    if returned_state.as_deref() != Some(state.as_str()) {
        return Err("Google login was rejected: the browser response did not match this sign-in attempt.".into());
    }

    let code = path_part
        .split_once('?')
        .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("code=")))
        .map(decode)
        .ok_or_else(|| "Google login did not return an authorization code.".to_string())?;

    let html = "<html><body style='font-family:sans-serif;text-align:center;padding:48px'><h2>Signed in to inkTeX</h2><p>You can close this browser tab and return to the app.</p></body></html>";
    let resp_http = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", html.len(), html);
    let _ = stream.write_all(resp_http.as_bytes());
    let _ = stream.flush();

    let csec = client_secret.trim();
    let mut params = vec![
        ("code", code.as_str()),
        ("client_id", cid),
        ("redirect_uri", redirect_uri.as_str()),
        ("grant_type", "authorization_code"),
        ("code_verifier", code_verifier.as_str()),
    ];
    if !csec.is_empty() {
        params.push(("client_secret", csec));
    }
    let tok_resp = ureq::post("https://oauth2.googleapis.com/token")
        .send_form(&params)
        .map_err(|e| format!("Token exchange failed: {e}"))?
        .into_json::<Value>()
        .map_err(|e| format!("Invalid token response: {e}"))?;

    let access_token = tok_resp["access_token"].as_str().ok_or("Missing access_token in response")?.to_string();
    let refresh_token = tok_resp["refresh_token"].as_str().unwrap_or("").to_string();
    let expires_in = tok_resp["expires_in"].as_u64().unwrap_or(3600);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();

    let email = ureq::get("https://www.googleapis.com/oauth2/v2/userinfo")
        .set("Authorization", &format!("Bearer {access_token}"))
        .call()
        .ok()
        .and_then(|r| r.into_json::<Value>().ok())
        .and_then(|u| u["email"].as_str().map(String::from))
        .unwrap_or_else(|| "Google Account".into());

    let _ = fs::create_dir_all(app_dir());
    let saved = json!({
        "client_id": cid,
        "client_secret": csec,
        "access_token": access_token,
        "refresh_token": refresh_token,
        "expires_at": now + expires_in,
        "email": email,
    });
    fs::write(oauth_path(), saved.to_string()).map_err(|e| e.to_string())?;
    Ok(email)
}

/// Extract all `<edit file="path">...content...</edit>` blocks from an AI response.
pub fn parse_file_edits(text: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut rest = text;
    while let Some(start) = rest.find("<edit ") {
        let after_tag = &rest[start + 6..];
        let Some(gt) = after_tag.find('>') else { break };
        let attrs = &after_tag[..gt];
        let file = attrs
            .split("file=")
            .nth(1)
            .and_then(|s| s.trim_start().strip_prefix('"'))
            .and_then(|s| s.split('"').next())
            .unwrap_or("")
            .trim()
            .to_string();
        let body_start = &after_tag[gt + 1..];
        let Some(end) = body_start.find("</edit>") else { break };
        let content = body_start[..end]
            .strip_prefix("\r\n")
            .or_else(|| body_start[..end].strip_prefix('\n'))
            .unwrap_or(&body_start[..end])
            .trim_end()
            .to_string();
        if !file.is_empty() {
            out.push((file, format!("{content}\n")));
        }
        rest = &body_start[end + "</edit>".len()..];
    }
    out
}

static LOG_LOCK: Mutex<()> = Mutex::new(());
const LOG_KEEP: usize = 1000;

/// One line per event in `<projects>/.activity.log` (JSON lines); only the newest LOG_KEEP are kept.
pub fn log_event(project: &str, action: &str, ok: bool, detail: &str) {
    let _l = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = workspace().join(".activity.log");
    let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let detail: String = detail.chars().take(300).collect();
    let mut lines: Vec<String> = fs::read_to_string(&path).unwrap_or_default().lines().map(String::from).collect();
    lines.push(json!({"time": time, "project": project, "action": action, "ok": ok, "detail": detail}).to_string());
    let start = lines.len().saturating_sub(LOG_KEEP);
    let _ = fs::write(&path, lines[start..].join("\n") + "\n"); // a log write must never fail the request
}

/// Logged events, newest first.
pub fn read_log() -> Vec<Value> {
    fs::read_to_string(workspace().join(".activity.log")).unwrap_or_default().lines().rev()
        .filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Windows' own bsdtar (unpacks zip); named explicitly so Git's GNU tar on PATH isn't picked up.
fn bsdtar() -> PathBuf {
    PathBuf::from(env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join(r"System32\tar.exe")
}

/// Unpack `zip` into the empty folder `tmp`; returns the folder holding the project's files.
fn unpack(zip_path: &Path, tmp: &Path, zip: &[u8]) -> Result<PathBuf, String> {
    fs::write(zip_path, zip).map_err(|e| e.to_string())?;
    fs::create_dir(tmp).map_err(|e| e.to_string())?;
    // bsdtar refuses entries with `..` or absolute paths, so a zip can't write outside `tmp`.
    // shortcut: no cap on unpacked size (only on the upload); add one if zips from strangers are ever accepted.
    let out = Command::new(bsdtar()).arg("-xf").arg(zip_path).arg("-C").arg(tmp).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("Could not unpack the zip: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let _ = fs::remove_dir_all(tmp.join("__MACOSX")); // macOS zip metadata
    let entries: Vec<PathBuf> = fs::read_dir(tmp).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    // A zip of one folder (GitHub-style) becomes that folder; Overleaf's export has the files at the top.
    let src = match entries.as_slice() {
        [only] if only.is_dir() => only.clone(),
        _ => tmp.to_path_buf(),
    };
    newest_tex(&src).ok_or("The zip has no .tex file at its top level.")?;
    Ok(src)
}

pub fn import_zip(file_name: &str, zip: &[u8]) -> Result<String, String> {
    let name = free_name(&Path::new(file_name).file_stem().map(|s| s.to_string_lossy()).unwrap_or_default())?;
    let id = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let (zip_path, tmp) = (workspace().join(format!(".upload-{id}.zip")), workspace().join(format!(".upload-{id}")));
    let res = unpack(&zip_path, &tmp, zip)
        .and_then(|src| fs::rename(src, workspace().join(&name)).map_err(|e| e.to_string()));
    let _ = fs::remove_file(&zip_path);
    let _ = fs::remove_dir_all(&tmp);
    res.map(|_| name)
}

/// Save an uploaded image in the project's figures folder (or top level) under a LaTeX-friendly, unused name.
/// Returns its path relative to the project, e.g. `figures/plot.png`.
pub fn save_image(root: &Path, file_name: &str, data: &[u8]) -> Result<String, String> {
    let folder = ["figures", "images", "figs", "img"].into_iter().find(|d| root.join(d).is_dir());
    let dir = folder.map_or(root.to_path_buf(), |d| root.join(d));
    let p = Path::new(file_name);
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !["png", "jpg", "jpeg", "pdf", "eps"].contains(&ext.as_str()) {
        return Err("Images must be PNG, JPG, PDF or EPS.".into());
    }
    // \includegraphics trips on spaces, extra dots and some symbols in names, so keep them plain.
    let stem: String = p.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default().chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(80).collect();
    let stem = if stem.is_empty() { "image".to_string() } else { stem };
    let name = (1..1000).map(|i| if i == 1 { format!("{stem}.{ext}") } else { format!("{stem}-{i}.{ext}") })
        .find(|n| !dir.join(n).exists()).ok_or("name taken")?;
    fs::write(dir.join(&name), data).map_err(|e| e.to_string())?;
    Ok(folder.map_or(name.clone(), |d| format!("{d}/{name}")))
}

pub fn save_asset(root: &Path, file_name: &str, data: &[u8]) -> Result<String, String> {
    let p = Path::new(file_name);
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ["png", "jpg", "jpeg", "pdf", "eps"].contains(&ext.as_str()) {
        return save_image(root, file_name, data);
    }
    let target = safe_path(root, file_name)?;
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&target, data).map_err(|e| e.to_string())?;
    Ok(file_name.to_string())
}

pub fn strip_fences(t: &str) -> String {
    let s = t.trim();
    if s.len() > 6 && s.starts_with("```") && s.ends_with("```") {
        if let Some(nl) = s.find('\n') {
            if nl + 1 <= s.len() - 3 {
                let inner = &s[nl + 1..s.len() - 3];
                return inner.strip_suffix('\n').unwrap_or(inner).to_string();
            }
        }
    }
    t.to_string()
}

/// Pull `file.tex:LINE: message` entries (pdflatex -file-line-error / tectonic) out of a log.
fn parse_errors(log: &str) -> Vec<Value> {
    let (mut errs, mut seen) = (vec![], HashSet::new());
    for line in log.lines() {
        if line.trim_start().starts_with("warning:") {
            continue; // tectonic's "warning: file.tex:12: Overfull \hbox" is not an error
        }
        if let Some(i) = line.find(".tex:") {
            let start = line[..i].rfind(char::is_whitespace).map_or(0, |j| j + 1);
            let rest = &line[i + 5..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() && rest[digits.len()..].starts_with(':') {
                let msg = rest[digits.len() + 1..].trim().to_string();
                let file = Path::new(&line[start..i + 4]).file_name().map(|f| f.to_string_lossy().to_string());
                if seen.insert((digits.clone(), msg.clone())) {
                    errs.push(json!({"file": file.unwrap_or_default(), "line": digits.parse::<u64>().unwrap_or(0), "msg": msg}));
                }
                continue;
            }
        }
        if errs.is_empty() && line.starts_with("! ") {
            errs.push(json!({"file": "", "line": 0, "msg": &line[2..]}));
        }
    }
    // A bare "! msg" is only a fallback; drop it once the same run gave located errors.
    if errs.iter().any(|e| e["line"] != 0) {
        errs.retain(|e| e["line"] != 0);
    }
    errs
}

fn which(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    env::split_paths(&env::var_os("PATH")?)
        .flat_map(|d| [d.join(name), d.join(format!("{name}.exe"))])
        .find(|p| p.is_file())
}

fn app_dir() -> PathBuf {
    let base = PathBuf::from(env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into()));
    let new_dir = base.join("texcraft-ai");
    let old_dir = base.join("latex-ai-editor");
    if old_dir.join("tectonic.exe").is_file() && !new_dir.join("tectonic.exe").is_file() {
        return old_dir;
    }
    new_dir
}

static DOWNLOAD: Mutex<()> = Mutex::new(());

/// Built-in engine: fetch Tectonic (one exe; pulls LaTeX packages on demand) into the app folder once.
fn builtin_tectonic() -> Result<PathBuf, String> {
    let _lock = DOWNLOAD.lock().unwrap_or_else(|e| e.into_inner());
    let (dir, exe) = (app_dir(), app_dir().join("tectonic.exe"));
    if exe.is_file() {
        return Ok(exe);
    }
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let releases: Value = ureq::get("https://api.github.com/repos/tectonic-typesetting/tectonic/releases")
        .set("User-Agent", "texcraft-ai")
        .call().map_err(|e| format!("Could not reach GitHub to get the LaTeX engine: {e}"))?
        .into_json().map_err(|e| e.to_string())?;
    let url = releases.as_array().into_iter().flatten()
        .filter(|r| r["prerelease"] == false)
        .flat_map(|r| r["assets"].as_array().into_iter().flatten())
        .filter_map(|a| a["browser_download_url"].as_str())
        .find(|u| u.ends_with("x86_64-pc-windows-msvc.zip"))
        .ok_or("No Windows build of Tectonic found on GitHub")?;
    let mut zip = vec![];
    ureq::get(url).call().map_err(|e| e.to_string())?.into_reader().read_to_end(&mut zip).map_err(|e| e.to_string())?;
    let zip_path = dir.join("tectonic.zip");
    fs::write(&zip_path, zip).map_err(|e| e.to_string())?;
    let st = Command::new(bsdtar()).arg("-xf").arg(&zip_path).arg("-C").arg(&dir).status().map_err(|e| e.to_string())?;
    let _ = fs::remove_file(&zip_path);
    if !st.success() || !exe.is_file() {
        return Err("Could not unpack the Tectonic LaTeX engine".into());
    }
    Ok(exe)
}

fn tectonic_args(file: &str) -> Vec<String> {
    vec!["--keep-logs".into(), "--synctex".into(), file.into()]
}

/// A compiler already on the PC (or the built-in one, once downloaded); never downloads.
pub fn find_compiler(file: &str) -> Option<(PathBuf, Vec<String>)> {
    let args = |a: &[&str]| a.iter().map(|s| s.to_string()).chain([file.to_string()]).collect();
    if let Some(t) = which("tectonic").or_else(|| Some(app_dir().join("tectonic.exe")).filter(|p| p.is_file())) {
        return Some((t, tectonic_args(file)));
    }
    if let (Some(l), Some(_)) = (which("latexmk"), which("perl")) {
        return Some((l, args(&["-pdf", "-interaction=nonstopmode", "-file-line-error", "-synctex=1"])));
    }
    let local = env::var("LOCALAPPDATA").unwrap_or_default() + r"\Programs\MiKTeX\miktex\bin\x64\pdflatex.exe";
    ["pdflatex", &local, r"C:\Program Files\MiKTeX\miktex\bin\x64\pdflatex.exe"].iter().find_map(|e| which(e))
        .map(|p| (p, args(&["-interaction=nonstopmode", "-file-line-error", "-synctex=1"])))
}

/// Prefer a compiler already on the PC; otherwise use (and on first use download) the built-in one.
fn compile_cmd(file: &str) -> Result<(PathBuf, Vec<String>), String> {
    match find_compiler(file) {
        Some(c) => Ok(c),
        None => Ok((builtin_tectonic()?, tectonic_args(file))),
    }
}

/// SyncTeX: the project file and line under a point on a PDF page; `x`, `y` are PDF points from the page's top-left.
pub fn synctex(root: &Path, pdf: &str, page: u32, x: f64, y: f64) -> Result<Value, String> {
    let gz = fs::File::open(safe_path(root, pdf)?.with_extension("synctex.gz"))
        .map_err(|_| "No position data for this PDF yet. Recompile, then double-click again.")?;
    let mut text = String::new();
    flate2::read::GzDecoder::new(gz).read_to_string(&mut text).map_err(|e| e.to_string())?;
    let (file, line) = synctex_find(&text, &root.to_string_lossy(), page, x, y).ok_or("No LaTeX line found at that spot.")?;
    Ok(json!({"file": file, "line": line}))
}

/// One SyncTeX box/glue/kern record: (kind, file tag, line, h, v, width, height, depth), lengths in sp.
type SyncRec = (u8, u32, u32, f64, f64, f64, f64, f64);

fn sync_rec(l: &str) -> Option<SyncRec> {
    let kind = *l.as_bytes().first()?;
    if !b"[(hvxkg$".contains(&kind) {
        return None;
    }
    let mut parts = l[1..].split(':');
    let mut tl = parts.next()?.split(',');
    let (tag, line) = (tl.next()?.parse().ok()?, tl.next()?.parse().ok()?);
    let mut hv = parts.next()?.split(',');
    let (h, v) = (hv.next()?.parse().ok()?, hv.next()?.parse().ok()?);
    let mut whd = parts.next().unwrap_or("").split(',').map(|s| s.parse::<f64>().unwrap_or(0.0));
    let mut n = || whd.next().unwrap_or(0.0);
    Some((kind, tag, line, h, v, n(), n(), n()))
}

/// Pick the record under (x, y) on `page`: the nearest word in the innermost text line there, else the
/// nearest record. Only files inside `root` count (packages and the TeX distribution are skipped).
// shortcut: assumes X/Y Offset 0 (true for pdfTeX and Tectonic PDF output); read them if another engine sets them.
fn synctex_find(text: &str, root: &str, page: u32, x: f64, y: f64) -> Option<(String, u32)> {
    let root = root.replace('\\', "/").trim_end_matches('/').to_string() + "/";
    let (mut files, mut recs) = (std::collections::HashMap::new(), Vec::<SyncRec>::new());
    let (mut unit, mut mag, mut on) = (1.0, 1000.0, false);
    let (open, close) = (format!("{{{page}"), format!("}}{page}"));
    for l in text.lines() {
        if on {
            if l == close {
                break;
            }
            recs.extend(sync_rec(l).filter(|r| r.2 > 0));
        } else if l == open {
            on = true;
        } else if let Some((tag, path)) = l.strip_prefix("Input:").and_then(|r| r.split_once(':')) {
            let p = path.replace('\\', "/");
            let rel = match p.get(..root.len()) {
                Some(head) if head.eq_ignore_ascii_case(&root) => p[root.len()..].to_string(),
                _ if !p.is_empty() && !p.contains(':') && !p.starts_with('/') => p.trim_start_matches("./").to_string(),
                _ => continue,
            };
            if let Ok(t) = tag.parse::<u32>() {
                files.insert(t, rel);
            }
        } else if let Some(u) = l.strip_prefix("Unit:") {
            unit = u.parse().unwrap_or(1.0);
        } else if let Some(m) = l.strip_prefix("Magnification:") {
            mag = m.parse().unwrap_or(1000.0);
        }
    }
    let sp = 65781.76 / (unit * mag / 1000.0); // big points -> sp
    let (px, py) = (x * sp, y * sp);
    let ours = |r: &&SyncRec| files.contains_key(&r.1);
    let line_box = recs.iter()
        .filter(|r| r.0 == b'(' && r.5 > 0.0 && r.3 <= px && px <= r.3 + r.5 && r.4 - r.6 <= py && py <= r.4 + r.7)
        .min_by(|a, b| a.5.total_cmp(&b.5));
    let best = line_box.and_then(|b| {
        recs.iter().filter(ours)
            .filter(|r| r.0 != b'(' && r.0 != b'[' && (r.4 - b.4).abs() < 65536.0 && r.3 >= b.3 && r.3 <= b.3 + b.5)
            .min_by(|a, c| (a.3 - px).abs().total_cmp(&(c.3 - px).abs()))
            .or(Some(b).filter(ours))
    }).or_else(|| {
        let d = |r: &&SyncRec| (r.3 - px).abs() + 4.0 * (r.4 - py).abs();
        recs.iter().filter(ours).min_by(|a, c| d(a).total_cmp(&d(c)))
    })?;
    Some((files[&best.1].clone(), best.2))
}

fn tail(s: &str, n: usize) -> &str {
    let start = s.char_indices().rev().nth(n.saturating_sub(1)).map_or(0, |(i, _)| i);
    &s[start..]
}

pub fn compile_tex(root: &Path, name: &str) -> Result<Value, String> {
    let path = safe_path(root, name)?;
    let file = path.file_name().unwrap().to_string_lossy().to_string();
    let (exe, args) = match compile_cmd(&file) {
        Ok(c) => c,
        Err(e) => return Ok(json!({"ok": false, "log": e, "errors": []})),
    };
    // shortcut: no compile timeout (nonstopmode never waits for input); add one if a compiler ever hangs.
    let out = Command::new(exe).args(args).current_dir(path.parent().unwrap()).output().map_err(|e| e.to_string())?;
    let console = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    let log = fs::read(path.with_extension("log")).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_else(|_| console.clone());
    let pdf = Path::new(name).with_extension("pdf");
    // Only top-level documents (the paper itself) go into PDF history, not diagrams or chapters.
    if out.status.success() && root.join(&pdf).exists() && !name.contains('/') {
        let _ = save_history(root, &pdf); // history is a convenience; a failed copy must not fail the build
    }
    Ok(json!({
        "ok": out.status.success(),
        "pdf": root.join(&pdf).exists().then(|| pdf.to_string_lossy().replace('\\', "/")),
        "log": tail(&log, 20000),
        "errors": parse_errors(&format!("{log}\n{console}")),
    }))
}

fn post(req: ureq::Request, body: Value) -> Result<Value, String> {
    match req.send_json(body) {
        Ok(r) => r.into_json().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, r)) => Err(format!("HTTP {code}: {}", r.into_string().unwrap_or_default())),
        Err(e) => Err(e.to_string()),
    }
}

pub fn ask_ai(b: &Value) -> Result<String, String> {
    let s = |k: &str| b[k].as_str().unwrap_or("").to_string();
    let write = s("mode") == "write";
    let system = if write { WRITE_SYSTEM } else { SYSTEM };
    let errors = if s("errors").is_empty() { "none".into() } else { s("errors") };
    let prompt = if write {
        format!("REQUEST:\n{}\n\n{}", s("instruction"), s("text"))
    } else {
        format!("INSTRUCTION:\n{}\n\nCOMPILE ERRORS:\n{errors}\n\nFILE:\n{}", s("instruction"), s("text"))
    };
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(900)).build();
    let text: String = match s("provider").as_str() {
        "claude" => {
            let key = get_claude_key()?;
            let model = model_of(b, "CLAUDE_MODEL", "claude-opus-5-5")?;
            // shortcut: non-streaming with a 15 min client timeout; switch to SSE streaming if huge papers time out.
            // fallbacks "default": if Opus declines, the API retries on a fallback model.
            let r = post(
                agent.post(&format!("{}/v1/messages", env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".into()).trim_end_matches('/')))
                    .set("x-api-key", &key)
                    .set("anthropic-version", "2026-10-21")
                    .set("anthropic-beta", "server-side-fallback-2026-07-01"),
                for_model(json!({"model": model, "max_tokens": 32000, "system": system, "fallbacks": "default",
                       "messages": [{"role": "user", "content": prompt}]}), &model),
            )?;
            claude_text(&r)?
        }
        "gemini" => {
            let auth = get_gemini_auth()?;
            let picked = model_of(b, "GEMINI_MODEL", "gemini-3.8-flash")?;
            let mut candidates = vec![picked];
            for fb in ["gemini-flash-lite-latest", "gemini-3.8-flash", "gemini-flash-latest"] {
                if !candidates.contains(&fb.to_string()) {
                    candidates.push(fb.to_string());
                }
            }
            let mut last_err = String::new();
            let mut res_text = None;
            for m in candidates {
                let mut req = agent.post(&format!("https://generativelanguage.googleapis.com/v1beta/models/{m}:generateContent"));
                req = match &auth {
                    GeminiAuth::Bearer(tok) => req.set("Authorization", &format!("Bearer {tok}")),
                    GeminiAuth::ApiKey(key) => req.set("x-goog-api-key", key).query("key", key),
                };
                let r = post(
                    req,
                    json!({"system_instruction": {"parts": [{"text": system}]},
                           "contents": [{"role": "user", "parts": [{"text": prompt}]}]}),
                );
                match r {
                    Ok(val) => match gemini_text(&val) {
                        Ok(t) => { res_text = Some(t); break; }
                        Err(e) => { last_err = e; }
                    },
                    Err(e) => { last_err = e; }
                }
            }
            res_text.ok_or_else(|| last_err)?
        }
        "ollama" => {
            let model = b["model"].as_str().filter(|m| !m.is_empty()).unwrap_or("llama3.2");
            let r = post(agent.post(&format!("{}/api/chat", ollama_url())), json!({"model": model, "stream": false,
                "messages": [{"role": "system", "content": system}, {"role": "user", "content": prompt}]}))
                .map_err(|e| format!("Local model unavailable ({e}). Install Ollama from ollama.com, then run: ollama pull llama3.2"))?;
            r["message"]["content"].as_str().unwrap_or("").to_string()
        }
        _ => return Err("unknown provider".into()),
    };
    if text.trim().is_empty() {
        return Err("AI returned an empty answer.".into());
    }
    Ok(strip_fences(&text))
}

type Emit<'a> = &'a mut dyn FnMut(Value) -> std::io::Result<()>;

fn ollama_url() -> String {
    env::var("OLLAMA_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".into())
}

/// Models on the local Ollama server, if it's running.
pub fn ollama_models() -> Vec<String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_millis(800)).build();
    agent.get(&format!("{}/api/tags", ollama_url())).call().ok().and_then(|r| r.into_json::<Value>().ok())
        .map(|v| v["models"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn open_stream(req: ureq::Request, body: Value) -> Result<impl BufRead, String> {
    match req.send_json(body) {
        Ok(r) => Ok(BufReader::new(r.into_reader())),
        Err(ureq::Error::Status(code, r)) => Err(format!("HTTP {code}: {}", r.into_string().unwrap_or_default())),
        Err(e) => Err(e.to_string()),
    }
}

fn push_str(block: &mut Value, key: &str, t: &str) {
    match block.get_mut(key) {
        Some(Value::String(s)) => s.push_str(t),
        _ => block[key] = json!(t),
    }
}

/// The reply as it must be echoed next turn: before the last `fallback` block (a mid-answer switch to the
/// fallback model) only text may go back; the fallback block itself stays where it was.
fn claude_echo(blocks: Vec<Value>) -> Vec<Value> {
    let cut = blocks.iter().rposition(|b| b["type"] == "fallback");
    blocks.into_iter().enumerate()
        .filter(|(i, b)| cut.map_or(true, |c| *i >= c || b["type"] == "text"))
        .map(|(_, b)| b).collect()
}

/// Stream one chat answer: `{"t": text}` events as it's written, then returns the final
/// `{"done": true, "content": â€¦, "note": â€¦}`. `content` is what the browser sends back next turn
/// (Claude's blocks verbatim, thinking included; plain text for the others).
pub fn chat_stream(b: &Value, emit: Emit) -> Result<Value, String> {
    let msgs = b["messages"].as_array().filter(|m| !m.is_empty()).ok_or("no messages")?;
    let text_of = |m: &Value| m["text"].as_str().or(m["content"].as_str()).unwrap_or("").to_string();
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(300)).build();
    let out = |e: std::io::Error| format!("browser disconnected: {e}");
    match b["provider"].as_str().unwrap_or("") {
        "claude" => {
            let key = get_claude_key()?;
            let model = model_of(b, "CLAUDE_MODEL", "claude-opus-5-5")?;
            let messages: Vec<Value> = msgs.iter().map(|m| {
                let content = if m["role"] == "assistant" && m["content"].is_array() { m["content"].clone() } else { json!(text_of(m)) };
                json!({"role": if m["role"] == "assistant" { "assistant" } else { "user" }, "content": content})
            }).collect();
            // drop_block: if the replayed history ever doesn't match, the API drops stale thinking instead of failing.
            let base = env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".into());
            let lines = open_stream(
                agent.post(&format!("{}/v1/messages", base.trim_end_matches('/')))
                    .set("x-api-key", &key)
                    .set("anthropic-version", "2026-10-21")
                    .set("anthropic-beta", "server-side-fallback-2026-07-01,thinking-binding-controls-2026-08-01"),
                for_model(json!({"model": model, "max_tokens": 32000, "stream": true, "system": CHAT_SYSTEM, "fallbacks": "default",
                       "thinking": {"type": "adaptive", "block_binding": {"prefix_mismatch_behavior": "drop_block"}},
                       "output_config": {"effort": "medium"}, "messages": messages}), &model),
            )?;
            let (mut blocks, mut stop): (Vec<Value>, String) = (vec![], String::new());
            for line in lines.lines() {
                let line = line.map_err(|e| e.to_string())?;
                let Some(ev) = line.strip_prefix("data: ").and_then(|d| serde_json::from_str::<Value>(d).ok()) else { continue };
                match ev["type"].as_str().unwrap_or("") {
                    "content_block_start" => blocks.push(ev["content_block"].clone()),
                    "content_block_delta" => {
                        let (d, i) = (&ev["delta"], ev["index"].as_u64().unwrap_or(0) as usize);
                        let Some(block) = blocks.get_mut(i) else { continue };
                        match d["type"].as_str().unwrap_or("") {
                            "text_delta" => {
                                let t = d["text"].as_str().unwrap_or("");
                                push_str(block, "text", t);
                                emit(json!({"t": t})).map_err(out)?;
                            }
                            "thinking_delta" => push_str(block, "thinking", d["thinking"].as_str().unwrap_or("")),
                            "signature_delta" => block["signature"] = d["signature"].clone(),
                            _ => {}
                        }
                    }
                    "message_delta" => if let Some(s) = ev["delta"]["stop_reason"].as_str() { stop = s.into() },
                    "error" => return Err(ev["error"]["message"].as_str().unwrap_or("Claude stream error").into()),
                    _ => {}
                }
            }
            let note = match stop.as_str() {
                "refusal" => Some("Claude declined this request."),
                "max_tokens" => Some("The answer hit the length limit and was cut off."),
                _ => None,
            };
            Ok(json!({"done": true, "content": claude_echo(blocks), "note": note}))
        }
        "gemini" => {
            let auth = get_gemini_auth()?;
            let picked = model_of(b, "GEMINI_MODEL", "gemini-3.8-flash")?;
            let mut candidates = vec![picked];
            for fb in ["gemini-flash-lite-latest", "gemini-3.8-flash", "gemini-flash-latest"] {
                if !candidates.contains(&fb.to_string()) {
                    candidates.push(fb.to_string());
                }
            }
            let contents: Vec<Value> = msgs.iter().map(|m| json!({"role": if m["role"] == "assistant" { "model" } else { "user" }, "parts": [{"text": text_of(m)}]})).collect();
            let mut stream_res = Err("Gemini request failed".to_string());
            for m in candidates {
                let mut req = agent.post(&format!("https://generativelanguage.googleapis.com/v1beta/models/{m}:streamGenerateContent?alt=sse"));
                req = match &auth {
                    GeminiAuth::Bearer(tok) => req.set("Authorization", &format!("Bearer {tok}")),
                    GeminiAuth::ApiKey(key) => req.set("x-goog-api-key", key).query("key", key),
                };
                let call = open_stream(
                    req,
                    json!({"system_instruction": {"parts": [{"text": CHAT_SYSTEM}]}, "contents": contents.clone()}),
                );
                match call {
                    Ok(lines) => { stream_res = Ok(lines); break; }
                    Err(e) => { stream_res = Err(e); }
                }
            }
            let lines = stream_res?;
            let mut all = String::new();
            for line in lines.lines() {
                let line = line.map_err(|e| e.to_string())?;
                let Some(ev) = line.strip_prefix("data: ").and_then(|d| serde_json::from_str::<Value>(d).ok()) else { continue };
                let t = gemini_text(&ev)?; // same block / cut-off checks as AI Assist, per chunk
                if !t.is_empty() {
                    all.push_str(&t);
                    emit(json!({"t": t})).map_err(out)?;
                }
            }
            Ok(json!({"done": true, "content": all}))
        }
        "ollama" => {
            let model = b["model"].as_str().filter(|m| !m.is_empty()).unwrap_or("llama3.2");
            let messages: Vec<Value> = std::iter::once(json!({"role": "system", "content": CHAT_SYSTEM}))
                .chain(msgs.iter().map(|m| json!({"role": if m["role"] == "assistant" { "assistant" } else { "user" }, "content": text_of(m)})))
                .collect();
            let lines = open_stream(agent.post(&format!("{}/api/chat", ollama_url())), json!({"model": model, "messages": messages, "stream": true}))
                .map_err(|e| format!("Local model unavailable ({e}). Install Ollama from ollama.com, then run: ollama pull llama3.2"))?;
            let mut all = String::new();
            for line in lines.lines() {
                let Ok(ev) = serde_json::from_str::<Value>(&line.map_err(|e| e.to_string())?) else { continue };
                if let Some(e) = ev["error"].as_str() {
                    return Err(format!("Ollama: {e}"));
                }
                let t = ev["message"]["content"].as_str().unwrap_or("");
                if !t.is_empty() {
                    all.push_str(t);
                    emit(json!({"t": t})).map_err(out)?;
                }
                if ev["done"] == true {
                    break;
                }
            }
            Ok(json!({"done": true, "content": all}))
        }
        _ => Err("Pick a model.".into()),
    }
}

/// Text of a Messages API reply; refusals and truncation become errors so a partial file is never applied.
fn claude_text(r: &Value) -> Result<String, String> {
    match r["stop_reason"].as_str() {
        Some("refusal") => return Err("Claude declined this request.".into()),
        Some("max_tokens") => return Err("Output hit max_tokens; file too large to rewrite in one go.".into()),
        _ => {}
    }
    Ok(r["content"].as_array().into_iter().flatten()
        .filter(|c| c["type"] == "text").filter_map(|c| c["text"].as_str()).collect())
}

/// Text of a Gemini generateContent reply; blocked or cut-off answers become errors.
fn gemini_text(r: &Value) -> Result<String, String> {
    if let Some(reason) = r["promptFeedback"]["blockReason"].as_str() {
        return Err(format!("Gemini blocked the request ({reason})."));
    }
    let c = &r["candidates"][0];
    match c["finishReason"].as_str() {
        Some("MAX_TOKENS") => return Err("Output hit Gemini's token limit; file too large to rewrite in one go.".into()),
        Some(f @ ("SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT")) => return Err(format!("Gemini stopped the answer ({f}).")),
        _ => {}
    }
    // Thinking models may return "thought" parts; only the answer parts belong in the file.
    Ok(c["content"]["parts"].as_array().into_iter().flatten()
        .filter(|p| p["thought"] != true).filter_map(|p| p["text"].as_str()).collect())
}

pub fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let (p, name) = (e.path(), e.file_name().to_string_lossy().to_string());
        if name.starts_with('.') || name == "target" {
            continue;
        }
        if p.is_dir() {
            walk(root, &p, out);
        } else if [".tex", ".bib", ".png", ".jpg", ".jpeg", ".eps", ".sty", ".cls", ".txt", ".csv"].iter().any(|x| name.to_lowercase().ends_with(x))
            // a PDF next to a same-named .tex is build output; other PDFs are figures
            || (name.to_lowercase().ends_with(".pdf") && !p.with_extension("tex").exists()) {
            out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
        }
    }
}

pub fn inject_project_context(root: &Path, b: &mut Value) {
    let mut files = Vec::new();
    walk(root, root, &mut files);
    let mut text_files: Vec<String> = files.into_iter()
        .filter(|f| {
            let lf = f.to_lowercase();
            lf.ends_with(".tex") || lf.ends_with(".bib") || lf.ends_with(".sty") || lf.ends_with(".cls") || lf.ends_with(".txt")
        })
        .collect();

    if text_files.is_empty() {
        return;
    }

    text_files.sort_by(|a, b| {
        let score = |s: &str| -> i32 {
            let ls = s.to_lowercase();
            if ls == "main.tex" { 0 }
            else if ls.contains("frontmatter") || ls.contains("abstract") { 1 }
            else if ls.contains("abbrev") || ls.contains("acronym") { 2 }
            else if ls.starts_with("chapters/") || ls.starts_with("chapter") { 3 }
            else if ls.ends_with(".tex") { 4 }
            else if ls.ends_with(".bib") { 5 }
            else { 6 }
        };
        score(a).cmp(&score(b)).then_with(|| a.cmp(b))
    });

    let mut context = String::new();
    let mut total_bytes = 0usize;
    const MAX_CONTEXT_BYTES: usize = 400 * 1024; // 400 KB

    for rel in text_files {
        if total_bytes >= MAX_CONTEXT_BYTES {
            break;
        }
        let tag = format!("<file name=\"{}\"", rel);
        let already_present = b["messages"].as_array().map_or(false, |arr| {
            arr.iter().any(|m| {
                let t = m["text"].as_str().or(m["content"].as_str()).unwrap_or("");
                t.contains(&tag)
            })
        });
        if already_present {
            continue;
        }
        let full_path = root.join(&rel);
        if let Ok(content) = fs::read_to_string(&full_path) {
            let bytes_left = MAX_CONTEXT_BYTES.saturating_sub(total_bytes);
            let slice = if content.len() > bytes_left {
                &content[..content.char_indices().map(|(i, _)| i).take_while(|&i| i <= bytes_left).last().unwrap_or(0)]
            } else {
                &content[..]
            };
            context.push_str(&format!("<file name=\"{}\">\n{}\n</file>\n\n", rel, slice));
            total_bytes += slice.len();
        }
    }

    if context.is_empty() {
        return;
    }

    if let Some(msgs) = b["messages"].as_array_mut() {
        if let Some(last_msg) = msgs.iter_mut().rev().find(|m| m["role"] == "user") {
            let existing_text = last_msg["text"].as_str().or(last_msg["content"].as_str()).unwrap_or("").to_string();
            let new_text = format!("{}\nUser Question / Instructions:\n{}", context.trim(), existing_text);
            if last_msg["text"].is_string() {
                last_msg["text"] = json!(new_text);
            } else {
                last_msg["content"] = json!(new_text);
            }
        }
    }
}

fn decode(s: &str) -> String {
    let (b, mut out, mut i) = (s.as_bytes(), vec![], 0);
    while i < b.len() {
        match (b[i], s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(v)) => { out.push(v); i += 3; }
            (b'+', _) => { out.push(b' '); i += 1; }
            (c, _) => { out.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences() {
        assert_eq!(strip_fences("```latex\n\\section{A}\n```"), "\\section{A}");
        assert_eq!(strip_fences("\\section{A}"), "\\section{A}");
        assert_eq!(strip_fences("```\n```"), "");
    }

    #[test]
    fn errors() {
        let e = parse_errors("./paper.tex:12: Undefined control sequence.\n! Emergency stop.");
        assert_eq!(e, vec![json!({"file": "paper.tex", "line": 12, "msg": "Undefined control sequence."})]);
        assert_eq!(parse_errors("error: sub/a.tex:3: Missing $")[0]["file"], "a.tex");
        assert_eq!(parse_errors("! Missing $ inserted.")[0]["msg"], "Missing $ inserted.");
        assert!(parse_errors("warning: main.tex:34: Overfull \\hbox (1.9pt too wide)").is_empty());
        // tectonic: log's "!" line comes first, console's located error later -> keep only the located one
        let e = parse_errors("! Undefined control sequence.\nl.5 \\bad\nerror: main.tex:5: Undefined control sequence");
        assert_eq!(e, vec![json!({"file": "main.tex", "line": 5, "msg": "Undefined control sequence"})]);
    }

    #[test]
    fn traversal_blocked() {
        for bad in ["../x.tex", "a/../../x", r"C:\Windows\win.ini", "/etc/passwd", ""] {
            assert!(safe_path(Path::new("p"), bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn project_names() {
        let ws = env::temp_dir().join("latex-ai-editor-test-ws");
        fs::create_dir_all(ws.join("My paper")).unwrap();
        let _ = WORKSPACE.set(ws);
        assert!(project_dir("My paper").is_ok());
        for bad in ["..", "My paper/..", r"C:\Windows", "a/b", ".upload-1", "", "missing"] {
            assert!(project_dir(bad).is_err(), "{bad}");
        }
        assert_eq!(clean_name("  a/b:c?. "), "a_b_c_");
        assert_eq!(free_name("My paper").unwrap(), "My paper (2)");
        assert!(free_name(" .. ").is_err());
    }

    #[test]
    fn tex_escaping() {
        assert_eq!(tex_escape("A & B_1 {x} 100% #1 $5"), r"A \& B\_1 \{x\} 100\% \#1 \$5");
        assert_eq!(tex_escape(r"a\b~^"), r"a\textbackslash{}b\textasciitilde{}\textasciicircum{}");
    }

    #[test]
    fn log_keeps_newest() {
        let ws = env::temp_dir().join("latex-ai-editor-test-ws");
        fs::create_dir_all(&ws).unwrap();
        let _ = WORKSPACE.set(ws);
        let _ = fs::remove_file(workspace().join(".activity.log"));
        for i in 0..LOG_KEEP + 5 {
            log_event("p", "Compile", i % 2 == 0, &i.to_string());
        }
        let l = read_log();
        assert_eq!(l.len(), LOG_KEEP);
        assert_eq!(l[0]["detail"], (LOG_KEEP + 4).to_string()); // newest first
        assert_eq!(l[0]["ok"], true);
    }

    #[test]
    fn history_keeps_newest() {
        let root = env::temp_dir().join(format!("latex-ai-editor-hist-{}", std::process::id()));
        fs::create_dir_all(root.join(".history")).unwrap();
        for s in 1..=12 {
            fs::write(root.join(format!(".history/{s}-main.pdf")), "old").unwrap();
        }
        fs::write(root.join("main.pdf"), "new").unwrap();
        save_history(&root, Path::new("main.pdf")).unwrap();
        let h = history(&root);
        assert_eq!(h.len(), HISTORY_KEEP);
        assert_eq!(fs::read_to_string(root.join(".history").join(&h[0].0)).unwrap(), "new"); // newest first
        assert!(!root.join(".history/3-main.pdf").exists()); // oldest pruned
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn image_names() {
        let root = env::temp_dir().join(format!("latex-ai-editor-img-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        assert_eq!(save_image(&root, "My Fig.v2.PNG", b"x").unwrap(), "My_Fig_v2.png");
        assert_eq!(save_image(&root, "My Fig.v2.png", b"x").unwrap(), "My_Fig_v2-2.png"); // never overwrites
        assert!(save_image(&root, "notes.exe", b"x").is_err());
        assert!(save_image(&root, "../../evil.png", b"x").unwrap() == "evil.png" && root.join("evil.png").is_file());
        fs::create_dir(root.join("figures")).unwrap();
        assert_eq!(save_image(&root, "plot.jpg", b"x").unwrap(), "figures/plot.jpg");
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn zip_of_one_folder_is_unwrapped() {
        let t = env::temp_dir().join(format!("latex-ai-editor-zip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&t);
        fs::create_dir_all(t.join("src/paper")).unwrap();
        fs::write(t.join("src/paper/main.tex"), TEMPLATE).unwrap();
        let st = Command::new(bsdtar()).args(["-a", "-cf"]).arg(t.join("p.zip")).arg("-C").arg(t.join("src")).arg("paper").status().unwrap();
        assert!(st.success());
        let zip = fs::read(t.join("p.zip")).unwrap();
        let src = unpack(&t.join("up.zip"), &t.join("out"), &zip).unwrap();
        assert!(src.ends_with("paper") && src.join("main.tex").is_file());
        assert!(unpack(&t.join("bad.zip"), &t.join("out2"), b"not a zip").is_err());
        fs::remove_dir_all(&t).unwrap();
    }

    #[test]
    fn claude_key_shape_and_tokens() {
        assert!(save_claude_key("AIzaNotClaude1234567890").is_err());
        assert!(save_claude_key("sk-ant-x").is_err());
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
    }

    #[test]
    fn claude_replies() {
        let ok = json!({"stop_reason": "end_turn", "content": [
            {"type": "thinking", "thinking": ""}, {"type": "text", "text": "\\section{A}"}]});
        assert_eq!(claude_text(&ok).unwrap(), "\\section{A}");
        assert!(claude_text(&json!({"stop_reason": "refusal", "content": []})).is_err());
        assert!(claude_text(&json!({"stop_reason": "max_tokens", "content": [{"type": "text", "text": "half"}]})).is_err());
    }

    #[test]
    fn synctex_finds_word_line() {
        let t = "SyncTeX Version:1\nInput:1:C:\\proj\\main.tex\nInput:2:C:\\proj\\chapters/intro.tex\nInput:3:\n\
                 Input:4:C:\\texlive\\article.cls\nUnit:1\nMagnification:1000\nContent:\n{1\n\
                 [1,5:4736287,52610785:29685704,47874498,0\n(2,7:4736287,6000000:20000000,500000,100000\n\
                 g2,7:4736287,6000000\ng2,8:10000000,6000000\n)\n(4,3:4736287,9000000:20000000,500000,100000\n\
                 g4,3:5000000,9000000\ng1,12:6000000,9000000\n)\n]\n}1\n{2\n}2\n";
        let bp = |sp: f64| sp / 65781.76;
        // a word in the chapter's line: the nearest glue in that line wins
        assert_eq!(synctex_find(t, "C:\\proj", 1, bp(9_900_000.0), bp(5_900_000.0)), Some(("chapters/intro.tex".into(), 8)));
        // the line box belongs to a package file, but the word inside it is ours
        assert_eq!(synctex_find(t, "c:/PROJ/", 1, bp(5_500_000.0), bp(9_000_000.0)), Some(("main.tex".into(), 12)));
        assert_eq!(synctex_find(t, "C:\\proj", 2, 10.0, 10.0), None); // empty page
    }

    #[test]
    fn model_pick_and_haiku_body() {
        assert_eq!(model_of(&json!({"model": "claude-sonnet-5-5"}), "NO_SUCH_VAR_X", "d").unwrap(), "claude-sonnet-5-5");
        assert_eq!(model_of(&json!({}), "NO_SUCH_VAR_X", "d").unwrap(), "d");
        assert!(model_of(&json!({"model": "x/../y?key=1"}), "NO_SUCH_VAR_X", "d").is_err());
        let b = for_model(json!({"fallbacks": "default", "thinking": {}, "model": "m"}), "claude-haiku-5-5");
        assert!(b.get("fallbacks").is_none() && b.get("thinking").is_none());
        assert!(for_model(json!({"fallbacks": "default"}), "claude-opus-5-5").get("fallbacks").is_some());
        assert_eq!(model_list("NO_SUCH_VAR_X", &["a", "b", "a"]), vec!["a", "b"]);
    }

    #[test]
    fn chat_echo_after_fallback() {
        let plain = vec![json!({"type": "thinking", "thinking": "", "signature": "s"}), json!({"type": "text", "text": "hi"})];
        assert_eq!(claude_echo(plain.clone()), plain); // no fallback: everything goes back verbatim
        let fb = vec![
            json!({"type": "thinking", "thinking": "", "signature": "a"}), json!({"type": "text", "text": "par"}),
            json!({"type": "fallback"}), json!({"type": "thinking", "thinking": "", "signature": "b"}), json!({"type": "text", "text": "tial"}),
        ];
        let e = claude_echo(fb);
        assert_eq!(e.iter().map(|b| b["type"].as_str().unwrap()).collect::<Vec<_>>(), ["text", "fallback", "thinking", "text"]);
        assert_eq!(e[2]["signature"], "b");
        let mut b = json!({"type": "text", "text": "a"});
        push_str(&mut b, "text", "b");
        push_str(&mut b, "thinking", "t");
        assert_eq!((b["text"].as_str(), b["thinking"].as_str()), (Some("ab"), Some("t")));
    }

    #[test]
    fn gemini_replies() {
        let ok = json!({"candidates": [{"finishReason": "STOP", "content": {"parts": [
            {"text": "plan...", "thought": true}, {"text": "\\section{A}"}]}}]});
        assert_eq!(gemini_text(&ok).unwrap(), "\\section{A}");
        assert!(gemini_text(&json!({"promptFeedback": {"blockReason": "SAFETY"}})).is_err());
        assert!(gemini_text(&json!({"candidates": [{"finishReason": "MAX_TOKENS", "content": {"parts": [{"text": "half"}]}}]})).is_err());
    }

    #[test]
    fn url_decode() {
        assert_eq!(decode("my%20paper%2Fmain.tex"), "my paper/main.tex");
        assert_eq!(decode("100%"), "100%");
    }

    #[test]
    fn parses_multi_file_edits() {
        let reply = "Here is the fix:\n<edit file=\"chapters/01_intro.tex\">\n\\section{Intro}\nFixed.\n</edit>\n";
        let edits = parse_file_edits(reply);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].0, "chapters/01_intro.tex");
        assert_eq!(edits[0].1, "\\section{Intro}\nFixed.\n");
    }
}
