use crate::backend::{
    ask_ai, builtin_google_client, chat_stream, compile_tex, get_claude_key, get_gemini_auth, get_gemini_key, google_oauth_info, history,
    import_zip, inject_project_context, list_projects, log_event, make_diagram, model_list,
    new_project, ollama_models, parse_file_edits, profile, project_dir, read_log, safe_path,
    save_asset, save_claude_key, save_gemini_key, save_profile, sign_out_claude, sign_out_google, start_google_oauth_login,
    strip_fences, synctex, walk, CLAUDE_MODELS, GEMINI_MODELS, LOGO, WRITE_SYSTEM,
};
use crate::editor::{
    char_of_line, ensure_package, highlight_latex, insert_at_cursor, line_col_of_char,
    toggle_comment, wrap_selection,
};
use crate::gdrive::{
    link_drive_folder, load_drive_meta, share_drive_folder, sync_project_with_drive,
    DriveSyncReport,
};
use crate::pdf::{render_pdf_file, RenderedPage};
use crate::theme::{install_fonts, paint_glass_backdrop, ThemeKind, ThemePalette};
use eframe::egui::{
    self, Align, Color32, ColorImage, CornerRadius, FontId, Frame, Key, Layout, Margin, RichText,
    ScrollArea, Sense, Stroke, TextEdit, TextureHandle, TextureOptions, Ui, Vec2,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Code,
    Split,
    Pdf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AiTab {
    Chat,
    Edit,
}

#[derive(Clone)]
struct CompileError {
    file: String,
    line: usize,
    msg: String,
}

struct ChatMessage {
    role: String, // "user" | "assistant"
    text: String,
    thinking: String,
}

enum ChatEvent {
    Thinking(String),
    Text(String),
    Done(Result<Value, String>),
}

enum BgResult {
    Compiled {
        ok: bool,
        pdf: Option<String>,
        log: String,
        errors: Vec<CompileError>,
        ms: u64,
    },
    PdfRendered {
        path: String,
        pages: Result<Vec<RenderedPage>, String>,
    },
    AiEdit(Result<String, String>, bool), // (text, recompile_after)
    HelpWrite(Result<String, String>),
    DiagramBuilt(Result<(String, String), String>), // (pdf_rel_path, caption)
    GoogleLogin(Result<String, String>),
    ClipboardKey(&'static str, String),
    Thumb { key: String, stamp: u64, image: Option<ColorImage> },
    DriveSynced(Result<DriveSyncReport, String>),
    DriveShared(Result<String, String>),
}

pub struct TexCraftApp {
    theme: ThemeKind,
    text_scale: f32,
    icon_scale: f32,
    logo_tex: Option<TextureHandle>,
    status_msg: String,
    status_is_err: bool,

    // Home state
    projects: Vec<Value>,
    project_search: String,
    open_project: Option<String>,
    open_project_root: Option<PathBuf>,

    // Profile & Keys
    prof_name: String,
    prof_affil: String,
    prof_email: String,
    gemini_key_input: String,
    gemini_key_show: bool,
    gemini_key_set: bool,
    oauth_client_id: String,
    oauth_client_secret: String,
    google_signed_in_email: Option<String>,
    google_login_busy: bool,
    clipboard_watch_until: Option<Instant>,
    clipboard_watch_kind: &'static str, // "gemini" | "claude"
    show_login: bool,
    ribbon_tab: u8,
    last_typing: Option<(Instant, Instant)>, // (burst start, last keystroke) for grouped undo
    pending_cursor: Option<usize>,           // move the editor caret here next frame (after a toolbar edit)
    thumbs: HashMap<String, (u64, Option<TextureHandle>)>, // project -> (pdf mtime, first-page thumbnail)
    thumb_jobs: usize,
    pinned: BTreeSet<String>,
    ai_autofix: bool,       // user permission: let the AI fix compile errors and apply the fix without review
    autofix_attempts: u8,   // tries since the user last typed; capped at AUTOFIX_MAX
    autofix_pending: bool,  // the running AI edit is an auto-fix
    claude_key_set: bool,
    compiler_desc: String,
    show_profile_dlg: bool,
    profile_err: String,

    // Modals
    show_new_proj_dlg: bool,
    new_proj_name: String,
    new_proj_template: String,
    confirm_delete_proj: Option<String>,
    show_logs_dlg: bool,
    logs_cache: Vec<Value>,

    // Editor state
    view_mode: ViewMode,
    files: Vec<String>,
    collapsed_dirs: BTreeSet<String>,
    current_file: String,
    code_text: String,
    saved_text: String,
    undo_stack: Vec<String>,
    redo_stack: Vec<String>,
    last_edit_time: Option<Instant>,
    auto_save: bool,
    cursor_range: Option<(usize, usize)>,
    jump_to_line: Option<usize>,

    // File actions
    show_new_file_dlg: bool,
    new_file_name: String,
    rename_file_target: Option<String>,
    rename_file_input: String,
    confirm_delete_file: Option<String>,

    // Compile & PDF
    compiling: bool,
    compile_log: String,
    compile_errors: Vec<CompileError>,
    show_logs_drawer: bool,
    current_pdf_rel: Option<String>,
    pdf_zoom: f32,
    pdf_pages: Vec<(TextureHandle, f32, f32)>,
    pdf_rendering: bool,
    pdf_err: String,
    pdf_history: Vec<(String, u64)>,
    preview_image: Option<(String, TextureHandle)>,

    // AI state
    show_files_drawer: bool,
    show_ai_drawer: bool,
    ai_tab: AiTab,
    models: Vec<(String, String, String)>, // (display, provider, model_id)
    selected_model_idx: usize,
    full_project_access: bool,
    chat_messages: Vec<ChatMessage>,
    chat_input: String,
    chat_streaming: bool,
    chat_rx: Option<mpsc::Receiver<ChatEvent>>,
    chat_cancel: Arc<AtomicBool>,
    ai_edit_instruction: String,
    ai_edit_busy: bool,
    ai_diff_proposal: Option<(String, String, bool)>, // (original, proposed, recompile_after)

    // Tool dialogs
    show_image_dlg: bool,
    img_dlg_file: String,
    img_dlg_width: String,
    img_dlg_caption: String,
    img_dlg_pos: String,

    show_table_dlg: bool,
    tbl_rows: usize,
    tbl_cols: usize,
    tbl_align: String,
    tbl_caption: String,

    show_nn_dlg: bool,
    nn_name: String,
    nn_spec: String,
    nn_caption: String,
    nn_busy: bool,

    show_symbols_dlg: bool,
    show_hw_dlg: bool,
    hw_prompt: String,
    hw_busy: bool,
    hw_result: Option<String>,

    // Google Drive Live Sync & Share
    show_share_dlg: bool,
    drive_syncing: bool,
    live_sync: bool,
    last_live_sync: Instant,
    drive_folder_id: String,
    drive_share_url: String,
    drive_collab_email: String,
    drive_collab_role: String,
    drive_link_input: String,
    drive_msg: String,

    bg_tx: mpsc::Sender<BgResult>,
    bg_rx: mpsc::Receiver<BgResult>,
}

impl TexCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_project: Option<String>) -> Self {
        let text_scale = cc.egui_ctx.data_mut(|d| d.get_persisted::<f32>(egui::Id::new(TEXT_SCALE_ID))).unwrap_or(1.0);
        let icon_scale = cc.egui_ctx.data_mut(|d| d.get_persisted::<f32>(egui::Id::new(ICON_SCALE_ID))).unwrap_or(1.0);
        install_fonts(&cc.egui_ctx, text_scale);
        let prof = profile();
        let theme = prof["theme"]
            .as_str()
            .map(ThemeKind::from_id)
            .unwrap_or(ThemeKind::Dark);
        theme.apply(&cc.egui_ctx);

        let logo_tex = image::load_from_memory(LOGO).ok().map(|img| {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let color_img = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            cc.egui_ctx.load_texture("texcraft_logo", color_img, TextureOptions::LINEAR)
        });

        let (bg_tx, bg_rx) = mpsc::channel();
        let mut app = Self {
            theme,
            text_scale,
            icon_scale,
            logo_tex,
            status_msg: "Ready".into(),
            status_is_err: false,

            projects: list_projects(),
            project_search: String::new(),
            open_project: None,
            open_project_root: None,

            prof_name: prof["name"].as_str().unwrap_or("").to_string(),
            prof_affil: prof["affiliation"].as_str().unwrap_or("").to_string(),
            prof_email: prof["email"].as_str().unwrap_or("").to_string(),
            gemini_key_input: get_gemini_key().unwrap_or_default(),
            gemini_key_show: false,
            gemini_key_set: get_gemini_auth().is_ok(),
            oauth_client_id: google_oauth_info()
                .map(|(_, cid)| cid)
                .filter(|s| !s.is_empty())
                .or_else(|| std::env::var("GOOGLE_OAUTH_CLIENT_ID").ok())
                .unwrap_or_else(|| prof["google_client_id"].as_str().unwrap_or("").to_string()),
            oauth_client_secret: prof["google_client_secret"].as_str().unwrap_or("").to_string(),
            google_signed_in_email: google_oauth_info().map(|(em, _)| em),
            google_login_busy: false,
            clipboard_watch_until: None,
            clipboard_watch_kind: "gemini",
            show_login: false,
            ribbon_tab: 0,
            last_typing: None,
            pending_cursor: None,
            thumbs: HashMap::new(),
            thumb_jobs: 0,
            ai_autofix: cc.egui_ctx.data_mut(|d| d.get_persisted::<bool>(egui::Id::new(AUTOFIX_ID))).unwrap_or(false),
            autofix_attempts: 0,
            autofix_pending: false,
            pinned: cc.egui_ctx.data_mut(|d| d.get_persisted::<Vec<String>>(egui::Id::new(PINNED_ID))).unwrap_or_default().into_iter().collect(),
            claude_key_set: get_claude_key().is_ok(),
            compiler_desc: find_compiler_desc(),
            show_profile_dlg: false,
            profile_err: String::new(),

            show_new_proj_dlg: false,
            new_proj_name: String::new(),
            new_proj_template: "blank".into(),
            confirm_delete_proj: None,
            show_logs_dlg: false,
            logs_cache: vec![],

            view_mode: ViewMode::Split,
            files: vec![],
            collapsed_dirs: BTreeSet::new(),
            current_file: String::new(),
            code_text: String::new(),
            saved_text: String::new(),
            undo_stack: vec![],
            redo_stack: vec![],
            last_edit_time: None,
            auto_save: true,
            cursor_range: None,
            jump_to_line: None,

            show_new_file_dlg: false,
            new_file_name: String::new(),
            rename_file_target: None,
            rename_file_input: String::new(),
            confirm_delete_file: None,

            compiling: false,
            compile_log: String::new(),
            compile_errors: vec![],
            show_logs_drawer: false,
            current_pdf_rel: None,
            pdf_zoom: 1.0,
            pdf_pages: vec![],
            pdf_rendering: false,
            pdf_err: String::new(),
            pdf_history: vec![],
            preview_image: None,

            show_files_drawer: true,
            show_ai_drawer: false,
            ai_tab: AiTab::Chat,
            models: vec![],
            selected_model_idx: 0,
            full_project_access: true,
            chat_messages: vec![],
            chat_input: String::new(),
            chat_streaming: false,
            chat_rx: None,
            chat_cancel: Arc::new(AtomicBool::new(false)),
            ai_edit_instruction: String::new(),
            ai_edit_busy: false,
            ai_diff_proposal: None,

            show_image_dlg: false,
            img_dlg_file: String::new(),
            img_dlg_width: "0.85\\linewidth".into(),
            img_dlg_caption: String::new(),
            img_dlg_pos: "htbp".into(),

            show_table_dlg: false,
            tbl_rows: 3,
            tbl_cols: 3,
            tbl_align: "l".into(),
            tbl_caption: String::new(),

            show_nn_dlg: false,
            nn_name: "network".into(),
            nn_spec: "conv     conv1  s=512 n=64 w=2 h=64 d=64 caption=Conv1\npool     pool1\nconv     conv2  s=128 n=64 w=2 h=32 d=32 caption=Conv2\nsoftmax  soft1  s=10 caption=Softmax gap=3\n".into(),
            nn_caption: "Neural network architecture.".into(),
            nn_busy: false,

            show_symbols_dlg: false,
            show_hw_dlg: false,
            hw_prompt: String::new(),
            hw_busy: false,
            hw_result: None,

            show_share_dlg: false,
            drive_syncing: false,
            live_sync: false,
            last_live_sync: Instant::now(),
            drive_folder_id: String::new(),
            drive_share_url: String::new(),
            drive_collab_email: String::new(),
            drive_collab_role: "writer".into(),
            drive_link_input: String::new(),
            drive_msg: String::new(),

            bg_tx,
            bg_rx,
        };
        app.refresh_models();
        // First run (or signed out): show the sign-in page until the user connects an account or skips it.
        let skipped = cc.egui_ctx.data_mut(|d| d.get_persisted::<bool>(egui::Id::new(LOGIN_SKIPPED))).unwrap_or(false);
        app.show_login = !skipped && !app.gemini_key_set && !app.claude_key_set;
        if let Some(p) = initial_project {
            app.open_project_by_name(&cc.egui_ctx, &p);
        }
        app
    }

    fn set_status(&mut self, msg: impl Into<String>, is_err: bool) {
        self.status_msg = msg.into();
        self.status_is_err = is_err;
    }

    fn refresh_models(&mut self) {
        let mut list = vec![];
        if self.gemini_key_set {
            for m in model_list("GEMINI_MODEL", GEMINI_MODELS) {
                list.push((format!("Gemini · {m}"), "gemini".to_string(), m));
            }
        }
        if self.claude_key_set {
            for m in model_list("CLAUDE_MODEL", CLAUDE_MODELS) {
                list.push((format!("Claude · {m}"), "claude".to_string(), m));
            }
        }
        for m in ollama_models() {
            list.push((format!("Local · {m}"), "ollama".to_string(), m));
        }
        if self.selected_model_idx >= list.len() {
            self.selected_model_idx = 0;
        }
        self.models = list;
    }

    fn open_project_by_name(&mut self, ctx: &egui::Context, name: &str) {
        match project_dir(name) {
            Ok(root) => {
                self.open_project = Some(name.to_string());
                self.open_project_root = Some(root.clone());
                self.chat_messages.clear();
                self.compile_errors.clear();
                self.compile_log.clear();
                self.ai_diff_proposal = None;
                self.preview_image = None;
                self.refresh_project_files();
                // Pick main.tex or first .tex
                let first_tex = self
                    .files
                    .iter()
                    .find(|f| f.eq_ignore_ascii_case("main.tex"))
                    .or_else(|| self.files.iter().find(|f| f.to_lowercase().ends_with(".tex")))
                    .cloned();
                if let Some(f) = first_tex {
                    self.open_file(ctx, &f);
                } else {
                    self.current_file.clear();
                    self.code_text.clear();
                    self.saved_text.clear();
                }
                // Check for existing PDF
                let default_pdf = self
                    .current_file
                    .strip_suffix(".tex")
                    .map(|s| format!("{s}.pdf"))
                    .unwrap_or_else(|| "main.pdf".into());
                if root.join(&default_pdf).is_file() {
                    self.load_pdf(ctx, &default_pdf);
                } else if let Some(first_pdf) = self.files.iter().find(|f| f.to_lowercase().ends_with(".pdf")).cloned() {
                    self.load_pdf(ctx, &first_pdf);
                } else {
                    self.current_pdf_rel = None;
                    self.pdf_pages.clear();
                }
                self.set_status(format!("Opened {name}"), false);
            }
            Err(e) => self.set_status(e, true),
        }
    }

    fn refresh_project_files(&mut self) {
        if let Some(root) = &self.open_project_root {
            let mut f = vec![];
            walk(root, root, &mut f);
            f.sort();
            self.files = f;
            self.pdf_history = history(root);
        }
    }

    fn open_file(&mut self, ctx: &egui::Context, rel: &str) {
        if self.code_text != self.saved_text && !self.current_file.is_empty() {
            self.save_current_file();
        }
        let Some(root) = &self.open_project_root else { return };
        let lower = rel.to_lowercase();
        if lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
            if let Ok(path) = safe_path(root, rel) {
                if let Ok(bytes) = fs::read(&path) {
                    if let Ok(img) = image::load_from_memory(&bytes) {
                        let rgba = img.to_rgba8();
                        let ci = ColorImage::from_rgba_unmultiplied(
                            [rgba.width() as usize, rgba.height() as usize],
                            rgba.as_raw(),
                        );
                        let tex = ctx.load_texture(format!("preview_{rel}"), ci, TextureOptions::LINEAR);
                        self.preview_image = Some((rel.to_string(), tex));
                        return;
                    }
                }
            }
        }
        if lower.ends_with(".pdf") {
            self.preview_image = None;
            self.load_pdf(ctx, rel);
            return;
        }
        if let Ok(path) = safe_path(root, rel) {
            match fs::read_to_string(&path) {
                Ok(text) => {
                    self.current_file = rel.to_string();
                    self.code_text = text.clone();
                    self.saved_text = text;
                    self.undo_stack.clear();
                    self.redo_stack.clear();
                    self.last_edit_time = None;
                    self.ai_diff_proposal = None;
                    self.set_status(format!("Opened {rel}"), false);
                }
                Err(e) => self.set_status(format!("Cannot read {rel}: {e}"), true),
            }
        }
    }

    fn save_current_file(&mut self) {
        if self.current_file.is_empty() {
            return;
        }
        let Some(root) = &self.open_project_root else { return };
        match safe_path(root, &self.current_file) {
            Ok(p) => {
                if let Some(parent) = p.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if p.exists() {
                    let bak = p.with_file_name(format!("{}.bak", p.file_name().unwrap().to_string_lossy()));
                    let _ = fs::copy(&p, bak);
                }
                match fs::write(&p, &self.code_text) {
                    Ok(_) => {
                        self.saved_text = self.code_text.clone();
                        self.last_edit_time = None;
                    }
                    Err(e) => self.set_status(format!("Save failed: {e}"), true),
                }
            }
            Err(e) => self.set_status(e, true),
        }
    }

    fn trigger_compile(&mut self) {
        if self.compiling {
            return;
        }
        self.save_current_file();
        let Some(root) = self.open_project_root.clone() else { return };
        let Some(proj) = self.open_project.clone() else { return };
        let target = if self.files.iter().any(|f| f == "main.tex") {
            "main.tex".to_string()
        } else if !self.current_file.is_empty() && self.current_file.ends_with(".tex") {
            self.current_file.clone()
        } else {
            self.files
                .iter()
                .find(|f| f.ends_with(".tex"))
                .cloned()
                .unwrap_or_else(|| "main.tex".into())
        };

        self.compiling = true;
        self.set_status(format!("Compiling {target}..."), false);
        let tx = self.bg_tx.clone();
        thread::spawn(move || {
            let res = compile_tex(&root, &target);
            match res {
                Ok(v) => {
                    let ok = v["ok"].as_bool().unwrap_or(false);
                    let pdf = v["pdf"].as_str().map(String::from);
                    let log = v["log"].as_str().unwrap_or("").to_string();
                    let ms = v["ms"].as_u64().unwrap_or(0);
                    let errors: Vec<CompileError> = v["errors"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|e| CompileError {
                            file: e["file"].as_str().unwrap_or("").to_string(),
                            line: e["line"].as_u64().unwrap_or(0) as usize,
                            msg: e["msg"].as_str().unwrap_or("").to_string(),
                        })
                        .collect();
                    let first_err = errors.first().map(|e| e.msg.as_str()).unwrap_or("");
                    log_event(
                        &proj,
                        "Compile",
                        ok,
                        &if ok {
                            format!("{ms} ms")
                        } else {
                            format!("{} error(s): {first_err}", errors.len())
                        },
                    );
                    let _ = tx.send(BgResult::Compiled { ok, pdf, log, errors, ms });
                }
                Err(e) => {
                    log_event(&proj, "Compile", false, &e);
                    let _ = tx.send(BgResult::Compiled {
                        ok: false,
                        pdf: None,
                        log: e.clone(),
                        errors: vec![CompileError { file: target, line: 0, msg: e }],
                        ms: 0,
                    });
                }
            }
        });
    }

    fn load_pdf(&mut self, ctx: &egui::Context, rel: &str) {
        let Some(root) = &self.open_project_root else { return };
        let Ok(path) = safe_path(root, rel) else { return };
        if !path.is_file() {
            return;
        }
        self.current_pdf_rel = Some(rel.to_string());
        self.pdf_rendering = true;
        self.pdf_err.clear();
        let rel_owned = rel.to_string();
        let tx = self.bg_tx.clone();
        let ctx_clone = ctx.clone();
        thread::spawn(move || {
            let pages = render_pdf_file(&path, 2.0, usize::MAX);
            let _ = tx.send(BgResult::PdfRendered { path: rel_owned, pages });
            ctx_clone.request_repaint();
        });
    }

    fn start_chat_send(&mut self, ctx: &egui::Context) {
        let q = self.chat_input.trim().to_string();
        if q.is_empty() || self.chat_streaming {
            return;
        }
        if self.models.is_empty() {
            self.show_profile_dlg = true;
            self.set_status("Please configure a Gemini or Claude API key first", true);
            return;
        }
        self.save_current_file();
        self.chat_input.clear();
        self.chat_messages.push(ChatMessage {
            role: "user".into(),
            text: q.clone(),
            thinking: String::new(),
        });
        self.chat_messages.push(ChatMessage {
            role: "assistant".into(),
            text: String::new(),
            thinking: String::new(),
        });

        let (_, provider, model) = self.models[self.selected_model_idx].clone();
        let mut msgs_json: Vec<Value> = self.chat_messages[..self.chat_messages.len() - 1]
            .iter()
            .map(|m| json!({"role": m.role, "text": m.text}))
            .collect();

        // Prepend current file + errors context to the latest user message
        if let Some(last) = msgs_json.last_mut() {
            let mut ctx_prefix = String::new();
            if self.full_project_access && !self.current_file.is_empty() {
                ctx_prefix.push_str(&format!("<file name=\"{}\">\n{}\n</file>\n\n", self.current_file, self.code_text));
            }
            if !self.compile_errors.is_empty() {
                ctx_prefix.push_str("<compile_errors>\n");
                for e in &self.compile_errors {
                    ctx_prefix.push_str(&format!("{}:{}: {}\n", e.file, e.line, e.msg));
                }
                ctx_prefix.push_str("</compile_errors>\n\n");
            }
            if !ctx_prefix.is_empty() {
                last["text"] = json!(format!("{ctx_prefix}User Question:\n{q}"));
            }
        }

        let mut body = json!({
            "provider": provider,
            "model": model,
            "question": q,
            "messages": msgs_json,
        });
        if self.full_project_access {
            if let Some(root) = &self.open_project_root {
                inject_project_context(root, &mut body);
            }
        }

        let (tx, rx) = mpsc::channel();
        self.chat_rx = Some(rx);
        self.chat_streaming = true;
        self.chat_cancel.store(false, Ordering::SeqCst);
        let cancel = self.chat_cancel.clone();
        let proj = self.open_project.clone().unwrap_or_default();
        let ctx_clone = ctx.clone();

        thread::spawn(move || {
            let mut emit = |v: Value| -> std::io::Result<()> {
                if cancel.load(Ordering::SeqCst) {
                    return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "Stopped"));
                }
                if let Some(t) = v["thinking"].as_str() {
                    let _ = tx.send(ChatEvent::Thinking(t.to_string()));
                }
                if let Some(t) = v["t"].as_str().or_else(|| v["text"].as_str()) {
                    let _ = tx.send(ChatEvent::Text(t.to_string()));
                }
                ctx_clone.request_repaint();
                Ok(())
            };
            let res = chat_stream(&body, &mut emit);
            log_event(&proj, "AI chat", res.is_ok(), &q);
            let _ = tx.send(ChatEvent::Done(res));
            ctx_clone.request_repaint();
        });
    }

    fn start_ai_edit(&mut self, ctx: &egui::Context, instruction: String, recompile_after: bool) {
        if self.ai_edit_busy || self.current_file.is_empty() {
            return;
        }
        if self.models.is_empty() {
            self.show_profile_dlg = true;
            return;
        }
        self.save_current_file();
        let (_, provider, model) = self.models[self.selected_model_idx].clone();
        let errors_json: Vec<Value> = self
            .compile_errors
            .iter()
            .map(|e| json!({"file": e.file, "line": e.line, "msg": e.msg}))
            .collect();
        let body = json!({
            "provider": provider,
            "model": model,
            "text": self.code_text,
            "instruction": instruction,
            "errors": errors_json,
        });
        self.ai_edit_busy = true;
        self.set_status("AI is editing...", false);
        let tx = self.bg_tx.clone();
        let proj = self.open_project.clone().unwrap_or_default();
        let ctx_clone = ctx.clone();
        thread::spawn(move || {
            let res = ask_ai(&body);
            log_event(&proj, "AI edit", res.is_ok(), &instruction);
            let _ = tx.send(BgResult::AiEdit(res, recompile_after));
            ctx_clone.request_repaint();
        });
    }

    /// After a failed compile: if the user allowed it, let the AI fix the file with the first error and recompile.
    fn maybe_autofix(&mut self, ctx: &egui::Context) {
        if !self.ai_autofix || self.ai_edit_busy || self.models.is_empty() || self.compile_errors.is_empty() {
            return;
        }
        if self.autofix_attempts >= AUTOFIX_MAX {
            self.set_status(format!("AI couldn't fix the errors in {AUTOFIX_MAX} tries; please check them yourself"), true);
            return;
        }
        // Fix the file the first error is in (the AI edits one file at a time).
        let file = self.compile_errors[0].file.clone();
        if !file.is_empty() && !self.current_file.ends_with(&file) {
            if let Some(m) = self.files.iter().find(|f| f.ends_with(&file)).cloned() {
                self.open_file(ctx, &m);
            }
        }
        self.autofix_attempts += 1;
        self.autofix_pending = true;
        self.start_ai_edit(ctx, "Fix all compile errors in this file. Change only what is needed to fix them.".into(), true);
        if !self.ai_edit_busy {
            self.autofix_pending = false; // start_ai_edit bailed out (e.g. no file open)
        }
    }

    fn autofix_toggle(&mut self, ui: &mut Ui) {
        let r = ui.checkbox(&mut self.ai_autofix, "Auto-fix errors with AI").on_hover_text(
            "When a compile fails, the AI fixes the file and recompiles (up to 3 tries) without asking. Fixes are saved; Ctrl+Z undoes them.",
        );
        if r.changed() {
            let v = self.ai_autofix;
            ui.data_mut(|d| d.insert_persisted(egui::Id::new(AUTOFIX_ID), v));
        }
    }

    fn start_help_me_write(&mut self, ctx: &egui::Context) {
        let prompt = self.hw_prompt.trim().to_string();
        if prompt.is_empty() || self.hw_busy {
            return;
        }
        if self.models.is_empty() {
            self.show_profile_dlg = true;
            return;
        }
        let (_, provider, model) = self.models[self.selected_model_idx].clone();
        let char_count = self.code_text.chars().count();
        let (s, e) = self.cursor_range.unwrap_or((char_count, char_count));
        let (s, e) = (s.min(e).min(char_count), s.max(e).min(char_count));
        let bs = crate::editor::char_to_byte(&self.code_text, s);
        let be = crate::editor::char_to_byte(&self.code_text, e);
        let before: String = self.code_text[..bs].chars().rev().take(6000).collect::<String>().chars().rev().collect();
        let sel = &self.code_text[bs..be];
        let after: String = self.code_text[be..].chars().take(2000).collect();

        let context_msg = if sel.is_empty() {
            format!("Request: {prompt}\n\n<before>\n{before}\n</before>\n<cursor/>\n<after>\n{after}\n</after>")
        } else {
            format!("Request: {prompt}\n\n<before>\n{before}\n</before>\n<selection>\n{sel}\n</selection>\n<after>\n{after}\n</after>")
        };
        let body = json!({
            "provider": provider,
            "model": model,
            "system": WRITE_SYSTEM,
            "text": context_msg,
            "instruction": prompt,
        });
        self.hw_busy = true;
        self.hw_result = None;
        let tx = self.bg_tx.clone();
        let ctx_clone = ctx.clone();
        thread::spawn(move || {
            let res = ask_ai(&body);
            let _ = tx.send(BgResult::HelpWrite(res));
            ctx_clone.request_repaint();
        });
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        // Poll streaming chat
        if let Some(rx) = &self.chat_rx {
            let mut finished = false;
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    ChatEvent::Thinking(t) => {
                        if let Some(last) = self.chat_messages.last_mut() {
                            last.thinking.push_str(&t);
                        }
                    }
                    ChatEvent::Text(t) => {
                        if let Some(last) = self.chat_messages.last_mut() {
                            last.text.push_str(&t);
                        }
                    }
                    ChatEvent::Done(res) => {
                        finished = true;
                        if let Err(e) = res {
                            if e != "Stopped" {
                                if let Some(last) = self.chat_messages.last_mut() {
                                    if last.text.is_empty() {
                                        last.text = format!("Error: {e}");
                                    } else {
                                        last.text.push_str(&format!("\n\n[Error: {e}]"));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if finished {
                self.chat_streaming = false;
                self.chat_rx = None;
            }
        }

        // Poll background jobs
        while let Ok(res) = self.bg_rx.try_recv() {
            match res {
                BgResult::Compiled { ok, pdf, log, errors, ms } => {
                    self.compiling = false;
                    self.compile_log = log;
                    self.compile_errors = errors;
                    self.refresh_project_files();
                    if ok {
                        self.autofix_attempts = 0;
                        self.set_status(format!("Compiled in {ms} ms"), false);
                        if let Some(p) = pdf {
                            self.preview_image = None;
                            self.load_pdf(ctx, &p);
                        }
                    } else {
                        self.show_logs_drawer = true;
                        self.set_status(
                            format!("Compile failed ({} error(s))", self.compile_errors.len()),
                            true,
                        );
                        self.maybe_autofix(ctx);
                    }
                }
                BgResult::PdfRendered { path, pages } => {
                    self.pdf_rendering = false;
                    match pages {
                        Ok(list) => {
                            self.pdf_pages = list
                                .into_iter()
                                .enumerate()
                                .map(|(i, rp)| {
                                    let tex = ctx.load_texture(
                                        format!("pdf_page_{path}_{i}"),
                                        rp.image,
                                        TextureOptions::LINEAR,
                                    );
                                    (tex, rp.pt_width, rp.pt_height)
                                })
                                .collect();
                        }
                        Err(e) => {
                            self.pdf_err = e;
                        }
                    }
                }
                BgResult::AiEdit(res, recompile_after) => {
                    self.ai_edit_busy = false;
                    match res {
                        Ok(new_text) => {
                            let cleaned = strip_fences(&new_text);
                            let auto = std::mem::take(&mut self.autofix_pending);
                            // Auto-apply only fixes that keep most of the file; anything drastic goes to review.
                            if auto && cleaned.trim().len() * 2 >= self.code_text.trim().len() {
                                self.push_undo();
                                self.code_text = cleaned;
                                self.save_current_file();
                                self.set_status(format!("AI fixed errors (try {}/{AUTOFIX_MAX}), recompiling. Ctrl+Z undoes it.", self.autofix_attempts), false);
                                self.trigger_compile();
                            } else {
                                if auto {
                                    self.set_status("AI's fix removes a lot of the file, so it needs your review", true);
                                } else {
                                    self.set_status("AI proposed changes — review diff", false);
                                }
                                self.ai_diff_proposal = Some((self.code_text.clone(), cleaned, recompile_after));
                            }
                        }
                        Err(e) => {
                            self.autofix_pending = false;
                            self.set_status(format!("AI error: {e}"), true);
                        }
                    }
                }
                BgResult::HelpWrite(res) => {
                    self.hw_busy = false;
                    match res {
                        Ok(txt) => self.hw_result = Some(strip_fences(&txt)),
                        Err(e) => self.set_status(format!("Help me write failed: {e}"), true),
                    }
                }
                BgResult::DiagramBuilt(res) => {
                    self.nn_busy = false;
                    match res {
                        Ok((pdf_rel, caption)) => {
                            ensure_package(&mut self.code_text, "graphicx");
                            let cap = if caption.trim().is_empty() {
                                String::new()
                            } else {
                                format!("\n  \\caption{{{}}}", caption.trim())
                            };
                            let snippet = format!(
                                "\\begin{{figure}}[htbp]\n  \\centering\n  \\includegraphics[width=\\linewidth]{{{pdf_rel}}}{cap}\n\\end{{figure}}\n"
                            );
                            let c = insert_at_cursor(&mut self.code_text, self.cursor_range, &snippet);
                            self.cursor_range = Some((c, c));
                            self.refresh_project_files();
                            self.show_nn_dlg = false;
                            self.set_status(format!("Inserted diagram {pdf_rel}"), false);
                        }
                        Err(e) => self.set_status(format!("NN Diagram error: {e}"), true),
                    }
                }
                BgResult::Thumb { key, stamp, image } => {
                    self.thumb_jobs = self.thumb_jobs.saturating_sub(1);
                    if let (Some(entry), Some(img)) = (self.thumbs.get_mut(&key), image) {
                        if entry.0 == stamp {
                            entry.1 = Some(ctx.load_texture(format!("thumb_{key}"), img, TextureOptions::LINEAR));
                        }
                    }
                }
                BgResult::ClipboardKey(kind, key) => {
                    if kind == self.clipboard_watch_kind {
                        self.clipboard_watch_until = None;
                    }
                    let saved = match (kind, key.is_empty()) {
                        (_, true) => {
                            if kind == self.clipboard_watch_kind {
                                self.profile_err = "No key was copied within 90 seconds. Try again, or paste it in AI > Settings.".into();
                            }
                            None
                        }
                        ("claude", false) => Some(save_claude_key(&key).map(|_| self.claude_key_set = true)),
                        _ => Some(save_gemini_key(&key).map(|_| {
                            self.gemini_key_input = key.clone();
                            self.gemini_key_set = true;
                        })),
                    };
                    match saved {
                        Some(Ok(())) => {
                            self.profile_err.clear();
                            self.refresh_models();
                            self.set_status(if kind == "claude" { "Claude connected" } else { "Gemini connected" }, false);
                        }
                        Some(Err(e)) => self.profile_err = e,
                        None => {}
                    }
                }
                BgResult::GoogleLogin(res) => {
                    self.google_login_busy = false;
                    match res {
                        Ok(email) => {
                            self.google_signed_in_email = Some(email.clone());
                            self.gemini_key_set = true;
                            self.profile_err.clear();
                            self.refresh_models();
                            self.set_status(format!("Signed in to Gemini as {email}"), false);
                        }
                        Err(e) => {
                            self.profile_err = e;
                        }
                    }
                }
                BgResult::DriveSynced(res) => {
                    self.drive_syncing = false;
                    self.last_live_sync = Instant::now();
                    match res {
                        Ok(rep) => {
                            self.drive_folder_id = rep.folder_id;
                            self.drive_share_url = rep.share_url;
                            self.refresh_project_files();
                            if !self.current_file.is_empty() {
                                if let Some(root) = &self.open_project_root {
                                    if let Ok(p) = safe_path(root, &self.current_file) {
                                        if let Ok(txt) = fs::read_to_string(&p) {
                                            if txt != self.code_text && self.code_text == self.saved_text {
                                                self.code_text = txt.clone();
                                                self.saved_text = txt;
                                            }
                                        }
                                    }
                                }
                            }
                            let msg = format!("Drive synced ({} up, {} down)", rep.uploaded, rep.downloaded);
                            self.drive_msg = msg.clone();
                            self.set_status(msg, false);
                        }
                        Err(e) => {
                            self.drive_msg = e.clone();
                            self.set_status(e, true);
                        }
                    }
                }
                BgResult::DriveShared(res) => match res {
                    Ok(url) => {
                        self.drive_share_url = url.clone();
                        self.drive_msg = format!("Shared! Link: {url}");
                        self.set_status("Shared project on Google Drive", false);
                    }
                    Err(e) => {
                        self.drive_msg = e.clone();
                        self.set_status(e, true);
                    }
                },
            }
        }
    }

    fn trigger_drive_sync(&mut self, ctx: &egui::Context) {
        if self.drive_syncing {
            return;
        }
        let Some(root) = self.open_project_root.clone() else { return };
        let Some(proj) = self.open_project.clone() else { return };
        self.save_current_file();
        self.drive_syncing = true;
        let tx = self.bg_tx.clone();
        let ctx_c = ctx.clone();
        thread::spawn(move || {
            let res = sync_project_with_drive(&root, &proj);
            let _ = tx.send(BgResult::DriveSynced(res));
            ctx_c.request_repaint();
        });
    }

    fn push_undo(&mut self) {
        if self.undo_stack.last().map(|s| s.as_str()) != Some(self.code_text.as_str()) {
            self.undo_stack.push(self.code_text.clone());
            if self.undo_stack.len() > 100 {
                self.undo_stack.remove(0);
            }
            self.redo_stack.clear();
        }
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.code_text.clone());
            self.code_text = prev;
            self.last_edit_time = Some(Instant::now());
            self.last_typing = None;
            self.set_status("Undone", false);
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.code_text.clone());
            self.code_text = next;
            self.last_edit_time = Some(Instant::now());
            self.last_typing = None;
            self.set_status("Redone", false);
        }
    }

    fn apply_file_edit(&mut self, rel_path: &str, new_content: &str) -> Result<(), String> {
        let root = self.open_project_root.as_ref().ok_or("No project open")?;
        let p = safe_path(root, rel_path)?;
        if let Some(parent) = p.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if p.exists() {
            let bak = p.with_file_name(format!("{}.bak", p.file_name().unwrap().to_string_lossy()));
            let _ = fs::copy(&p, bak);
        }
        fs::write(&p, new_content).map_err(|e| e.to_string())?;
        if self.current_file == rel_path {
            self.code_text = new_content.to_string();
            self.saved_text = new_content.to_string();
            self.last_edit_time = None;
        }
        if let Some(proj) = &self.open_project {
            log_event(proj, "AI edit file", true, rel_path);
        }
        self.refresh_project_files();
        Ok(())
    }
}

impl eframe::App for TexCraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_background(&ctx);

        // Auto-save debounce (1.5s after typing stops)
        if self.open_project.is_some() && self.auto_save && self.code_text != self.saved_text {
            if let Some(t) = self.last_edit_time {
                if t.elapsed().as_secs_f32() >= 1.5 {
                    self.save_current_file();
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(300));
                }
            }
        }

        // MS Word-style keyboard shortcuts when a project is open. Keys are consumed so the text box
        // doesn't also react (its own Ctrl+Z used to fight the app's undo stack).
        if self.open_project.is_some() {
            use egui::Modifiers as M;
            let (cs, ca) = (M::COMMAND | M::SHIFT, M::COMMAND | M::ALT);
            let hit = |m: M, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
            if hit(M::COMMAND, Key::S) {
                self.save_current_file();
                self.set_status("Saved", false);
            }
            if hit(M::COMMAND, Key::Enter) {
                self.trigger_compile();
            }
            // Shift/Alt variants first: consume_key(COMMAND, k) also matches Ctrl+Shift+k.
            if hit(cs, Key::Z) || hit(M::COMMAND, Key::Y) {
                self.redo();
            }
            if hit(M::COMMAND, Key::Z) {
                self.undo();
            }
            if hit(cs, Key::M) {
                self.wrap_pkg("amsmath", "\n\\begin{equation}\n", "\n\\end{equation}\n");
            }
            if hit(M::COMMAND, Key::M) {
                self.wrap("$", "$");
            }
            if hit(cs, Key::L) {
                self.show_ai_drawer = !self.show_ai_drawer;
            }
            if hit(M::COMMAND, Key::L) {
                self.wrap("\n\\begin{flushleft}\n", "\n\\end{flushleft}\n");
            }
            if hit(cs, Key::K) {
                self.wrap("\\textsc{", "}");
            }
            if hit(M::COMMAND, Key::K) {
                self.show_hw_dlg = true;
                self.hw_result = None;
            }
            if hit(cs, Key::Equals) || hit(cs, Key::Plus) {
                self.wrap("\\textsuperscript{", "}");
            }
            if hit(M::COMMAND, Key::Equals) {
                self.wrap("\\textsubscript{", "}");
            }
            for (k, before) in [(Key::B, "\\textbf{"), (Key::I, "\\textit{"), (Key::U, "\\underline{")] {
                if hit(M::COMMAND, k) {
                    self.wrap(before, "}");
                }
            }
            for (k, cmd) in [(Key::Num1, "section"), (Key::Num2, "subsection"), (Key::Num3, "subsubsection")] {
                if hit(ca, k) || hit(M::COMMAND, k) {
                    self.wrap(&format!("\\{cmd}{{"), "}");
                }
            }
            if hit(M::COMMAND, Key::E) {
                self.wrap("\n\\begin{center}\n", "\n\\end{center}\n");
            }
            if hit(M::COMMAND, Key::R) {
                self.wrap("\n\\begin{flushright}\n", "\n\\end{flushright}\n");
            }
            if hit(M::COMMAND, Key::Slash) {
                self.push_undo();
                toggle_comment(&mut self.code_text, self.cursor_range);
                self.last_edit_time = Some(Instant::now());
            }
        }

        // Live Google Drive auto-sync every 12s when enabled
        if self.open_project.is_some() && self.live_sync && !self.drive_syncing && self.last_live_sync.elapsed().as_secs() >= 12 {
            self.trigger_drive_sync(&ctx);
        }

        let pal = self.theme.palette();

        // Merged title + menu bar (VS Code style; the window is frameless)
        egui::Panel::top("top_bar")
            .frame(pal.glass_panel_frame(Margin { left: 10, right: 0, top: 0, bottom: 0 }))
            .exact_size(34.0)
            .show(ui, |ui| self.ui_top_bar(ui, &ctx, pal));

        // Bottom Status Bar (Glass)
        egui::Panel::bottom("status_bar")
            .frame(pal.glass_panel_frame(Margin::symmetric(12, 3)))
            .show(ui, |ui| self.ui_status_bar(ui, pal));

        if self.show_login {
            egui::CentralPanel::default()
                .frame(Frame::new().fill(pal.bg).inner_margin(Margin::same(24)))
                .show(ui, |ui| {
                    paint_glass_backdrop(ui, pal);
                    self.ui_login(ui, &ctx, pal);
                });
        } else if self.open_project.is_none() {
            egui::CentralPanel::default()
                .frame(Frame::new().fill(pal.bg).inner_margin(Margin::same(24)))
                .show(ui, |ui| {
                    paint_glass_backdrop(ui, pal);
                    self.ui_home(ui, &ctx, pal);
                });
        } else {
            self.ui_workspace(ui, &ctx, pal);
        }

        self.ui_dialogs(&ctx, pal);
        window_edges(&ctx, pal);
    }
}

impl TexCraftApp {
    fn ui_top_bar(&mut self, ui: &mut Ui, ctx: &egui::Context, pal: ThemePalette) {
        let bar = ui.max_rect();
        ui.horizontal(|ui| {
            if let Some(tex) = &self.logo_tex {
                ui.add(egui::Image::new(tex).fit_to_exact_size(Vec2::splat(18.0 * self.icon_scale)).corner_radius(CornerRadius::same(4)));
            }
            ui.add_space(2.0);

            let mut menus_end = bar.left();
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if self.open_project.is_some() {
                        if ui.button("Projects Home").clicked() {
                            self.save_current_file();
                            self.open_project = None;
                            self.open_project_root = None;
                            self.projects = list_projects();
                            ui.close();
                        }
                        if ui.button("New File...").clicked() {
                            self.new_file_name.clear();
                            self.show_new_file_dlg = true;
                            ui.close();
                        }
                        if ui.button("Save  (Ctrl+S)").clicked() {
                            self.save_current_file();
                            self.set_status("Saved", false);
                            ui.close();
                        }
                        if let Some(pdf_rel) = self.current_pdf_rel.clone() {
                            if ui.button("Export PDF...").clicked() {
                                if let Some(root) = &self.open_project_root {
                                    if let Ok(src) = safe_path(root, &pdf_rel) {
                                        let default_name = format!("{}.pdf", self.open_project.as_deref().unwrap_or("paper"));
                                        if let Some(dest) = rfd::FileDialog::new().set_file_name(&default_name).add_filter("PDF", &["pdf"]).save_file() {
                                            match fs::copy(&src, &dest) {
                                                Ok(_) => self.set_status(format!("Exported PDF to {}", dest.display()), false),
                                                Err(e) => self.set_status(format!("Export failed: {e}"), true),
                                            }
                                        }
                                    }
                                }
                                ui.close();
                            }
                        }
                        ui.separator();
                    }
                    if ui.button("New Project...").clicked() {
                        self.new_proj_name.clear();
                        self.new_proj_template = "blank".into();
                        self.show_new_proj_dlg = true;
                        ui.close();
                    }
                    if ui.button("Upload ZIP...").clicked() {
                        if let Some(path) = rfd::FileDialog::new().add_filter("ZIP Archive", &["zip"]).pick_file() {
                            if let Ok(bytes) = fs::read(&path) {
                                let fname = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                                match import_zip(&fname, &bytes) {
                                    Ok(n) => {
                                        log_event(&n, "Upload ZIP", true, &fname);
                                        self.projects = list_projects();
                                        self.open_project_by_name(ctx, &n);
                                    }
                                    Err(e) => self.set_status(e, true),
                                }
                            }
                        }
                        ui.close();
                    }
                });

                ui.menu_button("Edit", |ui| {
                    if self.open_project.is_some() {
                        if ui.button("Bold  (\\textbf)").clicked() {
                            let c = wrap_selection(&mut self.code_text, self.cursor_range, "\\textbf{", "}");
                            self.cursor_range = Some((c, c));
                            ui.close();
                        }
                        if ui.button("Italic  (\\textit)").clicked() {
                            let c = wrap_selection(&mut self.code_text, self.cursor_range, "\\textit{", "}");
                            self.cursor_range = Some((c, c));
                            ui.close();
                        }
                        if ui.button("Toggle Comment").clicked() {
                            toggle_comment(&mut self.code_text, self.cursor_range);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Symbols Picker").clicked() {
                            self.show_symbols_dlg = true;
                            ui.close();
                        }
                        if ui.button("Help me write  (Ctrl+K)").clicked() {
                            self.show_hw_dlg = true;
                            self.hw_result = None;
                            ui.close();
                        }
                    } else {
                        ui.label(RichText::new("Open a project to edit").color(pal.muted));
                    }
                });

                ui.menu_button("View", |ui| {
                    if self.open_project.is_some() {
                        if ui.selectable_label(self.view_mode == ViewMode::Code, "Code Only").clicked() {
                            self.view_mode = ViewMode::Code;
                            ui.close();
                        }
                        if ui.selectable_label(self.view_mode == ViewMode::Split, "Split (Code + PDF)").clicked() {
                            self.view_mode = ViewMode::Split;
                            ui.close();
                        }
                        if ui.selectable_label(self.view_mode == ViewMode::Pdf, "PDF Only").clicked() {
                            self.view_mode = ViewMode::Pdf;
                            ui.close();
                        }
                        ui.separator();
                        if ui.checkbox(&mut self.show_ai_drawer, "AI Assistant Panel  (Ctrl+Shift+L)").clicked() {
                            ui.close();
                        }
                        if ui.checkbox(&mut self.show_logs_drawer, "Compile Logs Drawer").clicked() {
                            ui.close();
                        }
                        ui.separator();
                    }
                    ui.menu_button("Theme", |ui| {
                        for t in ThemeKind::ALL {
                            if ui.selectable_label(self.theme == t, t.label()).clicked() {
                                self.theme = t;
                                t.apply(ctx);
                                let _ = save_profile(&json!({
                                    "name": self.prof_name,
                                    "affiliation": self.prof_affil,
                                    "email": self.prof_email,
                                    "theme": t.id(),
                                }));
                                ui.close();
                            }
                        }
                        ui.separator();
                        ui.menu_button("Text size", |ui| {
                            for (lbl, v) in TEXT_SIZES {
                                if ui.selectable_label((self.text_scale - v).abs() < 0.01, lbl).clicked() {
                                    self.text_scale = v;
                                    install_fonts(ctx, v);
                                    ui.data_mut(|d| d.insert_persisted(egui::Id::new(TEXT_SCALE_ID), v));
                                    ui.close();
                                }
                            }
                        });
                        ui.menu_button("Icon size", |ui| {
                            for (lbl, v) in ICON_SIZES {
                                if ui.selectable_label((self.icon_scale - v).abs() < 0.01, lbl).clicked() {
                                    self.icon_scale = v;
                                    ui.data_mut(|d| d.insert_persisted(egui::Id::new(ICON_SCALE_ID), v));
                                    ui.close();
                                }
                            }
                        });
                    });
                });

                if self.open_project.is_some() {
                    ui.menu_button("Compile", |ui| {
                        if ui.button("Recompile PDF  (Ctrl+Enter)").clicked() {
                            self.trigger_compile();
                            ui.close();
                        }
                        if ui.button("Insert NN Diagram...").clicked() {
                            self.show_nn_dlg = true;
                            ui.close();
                        }
                    });

                    ui.menu_button("Cloud", |ui| {
                        if ui.button("Sync with Google Drive Now").clicked() {
                            self.trigger_drive_sync(ctx);
                            ui.close();
                        }
                        ui.checkbox(&mut self.live_sync, "Live Auto-Sync (12s)");
                        if ui.button("Share / Connect Drive Folder...").clicked() {
                            if let Some(root) = &self.open_project_root {
                                let meta = load_drive_meta(root);
                                self.drive_folder_id = meta["folder_id"].as_str().unwrap_or("").to_string();
                                if !self.drive_folder_id.is_empty() {
                                    self.drive_share_url = format!("https://drive.google.com/drive/folders/{}", self.drive_folder_id);
                                }
                            }
                            self.show_share_dlg = true;
                            ui.close();
                        }
                    });
                }

                ui.menu_button("AI", |ui| {
                    if ui.button("Sign in...").clicked() {
                        self.profile_err.clear();
                        self.show_login = true;
                        ui.close();
                    }
                    self.autofix_toggle(ui);
                    if ui.button("Google Login & API Settings...").clicked() {
                        self.profile_err.clear();
                        self.show_profile_dlg = true;
                        ui.close();
                    }
                    if self.open_project.is_some() {
                        if ui.button("Toggle AI Panel  (Ctrl+Shift+L)").clicked() {
                            self.show_ai_drawer = !self.show_ai_drawer;
                            ui.close();
                        }
                    }
                });

                ui.menu_button("Help", |ui| {
                    if ui.button("Activity Logs").clicked() {
                        self.logs_cache = read_log();
                        self.show_logs_dlg = true;
                        ui.close();
                    }
                    if ui.button("Settings & Key").clicked() {
                        self.profile_err.clear();
                        self.show_profile_dlg = true;
                        ui.close();
                    }
                });
                // MenuBar reserves the full row width, so take the cursor after the last menu instead.
                menus_end = ui.cursor().min.x;
            });

            // Right side: window controls, then project quick actions; the gap left over is the drag area.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if window_button(ui, WinBtn::Close, pal).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if window_button(ui, if maximized { WinBtn::Restore } else { WinBtn::Max }, pal).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                    }
                    if window_button(ui, WinBtn::Min, pal).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                });
                ui.add_space(6.0);

                if self.open_project.is_some() {
                    let recompile_lbl = if self.compiling { "Compiling..." } else { "Recompile" };
                    let recompile_btn = egui::Button::new(RichText::new(recompile_lbl).color(Color32::WHITE).size(12.0))
                        .fill(pal.green)
                        .stroke(Stroke::NONE);
                    if ui.add_enabled(!self.compiling, recompile_btn).on_hover_text("Ctrl+Enter").clicked() {
                        self.trigger_compile();
                    }
                    if ui.add(egui::Button::new(RichText::new(if self.drive_syncing { "Syncing..." } else { "Sync" }).size(12.0)).frame(false)).clicked() {
                        self.trigger_drive_sync(ctx);
                    }
                    ui.separator();
                    for (m, lbl) in [(ViewMode::Pdf, "PDF"), (ViewMode::Split, "Split"), (ViewMode::Code, "Code")] {
                        if ui.selectable_label(self.view_mode == m, RichText::new(lbl).size(12.0)).clicked() {
                            self.view_mode = m;
                        }
                    }
                    ui.separator();
                    if ui.selectable_label(self.show_ai_drawer, RichText::new("AI").size(12.0)).on_hover_text("Ctrl+Shift+L").clicked() {
                        self.show_ai_drawer = !self.show_ai_drawer;
                    }
                    if ui.selectable_label(self.show_files_drawer, RichText::new("Files").size(12.0)).clicked() {
                        self.show_files_drawer = !self.show_files_drawer;
                    }
                }

                // Leftover space: drag to move, double-click to maximize, centred window title.
                let rect = egui::Rect::from_x_y_ranges(menus_end + 4.0..=ui.min_rect().left() - 4.0, bar.y_range());
                if rect.width() > 8.0 {
                    let resp = ui.interact(rect, egui::Id::new("title_drag"), Sense::click_and_drag());
                    if resp.double_clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                    } else if resp.drag_started_by(egui::PointerButton::Primary) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                    let title = match &self.open_project {
                        Some(p) => format!("{p} — inkTeX"),
                        None => "inkTeX".to_string(),
                    };
                    ui.painter().with_clip_rect(rect).text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        title,
                        FontId::proportional(12.5),
                        pal.muted,
                    );
                }
            });
        });
    }

    /// Google OAuth in the browser, on a worker thread; the result comes back as BgResult::GoogleLogin.
    fn start_google_login(&mut self, ctx: &egui::Context, cid: String, csec: String) {
        self.profile_err.clear();
        self.google_login_busy = true;
        let (tx, ctx) = (self.bg_tx.clone(), ctx.clone());
        thread::spawn(move || {
            let res = start_google_oauth_login(&cid, &csec);
            let _ = tx.send(BgResult::GoogleLogin(res));
            ctx.request_repaint();
        });
    }

    /// Watch the clipboard (off the UI thread) for up to 90s for a freshly copied key of `kind`.
    fn start_clipboard_watch(&mut self, ctx: &egui::Context, kind: &'static str) {
        self.profile_err.clear();
        if self.clipboard_watch_until.is_some_and(|t| Instant::now() < t) && self.clipboard_watch_kind == kind {
            return; // already watching for this provider
        }
        self.clipboard_watch_kind = kind;
        self.clipboard_watch_until = Some(Instant::now() + std::time::Duration::from_secs(90));
        let (tx, ctx) = (self.bg_tx.clone(), ctx.clone());
        thread::spawn(move || {
            let end = Instant::now() + std::time::Duration::from_secs(90);
            let mut key = String::new();
            while Instant::now() < end {
                let clip = read_clipboard();
                if key_kind(&clip) == Some(kind) {
                    key = clip;
                    break;
                }
                thread::sleep(std::time::Duration::from_secs(1));
            }
            let _ = tx.send(BgResult::ClipboardKey(kind, key));
            ctx.request_repaint();
        });
    }

    /// Sign-in page: one row per provider, then Continue (or skip, which is remembered).
    fn ui_login(&mut self, ui: &mut Ui, ctx: &egui::Context, pal: ThemePalette) {
        let watching = |k: &str| self.clipboard_watch_until.is_some_and(|t| Instant::now() < t) && self.clipboard_watch_kind == k;
        let (watch_google, watch_claude) = (watching("gemini"), watching("claude"));
        let card_w = 380.0;
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.5 - 250.0).max(12.0));
            pal.glass_panel_frame(Margin::symmetric(30, 26))
                .corner_radius(CornerRadius::same(14))
                .shadow(pal.glass_shadow())
                .show(ui, |ui| {
                    ui.set_width(card_w);
                    ui.vertical_centered(|ui| {
                        if let Some(tex) = &self.logo_tex {
                            ui.add(egui::Image::new(tex).fit_to_exact_size(Vec2::splat(52.0)).corner_radius(CornerRadius::same(12)));
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new("Welcome to inkTeX").size(22.0).color(pal.text));
                        ui.label(RichText::new("Sign in to turn on AI writing, error fixing and Google Drive sync.").size(12.5).color(pal.muted));
                        ui.add_space(20.0);

                        // Google
                        let g_sub = if self.gemini_key_set {
                            self.google_signed_in_email.clone().unwrap_or_else(|| "Connected".into())
                        } else if self.google_login_busy {
                            "Finish in your browser...".into()
                        } else if watch_google {
                            "Waiting for copied key...".into()
                        } else {
                            String::new()
                        };
                        let g = provider_button(ui, pal, card_w, "G", Color32::from_rgb(66, 133, 244), "Continue with Google", &g_sub, self.gemini_key_set);
                        if g.clicked() && !self.gemini_key_set && !self.google_login_busy {
                            let configured = (!self.oauth_client_id.trim().is_empty()).then(|| (self.oauth_client_id.clone(), self.oauth_client_secret.clone()));
                            match builtin_google_client().or(configured) {
                                Some((cid, csec)) => self.start_google_login(ctx, cid, csec),
                                None => {
                                    // No OAuth client in this build: fall back to AI Studio key + clipboard detection.
                                    open_url("https://aistudio.google.com/apikey");
                                    self.start_clipboard_watch(ctx, "gemini");
                                }
                            }
                        }
                        if watch_google {
                            ui.label(RichText::new("In Google AI Studio, create an API key and copy it. inkTeX picks it up automatically.").size(11.5).color(pal.muted));
                        }
                        ui.add_space(10.0);

                        // Claude (Anthropic has no third-party sign-in, so this is a guided API key)
                        let c_sub = if self.claude_key_set {
                            "Connected".to_string()
                        } else if watch_claude {
                            "Waiting for copied key...".into()
                        } else {
                            String::new()
                        };
                        let c = provider_button(ui, pal, card_w, "C", Color32::from_rgb(217, 119, 87), "Continue with Claude", &c_sub, self.claude_key_set);
                        if c.clicked() && !self.claude_key_set {
                            open_url("https://console.anthropic.com/settings/keys");
                            self.start_clipboard_watch(ctx, "claude");
                        }
                        if watch_claude {
                            ui.label(RichText::new("In the Claude Console, sign in, create a key and copy it. inkTeX picks it up automatically.").size(11.5).color(pal.muted));
                        }

                        if !self.profile_err.is_empty() {
                            ui.add_space(8.0);
                            ui.label(RichText::new(&self.profile_err).size(12.0).color(pal.red));
                        }
                        ui.add_space(20.0);

                        if self.gemini_key_set || self.claude_key_set {
                            let btn = egui::Button::new(RichText::new("Continue").color(Color32::WHITE)).fill(pal.accent).stroke(Stroke::NONE).min_size(Vec2::new(card_w, 36.0));
                            if ui.add(btn).clicked() {
                                self.show_login = false;
                            }
                        } else if ui.add(egui::Button::new(RichText::new("Continue without signing in").size(12.5).color(pal.muted)).frame(false)).clicked() {
                            self.show_login = false;
                            ui.data_mut(|d| d.insert_persisted(egui::Id::new(LOGIN_SKIPPED), true));
                        }
                        ui.add_space(6.0);
                        ui.label(RichText::new("Keys and sign-in tokens stay on this computer.").size(11.0).color(pal.muted));
                    });
                });
        });
    }

    fn ui_status_bar(&mut self, ui: &mut Ui, pal: ThemePalette) {
        ui.horizontal(|ui| {
            let color = if self.status_is_err { pal.red } else { pal.muted };
            ui.label(RichText::new(&self.status_msg).size(12.0).color(color));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.hyperlink_to(
                    RichText::new("github.com/ahammedzihadkhan").size(12.0).color(pal.accent),
                    "https://github.com/ahammedzihadkhan",
                );
                ui.label(RichText::new("Made by Ahammed Zihad Khan ·").size(12.0).color(pal.muted));
                ui.separator();
                if self.open_project.is_some() && !self.current_file.is_empty() {
                    let char_idx = self.cursor_range.map(|(a, _)| a).unwrap_or(0);
                    let (ln, col) = line_col_of_char(&self.code_text, char_idx);
                    ui.label(RichText::new(format!("Ln {ln}, Col {col}")).size(12.0).color(pal.muted));
                    ui.separator();
                    let dirty = self.code_text != self.saved_text;
                    let save_lbl = if dirty { "Unsaved" } else { "Saved" };
                    let save_col = if dirty { pal.amber } else { pal.green };
                    ui.label(RichText::new(save_lbl).size(12.0).color(save_col));
                } else {
                    ui.label(RichText::new(&self.compiler_desc).size(12.0).color(pal.muted));
                }
            });
        });
    }

    fn ui_home(&mut self, ui: &mut Ui, ctx: &egui::Context, pal: ThemePalette) {
        // View/sort prefs live in egui's persisted memory, so they survive restarts like Explorer's.
        let prefs_id = egui::Id::new("home_view_prefs");
        let (mut tiles, mut sort_key, mut asc): (bool, u8, bool) =
            ui.data_mut(|d| *d.get_persisted_mut_or(prefs_id, (false, SORT_DATE, false)));
        let mut to_open: Option<String> = None;
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::F)) {
            ui.memory_mut(|m| m.request_focus(egui::Id::new(("search_box", HOME_SEARCH_HINT))));
        }

        ui.vertical(|ui| {
            // Hero header: big logo + name, primary actions on the right.
            ui.horizontal(|ui| {
                if let Some(tex) = &self.logo_tex {
                    ui.add(egui::Image::new(tex).fit_to_exact_size(Vec2::splat(76.0)).corner_radius(CornerRadius::same(18)));
                    ui.add_space(14.0);
                }
                ui.vertical(|ui| {
                    ui.add_space(10.0);
                    ui.label(RichText::new("inkTeX").size(30.0).strong().color(pal.text));
                    let pinned = self.projects.iter().filter(|p| self.pinned.contains(p["name"].as_str().unwrap_or(""))).count();
                    let mut sub = format!("{} LaTeX projects", self.projects.len());
                    if pinned > 0 {
                        sub.push_str(&format!("  ·  {pinned} pinned"));
                    }
                    ui.label(RichText::new(sub).size(12.5).color(pal.muted));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let new_btn = egui::Button::new(RichText::new("+ New Project").color(Color32::WHITE)).fill(pal.accent).stroke(Stroke::NONE).min_size(Vec2::new(0.0, 34.0));
                    if ui.add(new_btn).clicked() {
                        self.new_proj_name.clear();
                        self.new_proj_template = "blank".into();
                        self.show_new_proj_dlg = true;
                    }
                    if ui.add(egui::Button::new("+ Resume").min_size(Vec2::new(0.0, 34.0))).clicked() {
                        match new_project("", "resume") {
                            Ok(n) => {
                                log_event(&n, "New project", true, "Resume template");
                                self.projects = list_projects();
                                to_open = Some(n);
                            }
                            Err(e) => self.set_status(e, true),
                        }
                    }
                    if ui.add(egui::Button::new("Upload ZIP").min_size(Vec2::new(0.0, 34.0))).clicked() {
                        if let Some(path) = rfd::FileDialog::new().add_filter("ZIP Archive", &["zip"]).pick_file() {
                            match fs::read(&path) {
                                Ok(bytes) => {
                                    let fname = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                                    match import_zip(&fname, &bytes) {
                                        Ok(n) => {
                                            log_event(&n, "Upload ZIP", true, &fname);
                                            self.projects = list_projects();
                                            to_open = Some(n);
                                        }
                                        Err(e) => self.set_status(e, true),
                                    }
                                }
                                Err(e) => self.set_status(format!("Failed to read ZIP: {e}"), true),
                            }
                        }
                    }
                });
            });
            ui.add_space(18.0);

            if !self.gemini_key_set && !self.claude_key_set {
                Frame::new()
                    .fill(pal.glass_card_color())
                    .stroke(Stroke::new(1.0, pal.amber.gamma_multiply(0.6)))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("AI is off").color(pal.amber));
                            ui.label(RichText::new("Sign in with Google or Claude to turn on AI writing, debugging and Drive sync.").size(12.5).color(pal.muted));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Sign in").clicked() {
                                    self.profile_err.clear();
                                    self.show_login = true;
                                }
                            });
                        });
                    });
                ui.add_space(12.0);
            }

            // Toolbar: search grows on the left; sort + view switch on the right, all one height.
            let ctrl_h = 34.0;
            ui.horizontal(|ui| {
                let search_w = (ui.available_width() - 330.0).clamp(220.0, 520.0);
                search_box(ui, pal, &mut self.project_search, HOME_SEARCH_HINT, search_w, ctrl_h);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    Frame::new()
                        .fill(pal.glass_card_color())
                        .stroke(Stroke::new(1.0, pal.line))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::same(3))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                for (lbl, val) in [("Tiles", true), ("List", false)] { // right-to-left parent: reads "List | Tiles"
                                    let sel = tiles == val;
                                    let b = egui::Button::new(RichText::new(lbl).size(12.5).color(if sel { Color32::WHITE } else { pal.text }))
                                        .fill(if sel { pal.accent } else { Color32::TRANSPARENT })
                                        .stroke(Stroke::NONE)
                                        .corner_radius(CornerRadius::same(6))
                                        .min_size(Vec2::new(58.0, ctrl_h - 8.0));
                                    if ui.add(b).on_hover_text(format!("{lbl} view")).clicked() {
                                        tiles = val;
                                    }
                                }
                            });
                        });
                    ui.add_space(6.0);
                    let arrow = if asc { "▲" } else { "▼" };
                    let sort_btn = egui::Button::new(RichText::new(format!("Sort: {}  {arrow}", sort_label(sort_key))).size(12.5))
                        .corner_radius(CornerRadius::same(8))
                        .min_size(Vec2::new(0.0, ctrl_h));
                    let r = ui.add(sort_btn);
                    egui::Popup::menu(&r).show(|ui| {
                        for k in [SORT_NAME, SORT_DATE, SORT_BUILDS] {
                            if ui.selectable_label(sort_key == k, sort_label(k)).clicked() {
                                sort_key = k;
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui.selectable_label(asc, "Ascending").clicked() {
                            asc = true;
                            ui.close();
                        }
                        if ui.selectable_label(!asc, "Descending").clicked() {
                            asc = false;
                            ui.close();
                        }
                    });
                });
            });
            ui.add_space(14.0);

            let query = self.project_search.trim().to_lowercase();
            let mut items: Vec<(String, u64, usize)> = self
                .projects
                .iter()
                .map(|p| {
                    (
                        p["name"].as_str().unwrap_or("").to_string(),
                        p["modified"].as_u64().unwrap_or(0),
                        p["history"].as_array().map_or(0, |h| h.len()),
                    )
                })
                .filter(|(n, _, _)| query.is_empty() || n.to_lowercase().contains(&query))
                .collect();
            sort_projects(&mut items, sort_key, asc);
            items.sort_by_key(|(n, _, _)| !self.pinned.contains(n)); // stable: pinned first, chosen order kept

            if items.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    let msg = if query.is_empty() { "No projects yet." } else { "No projects match your search." };
                    ui.label(RichText::new(msg).size(15.0).color(pal.muted));
                    ui.label(RichText::new("Create a new paper, start a resume, or upload an Overleaf ZIP.").size(12.5).color(pal.muted));
                });
                return;
            }

            if tiles {
                ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    let gap = 14.0;
                    let w = ui.available_width();
                    let n = (((w + gap) / (150.0 + 40.0 * self.icon_scale + gap)).floor() as usize).max(1);
                    let tile_w = (w - gap * (n - 1) as f32) / n as f32;
                    for row in items.chunks(n) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            for (i, (name, modified, builds)) in row.iter().enumerate() {
                                if i > 0 {
                                    ui.add_space(gap);
                                }
                                if self.project_tile(ui, ctx, pal, Vec2::new(tile_w, 150.0 * self.icon_scale + 70.0), name, *modified, *builds) {
                                    to_open = Some(name.clone());
                                }
                            }
                        });
                        ui.add_space(gap);
                    }
                });
            } else {
                // Column headers: click to sort, click again to flip direction (like Explorer).
                let (hrect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::hover());
                for k in [SORT_NAME, SORT_DATE, SORT_BUILDS] {
                    let col = list_col(hrect, k as usize);
                    let r = ui.interact(col, egui::Id::new(("home_col", k)), Sense::click());
                    let mut txt = sort_label(k).to_string();
                    if sort_key == k {
                        txt.push_str(if asc { "  ▲" } else { "  ▼" });
                    }
                    let color = if r.hovered() || sort_key == k { pal.text } else { pal.muted };
                    let x = if k == SORT_NAME { 24.0 + 16.0 * self.icon_scale } else { 8.0 };
                    ui.painter().text(col.left_center() + Vec2::new(x, 0.0), egui::Align2::LEFT_CENTER, txt, FontId::proportional(12.0), color);
                    if r.clicked() {
                        if sort_key == k {
                            asc = !asc;
                        } else {
                            sort_key = k;
                            asc = k == SORT_NAME;
                        }
                    }
                }
                ui.add_space(6.0);

                // Each project sits in its own glass card.
                ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    for (name, modified, builds) in &items {
                        if self.project_row(ui, pal, name, *modified, *builds) {
                            to_open = Some(name.clone());
                        }
                    }
                });
            }
        });

        ui.data_mut(|d| d.insert_persisted(prefs_id, (tiles, sort_key, asc)));
        if let Some(n) = to_open {
            self.open_project_by_name(ctx, &n);
        }
    }

    /// Shared Open/Delete/Pin handling for a list row or tile; returns true when the project should open.
    fn project_item_actions(&mut self, ui: &mut Ui, pal: ThemePalette, resp: &egui::Response, del: egui::Rect, hovered: bool, name: &str) -> bool {
        let mut open = resp.clicked();
        if hovered && ui.put(del, egui::Button::new(RichText::new("Delete").size(12.0).color(pal.red)).frame(false)).clicked() {
            self.confirm_delete_proj = Some(name.to_string());
            open = false;
        }
        resp.context_menu(|ui| {
            if ui.button("Open").clicked() {
                open = true;
                ui.close();
            }
            let pinned = self.pinned.contains(name);
            if ui.button(if pinned { "Unpin" } else { "Pin to top" }).clicked() {
                if pinned {
                    self.pinned.remove(name);
                } else {
                    self.pinned.insert(name.to_string());
                }
                let list: Vec<String> = self.pinned.iter().cloned().collect();
                ui.data_mut(|d| d.insert_persisted(egui::Id::new(PINNED_ID), list));
                ui.close();
            }
            if ui.button("Open folder in File Explorer").clicked() {
                if let Ok(dir) = project_dir(name) {
                    let _ = std::process::Command::new("explorer").arg(dir).spawn();
                }
                ui.close();
            }
            ui.separator();
            if ui.button(RichText::new("Delete").color(pal.red)).clicked() {
                self.confirm_delete_proj = Some(name.to_string());
                ui.close();
            }
        });
        open
    }

    /// Glass card behind a list row / tile; hover brightens it and tints the border.
    fn paint_item_card(ui: &Ui, pal: ThemePalette, rect: egui::Rect, id: egui::Id, hovered: bool, radius: f32) {
        let t = ui.ctx().animate_bool_responsive(id, hovered);
        let fill = pal.glass_card_color().lerp_to_gamma(pal.surface_hover, t * 0.6);
        let border = pal.glass_border_color().lerp_to_gamma(pal.accent, t * 0.5);
        ui.painter().rect(rect, radius, fill, Stroke::new(1.0, border), egui::StrokeKind::Inside);
    }

    /// One Explorer-style list row in its own card. Click opens; hover button or right-click deletes.
    fn project_row(&mut self, ui: &mut Ui, pal: ThemePalette, name: &str, modified: u64, builds: usize) -> bool {
        let s = self.icon_scale;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), (20.0 * s + 22.0).max(44.0)), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Open project {name}")));
        let hovered = ui.rect_contains_pointer(rect) || resp.context_menu_opened(); // not resp.hovered(): the Delete button on top would steal it
        Self::paint_item_card(ui, pal, rect, egui::Id::new(("proj_row", name)), hovered, 10.0);
        let icon = egui::Rect::from_center_size(egui::pos2(rect.left() + 16.0 + 8.0 * s, rect.center().y), Vec2::new(16.0, 20.0) * s);
        paint_file_icon(ui.painter(), icon, pal.accent, None);
        // Child ui: placing labels/buttons with put must not move the parent layout cursor.
        let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(rect));

        let name_col = list_col(rect, 0);
        let name_rect = egui::Rect::from_min_max(egui::pos2(rect.left() + 24.0 + 16.0 * s, rect.top()), egui::pos2(name_col.right() - 8.0, rect.bottom()));
        let label = if self.pinned.contains(name) { format!("★  {name}") } else { name.to_string() };
        put_left(ui, name_rect, egui::Label::new(RichText::new(label).size(13.5).color(pal.text)).truncate().selectable(false));
        let date_col = list_col(rect, 1).shrink2(Vec2::new(8.0, 0.0));
        put_left(ui, date_col, egui::Label::new(RichText::new(format_rel_time(modified)).size(12.5).color(pal.muted)).selectable(false));
        let builds_col = list_col(rect, 2).shrink2(Vec2::new(8.0, 0.0));
        put_left(ui, builds_col, egui::Label::new(RichText::new(builds.to_string()).size(12.5).color(pal.muted)).selectable(false));

        let del = egui::Rect::from_min_size(egui::pos2(rect.right() - 74.0, rect.center().y - 12.0), Vec2::new(66.0, 24.0));
        self.project_item_actions(ui, pal, &resp, del, hovered, name)
    }

    /// One tile: first page of the compiled PDF as a thumbnail (document icon until one exists), name, date.
    #[allow(clippy::too_many_arguments)]
    fn project_tile(&mut self, ui: &mut Ui, ctx: &egui::Context, pal: ThemePalette, size: Vec2, name: &str, modified: u64, builds: usize) -> bool {
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Open project {name}")));
        let hovered = ui.rect_contains_pointer(rect) || resp.context_menu_opened(); // not resp.hovered(): the Delete button on top would steal it
        Self::paint_item_card(ui, pal, rect, egui::Id::new(("proj_tile", name)), hovered, 12.0);

        let thumb_rect = egui::Rect::from_min_size(rect.min + Vec2::splat(8.0), Vec2::new(rect.width() - 16.0, 150.0 * self.icon_scale));
        match self.thumbnail(ctx, name) {
            Some(tex) => {
                // Show the top of the page at full width, like Overleaf / Explorer previews.
                let [iw, ih] = tex.size().map(|v| v as f32);
                let aspect = thumb_rect.width() / thumb_rect.height();
                let uv = if iw / ih < aspect {
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, ((iw / aspect) / ih).min(1.0)))
                } else {
                    let u = ((ih * aspect) / iw).min(1.0);
                    egui::Rect::from_min_max(egui::pos2((1.0 - u) / 2.0, 0.0), egui::pos2((1.0 + u) / 2.0, 1.0))
                };
                egui::Image::new(&tex).uv(uv).corner_radius(CornerRadius::same(8)).paint_at(ui, thumb_rect);
                ui.painter().rect_stroke(thumb_rect, 8.0, Stroke::new(1.0, pal.line), egui::StrokeKind::Inside);
            }
            None => {
                ui.painter().rect_filled(thumb_rect, 8.0, pal.surface.gamma_multiply(0.6));
                let icon = egui::Rect::from_center_size(thumb_rect.center(), Vec2::new(40.0, 50.0) * self.icon_scale);
                paint_file_icon(ui.painter(), icon, pal.accent, Some("TEX"));
            }
        }
        // Child ui: placing labels/buttons with put must not move the parent layout cursor.
        let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(rect));
        let label = if self.pinned.contains(name) { format!("★  {name}") } else { name.to_string() };
        let name_rect = egui::Rect::from_min_size(egui::pos2(rect.left() + 12.0, thumb_rect.bottom() + 8.0), Vec2::new(rect.width() - 24.0, 22.0));
        put_left(ui, name_rect, egui::Label::new(RichText::new(label).size(13.5).color(pal.text)).truncate().selectable(false));
        let meta = if builds > 0 { format!("{}  ·  {builds} builds", format_rel_time(modified)) } else { format_rel_time(modified) };
        let meta_rect = egui::Rect::from_min_size(egui::pos2(rect.left() + 12.0, thumb_rect.bottom() + 30.0), Vec2::new(rect.width() - 24.0, 18.0));
        put_left(ui, meta_rect, egui::Label::new(RichText::new(meta).size(11.5).color(pal.muted)).truncate().selectable(false));

        let del = egui::Rect::from_min_size(egui::pos2(rect.right() - 70.0, rect.top() + 12.0), Vec2::new(56.0, 22.0));
        self.project_item_actions(ui, pal, &resp, del, hovered, name)
    }

    /// Cached first-page thumbnail of a project's compiled PDF; renders it in the background when missing or stale.
    fn thumbnail(&mut self, ctx: &egui::Context, name: &str) -> Option<TextureHandle> {
        let p = self.projects.iter().find(|p| p["name"].as_str() == Some(name))?;
        let (pdf, stamp) = (p["pdf"].as_str()?.to_string(), p["pdf_modified"].as_u64().unwrap_or(0));
        match self.thumbs.get(name) {
            Some((st, tex)) if *st == stamp => tex.clone(),
            _ if self.thumb_jobs >= 2 => None, // at most 2 renders at once; the rest start as these finish
            _ => {
                self.thumbs.insert(name.to_string(), (stamp, None));
                self.thumb_jobs += 1;
                let (tx, ctx, key) = (self.bg_tx.clone(), ctx.clone(), name.to_string());
                thread::spawn(move || {
                    let image = render_pdf_file(std::path::Path::new(&pdf), 0.6, 1)
                        .ok()
                        .and_then(|mut pages| (!pages.is_empty()).then(|| pages.swap_remove(0).image));
                    let _ = tx.send(BgResult::Thumb { key, stamp, image });
                    ctx.request_repaint();
                });
                None
            }
        }
    }

    fn ui_workspace(&mut self, ui: &mut Ui, ctx: &egui::Context, pal: ThemePalette) {
        let files_t = ctx.animate_bool_responsive(egui::Id::new("anim_files_drawer"), self.show_files_drawer);
        let ai_t = ctx.animate_bool_responsive(egui::Id::new("anim_ai_drawer"), self.show_ai_drawer);

        // Left Sidebar: File Tree & PDF History (Smooth slide animation)
        if files_t > 0.02 {
            egui::Panel::left("files_panel")
                .resizable(self.show_files_drawer)
                .default_size((220.0 * files_t).max(16.0))
                .size_range(16.0..=360.0)
                .frame(pal.glass_panel_frame(Margin::same(10)))
                .show(ui, |ui| self.ui_file_tree(ui, ctx, pal));
        }

        // Right AI Drawer (Smooth slide animation)
        if ai_t > 0.02 {
            egui::Panel::right("ai_panel")
                .resizable(self.show_ai_drawer)
                .default_size((340.0 * ai_t).max(16.0))
                .size_range(16.0..=520.0)
                .frame(pal.glass_panel_frame(Margin::same(10)))
                .show(ui, |ui| self.ui_ai_panel(ui, ctx, pal));
        }

        // Formatting Toolbar under top bar (Glass)
        if self.view_mode != ViewMode::Pdf {
            egui::Panel::top("fmt_toolbar")
                .frame(pal.glass_panel_frame(Margin::symmetric(10, 5)))
                .show(ui, |ui| self.ui_formatting_bar(ui, ctx, pal));
        }

        // Central area: Code / Split / PDF
        egui::CentralPanel::default()
            .frame(Frame::new().fill(pal.bg).inner_margin(Margin::same(0)))
            .show(ui, |ui| {
                paint_glass_backdrop(ui, pal);
                match self.view_mode {
                    ViewMode::Code => {
                        self.ui_code_pane(ui, pal);
                    }
                    ViewMode::Pdf => {
                        self.ui_pdf_pane(ui, ctx, pal);
                    }
                    ViewMode::Split => {
                        let avail = ui.available_size();
                        ui.columns(2, |cols| {
                            cols[0].set_min_height(avail.y);
                            self.ui_code_pane(&mut cols[0], pal);
                            cols[1].set_min_height(avail.y);
                            self.ui_pdf_pane(&mut cols[1], ctx, pal);
                        });
                    }
                }
            });
    }

    fn ui_file_tree(&mut self, ui: &mut Ui, ctx: &egui::Context, pal: ThemePalette) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("FILES").strong().size(12.0).color(pal.muted));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("×").on_hover_text("Hide Files panel").clicked() {
                    self.show_files_drawer = false;
                }
                if ui.small_button("Upload").on_hover_text("Upload image or .tex/.bib file").clicked() {
                    if let Some(paths) = rfd::FileDialog::new().pick_files() {
                        if let Some(root) = self.open_project_root.clone() {
                            for p in paths {
                                if let Ok(bytes) = fs::read(&p) {
                                    let fname = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                                    match save_asset(&root, &fname, &bytes) {
                                        Ok(rel) => {
                                            self.set_status(format!("Uploaded {rel}"), false);
                                        }
                                        Err(e) => self.set_status(e, true),
                                    }
                                }
                            }
                            self.refresh_project_files();
                        }
                    }
                }
                if ui.small_button("+ File").clicked() {
                    self.new_file_name.clear();
                    self.show_new_file_dlg = true;
                }
            });
        });
        ui.add_space(6.0);

        // Group files by top-level folder
        let mut root_files: Vec<String> = vec![];
        let mut folders: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for f in &self.files {
            if let Some((dir, _)) = f.split_once('/') {
                folders.entry(dir.to_string()).or_default().push(f.clone());
            } else {
                root_files.push(f.clone());
            }
        }

        let mut clicked_file: Option<String> = None;
        let mut rename_req: Option<String> = None;
        let mut delete_req: Option<String> = None;

        ScrollArea::vertical().id_salt("file_tree_scroll").show(ui, |ui| {
            for f in &root_files {
                self.render_file_row(ui, pal, f, f, &mut clicked_file, &mut rename_req, &mut delete_req);
            }

            for (dir, items) in &folders {
                let collapsed = self.collapsed_dirs.contains(dir);
                let resp = ui.horizontal(|ui| {
                    let (rect, chev_resp) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
                    paint_chevron(ui, rect, !collapsed, pal.muted);
                    let lbl_resp = ui.selectable_label(false, RichText::new(dir).strong().size(13.0).color(pal.text));
                    chev_resp.clicked() || lbl_resp.clicked()
                });
                if resp.inner {
                    if collapsed {
                        self.collapsed_dirs.remove(dir);
                    } else {
                        self.collapsed_dirs.insert(dir.clone());
                    }
                }
                if !collapsed {
                    ui.indent(dir, |ui| {
                        for f in items {
                            let short = f.strip_prefix(&format!("{dir}/")).unwrap_or(f);
                            self.render_file_row(ui, pal, f, short, &mut clicked_file, &mut rename_req, &mut delete_req);
                        }
                    });
                }
            }

            if !self.pdf_history.is_empty() {
                ui.add_space(14.0);
                ui.separator();
                ui.label(RichText::new("PDF HISTORY").strong().size(11.0).color(pal.muted));
                ui.add_space(4.0);
                for (name, ts) in self.pdf_history.clone() {
                    let rel = format!(".history/{name}");
                    if ui.selectable_label(self.current_pdf_rel.as_deref() == Some(&rel), RichText::new(format!("{} ({})", name, format_rel_time(ts))).size(12.0)).clicked() {
                        self.load_pdf(ctx, &rel);
                    }
                }
            }
        });

        if let Some(f) = clicked_file {
            self.open_file(ctx, &f);
        }
        if let Some(f) = rename_req {
            self.rename_file_target = Some(f.clone());
            self.rename_file_input = f;
        }
        if let Some(f) = delete_req {
            self.confirm_delete_file = Some(f);
        }
    }

    fn render_file_row(
        &self,
        ui: &mut Ui,
        pal: ThemePalette,
        full_rel: &str,
        label: &str,
        clicked: &mut Option<String>,
        rename_req: &mut Option<String>,
        delete_req: &mut Option<String>,
    ) {
        let selected = self.current_file == full_rel;
        let badge = file_badge_label(full_rel);
        let badge_col = match badge {
            "TEX" => pal.accent,
            "BIB" => pal.amber,
            "IMG" => pal.green,
            "PDF" => pal.red,
            _ => pal.muted,
        };
        let row = ui.horizontal(|ui| {
            paint_doc_badge(ui, badge_col, badge);
            let resp = ui.selectable_label(selected, RichText::new(label).size(13.0));
            if resp.clicked() {
                *clicked = Some(full_rel.to_string());
            }
            resp.context_menu(|ui| {
                if ui.button("Rename").clicked() {
                    *rename_req = Some(full_rel.to_string());
                    ui.close();
                }
                if ui.button(RichText::new("Delete").color(pal.red)).clicked() {
                    *delete_req = Some(full_rel.to_string());
                    ui.close();
                }
            });
        });
        let _ = row;
    }

    /// Wrap the selection (or insert at the cursor) like a Word formatting button: one undo step, cursor lands inside.
    fn wrap(&mut self, before: &str, after: &str) {
        self.push_undo();
        let c = wrap_selection(&mut self.code_text, self.cursor_range, before, after);
        self.after_edit(c);
    }

    /// Same as `wrap`, first adding `\usepackage{pkg}` to the preamble if missing.
    fn wrap_pkg(&mut self, pkg: &str, before: &str, after: &str) {
        self.push_undo();
        let at = self.code_text.find("\\begin{document}").map(|b| self.code_text[..b].chars().count());
        let old = self.code_text.chars().count();
        ensure_package(&mut self.code_text, pkg);
        let delta = self.code_text.chars().count() - old;
        // The \usepackage line lands before the cursor, so shift the selection past it.
        if let (Some(at), Some((a, b)), true) = (at, self.cursor_range, delta > 0) {
            let sh = |x: usize| if x >= at { x + delta } else { x };
            self.cursor_range = Some((sh(a), sh(b)));
        }
        let c = wrap_selection(&mut self.code_text, self.cursor_range, before, after);
        self.after_edit(c);
    }

    fn insert_snippet(&mut self, snippet: &str) {
        self.push_undo();
        let c = insert_at_cursor(&mut self.code_text, self.cursor_range, snippet);
        self.after_edit(c);
    }

    fn after_edit(&mut self, cursor: usize) {
        self.cursor_range = Some((cursor, cursor));
        self.pending_cursor = Some(cursor);
        self.last_edit_time = Some(Instant::now());
        self.last_typing = None;
    }

    /// Selected text as (start, end, text) in char indices, if anything is selected.
    fn selection(&self) -> Option<(usize, usize, String)> {
        let (a, b) = self.cursor_range?;
        let (s, e) = (a.min(b), a.max(b));
        (s < e).then(|| (s, e, self.code_text.chars().skip(s).take(e - s).collect()))
    }

    /// Word-style ribbon: tabs (Home / Insert / References) over captioned groups of commands.
    fn ui_formatting_bar(&mut self, ui: &mut Ui, ctx: &egui::Context, pal: ThemePalette) {
        let _ = ctx;
        // Tab strip + always-visible actions
        ui.horizontal(|ui| {
            for (i, tab) in ["Home", "Insert", "References"].into_iter().enumerate() {
                let sel = self.ribbon_tab == i as u8;
                let t = RichText::new(tab).size(13.0).color(if sel { pal.accent } else { pal.muted });
                let r = ui.add(egui::Button::new(if sel { t.strong() } else { t }).frame(false));
                if sel {
                    ui.painter().hline(r.rect.x_range(), r.rect.bottom() + 1.0, Stroke::new(2.0, pal.accent));
                }
                if r.clicked() {
                    self.ribbon_tab = i as u8;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let rec_lbl = if self.compiling { "Compiling..." } else { "Recompile" };
                let rec_btn = egui::Button::new(RichText::new(rec_lbl).color(Color32::WHITE)).fill(pal.green).stroke(Stroke::NONE);
                if ui.add_enabled(!self.compiling, rec_btn).on_hover_text("Compile PDF (Ctrl+Enter)").clicked() {
                    self.trigger_compile();
                }
                if ui.add(egui::Button::new(RichText::new("Help me write").color(pal.accent))).on_hover_text("Ctrl+K").clicked() {
                    self.show_hw_dlg = true;
                    self.hw_result = None;
                }
            });
        });
        ui.add_space(2.0);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            match self.ribbon_tab {
                0 => {
                    ribbon_group(ui, pal, "Undo", |ui| {
                        if ui.add_enabled(!self.undo_stack.is_empty(), egui::Button::new("Undo")).on_hover_text("Undo (Ctrl+Z)").clicked() {
                            self.undo();
                        }
                        if ui.add_enabled(!self.redo_stack.is_empty(), egui::Button::new("Redo")).on_hover_text("Redo (Ctrl+Y)").clicked() {
                            self.redo();
                        }
                    });
                    ribbon_group(ui, pal, "Styles", |ui| {
                        ui.menu_button("Style  ▼", |ui| {
                            for (lbl, before, after) in [
                                ("Title", "\\title{", "}"),
                                ("Heading 1   (Ctrl+1)", "\\section{", "}"),
                                ("Heading 2   (Ctrl+2)", "\\subsection{", "}"),
                                ("Heading 3   (Ctrl+3)", "\\subsubsection{", "}"),
                                ("Paragraph heading", "\\paragraph{", "}"),
                                ("Quote", "\n\\begin{quote}\n", "\n\\end{quote}\n"),
                                ("Abstract", "\n\\begin{abstract}\n", "\n\\end{abstract}\n"),
                            ] {
                                if ui.button(lbl).clicked() {
                                    self.wrap(before, after);
                                    ui.close();
                                }
                            }
                        });
                    });
                    ribbon_group(ui, pal, "Font", |ui| {
                        for (txt, tip, before) in [
                            (RichText::new("B").strong(), "Bold (Ctrl+B)", "\\textbf{"),
                            (RichText::new("I").italics(), "Italic (Ctrl+I)", "\\textit{"),
                            (RichText::new("U").underline(), "Underline (Ctrl+U)", "\\underline{"),
                            (RichText::new("x₂"), "Subscript (Ctrl+=)", "\\textsubscript{"),
                            (RichText::new("x²"), "Superscript (Ctrl+Shift+=)", "\\textsuperscript{"),
                            (RichText::new("Code").monospace(), "Monospace", "\\texttt{"),
                            (RichText::new("Aa"), "Small caps (Ctrl+Shift+K)", "\\textsc{"),
                        ] {
                            if ui.add(egui::Button::new(txt.color(pal.text)).min_size(Vec2::new(28.0, 0.0))).on_hover_text(tip).clicked() {
                                self.wrap(before, "}");
                            }
                        }
                        ui.menu_button("Size  ▼", |ui| {
                            for (lbl, cmd) in [("Tiny", "tiny"), ("Small", "small"), ("Normal", "normalsize"), ("Large", "large"), ("Larger", "Large"), ("Huge", "huge")] {
                                if ui.button(lbl).clicked() {
                                    self.wrap(&format!("{{\\{cmd} "), "}");
                                    ui.close();
                                }
                            }
                        });
                        ui.menu_button("Color  ▼", |ui| {
                            for (lbl, c, col) in [
                                ("Red", "red", Color32::from_rgb(220, 38, 38)),
                                ("Blue", "blue", Color32::from_rgb(37, 99, 235)),
                                ("Green", "teal", Color32::from_rgb(13, 148, 136)),
                                ("Orange", "orange", Color32::from_rgb(234, 88, 12)),
                                ("Gray", "gray", Color32::from_rgb(120, 120, 120)),
                            ] {
                                if ui.button(RichText::new(format!("●  {lbl}")).color(col)).clicked() {
                                    self.wrap_pkg("xcolor", &format!("\\textcolor{{{c}}}{{"), "}");
                                    ui.close();
                                }
                            }
                        });
                    });
                    ribbon_group(ui, pal, "Paragraph", |ui| {
                        if ui.button("• List").on_hover_text("Bulleted list").clicked() {
                            self.insert_snippet("\\begin{itemize}\n  \\item \n\\end{itemize}\n");
                        }
                        if ui.button("1. List").on_hover_text("Numbered list").clicked() {
                            self.insert_snippet("\\begin{enumerate}\n  \\item \n\\end{enumerate}\n");
                        }
                        for (lbl, tip, env) in [("Left", "Align left (Ctrl+L)", "flushleft"), ("Center", "Center (Ctrl+E)", "center"), ("Right", "Align right (Ctrl+R)", "flushright")] {
                            if ui.button(lbl).on_hover_text(tip).clicked() {
                                self.wrap(&format!("\n\\begin{{{env}}}\n"), &format!("\n\\end{{{env}}}\n"));
                            }
                        }
                        if ui.button("% Comment").on_hover_text("Toggle comment (Ctrl+/)").clicked() {
                            self.push_undo();
                            toggle_comment(&mut self.code_text, self.cursor_range);
                            self.last_edit_time = Some(Instant::now());
                        }
                    });
                }
                1 => {
                    ribbon_group(ui, pal, "Illustrations", |ui| {
                        if ui.button("Image").clicked() {
                            self.img_dlg_file = self
                                .files
                                .iter()
                                .find(|f| {
                                    let l = f.to_lowercase();
                                    l.ends_with(".png") || l.ends_with(".jpg") || l.ends_with(".jpeg")
                                })
                                .cloned()
                                .unwrap_or_default();
                            self.show_image_dlg = true;
                        }
                        if ui.button("Table").clicked() {
                            self.show_table_dlg = true;
                        }
                        if ui.button("NN Diagram").clicked() {
                            self.show_nn_dlg = true;
                        }
                    });
                    ribbon_group(ui, pal, "Math", |ui| {
                        if ui.button("$x$").on_hover_text("Inline math (Ctrl+M)").clicked() {
                            self.wrap("$", "$");
                        }
                        if ui.button("Equation").on_hover_text("Equation block (Ctrl+Shift+M)").clicked() {
                            self.wrap_pkg("amsmath", "\n\\begin{equation}\n", "\n\\end{equation}\n");
                        }
                        if ui.button("Aligned").on_hover_text("Multi-line aligned equations").clicked() {
                            self.wrap_pkg("amsmath", "\n\\begin{align}\n", "\n\\end{align}\n");
                        }
                        if ui.button("Symbols").clicked() {
                            self.show_symbols_dlg = true;
                        }
                    });
                    ribbon_group(ui, pal, "Text", |ui| {
                        if ui.button("Footnote").clicked() {
                            self.wrap("\\footnote{", "}");
                        }
                        if ui.button("Link").on_hover_text("Hyperlink (adds hyperref)").clicked() {
                            self.wrap_pkg("hyperref", "\\href{https://}{", "}");
                        }
                        if ui.button("Code block").clicked() {
                            self.wrap("\n\\begin{verbatim}\n", "\n\\end{verbatim}\n");
                        }
                        if ui.button("Page break").clicked() {
                            self.insert_snippet("\n\\newpage\n");
                        }
                        if ui.button("Line break").clicked() {
                            self.insert_snippet("\\\\\n");
                        }
                    });
                }
                _ => {
                    ribbon_group(ui, pal, "Captions & links", |ui| {
                        if ui.button("Label").on_hover_text("\\label{...} to refer to later").clicked() {
                            self.wrap("\\label{", "}");
                        }
                        if ui.button("Cross-reference").on_hover_text("\\ref{...}").clicked() {
                            self.wrap("\\ref{", "}");
                        }
                        if ui.button("Cite").on_hover_text("\\cite{...}").clicked() {
                            self.wrap("\\cite{", "}");
                        }
                    });
                    ribbon_group(ui, pal, "Document", |ui| {
                        for (lbl, snip) in [
                            ("Table of contents", "\\tableofcontents\n"),
                            ("List of figures", "\\listoffigures\n"),
                            ("List of tables", "\\listoftables\n"),
                            ("Bibliography", "\n\\bibliographystyle{plain}\n\\bibliography{references}\n"),
                            ("Appendix", "\n\\appendix\n"),
                        ] {
                            if ui.button(lbl).clicked() {
                                self.insert_snippet(snip);
                            }
                        }
                    });
                }
            }
        });
    }

    /// Right-click menu inside the editor: Undo/Redo, Cut/Copy and the common formatting commands.
    fn editor_context_menu(&mut self, ui: &mut Ui) {
        if ui.add_enabled(!self.undo_stack.is_empty(), egui::Button::new("Undo").shortcut_text("Ctrl+Z")).clicked() {
            self.undo();
            ui.close();
        }
        if ui.add_enabled(!self.redo_stack.is_empty(), egui::Button::new("Redo").shortcut_text("Ctrl+Y")).clicked() {
            self.redo();
            ui.close();
        }
        ui.separator();
        let sel = self.selection();
        if ui.add_enabled(sel.is_some(), egui::Button::new("Cut").shortcut_text("Ctrl+X")).clicked() {
            if let Some((s, e, t)) = sel.clone() {
                ui.ctx().copy_text(t);
                self.push_undo();
                let c = insert_at_cursor(&mut self.code_text, Some((s, e)), "");
                self.after_edit(c);
            }
            ui.close();
        }
        if ui.add_enabled(sel.is_some(), egui::Button::new("Copy").shortcut_text("Ctrl+C")).clicked() {
            if let Some((_, _, t)) = sel {
                ui.ctx().copy_text(t);
            }
            ui.close();
        }
        ui.separator();
        for (lbl, keys, before, after) in [
            ("Bold", "Ctrl+B", "\\textbf{", "}"),
            ("Italic", "Ctrl+I", "\\textit{", "}"),
            ("Underline", "Ctrl+U", "\\underline{", "}"),
            ("Inline math", "Ctrl+M", "$", "$"),
        ] {
            if ui.add(egui::Button::new(lbl).shortcut_text(keys)).clicked() {
                self.wrap(before, after);
                ui.close();
            }
        }
        if ui.add(egui::Button::new("Comment").shortcut_text("Ctrl+/")).clicked() {
            self.push_undo();
            toggle_comment(&mut self.code_text, self.cursor_range);
            self.last_edit_time = Some(Instant::now());
            ui.close();
        }
    }

    fn ui_code_pane(&mut self, ui: &mut Ui, pal: ThemePalette) {
        Frame::new().fill(pal.bg).inner_margin(Margin::same(8)).show(ui, |ui| {
            // Editor tab bar
            ui.horizontal(|ui| {
                let title = if self.current_file.is_empty() { "No file open" } else { &self.current_file };
                ui.label(RichText::new(title).strong().size(13.0).color(pal.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("Save").clicked() {
                        self.save_current_file();
                        self.set_status("Saved", false);
                    }
                    ui.checkbox(&mut self.auto_save, "Auto-save");
                });
            });
            ui.separator();

            // AI diff proposal banner if active
            if let Some((orig, prop, recompile_after)) = self.ai_diff_proposal.clone() {
                self.ui_diff_overlay(ui, pal, &orig, &prop, recompile_after);
                return;
            }

            if self.current_file.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Select a .tex or .bib file from the sidebar.").color(pal.muted));
                });
                return;
            }

            let line_count = self.code_text.lines().count().max(1);
            let err_lines: BTreeSet<usize> = self
                .compile_errors
                .iter()
                .filter(|e| e.file.is_empty() || self.current_file.ends_with(&e.file))
                .map(|e| e.line)
                .collect();

            ScrollArea::vertical().id_salt("code_scroll").show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    // Line number gutter
                    let mut gutter = String::new();
                    for ln in 1..=line_count {
                        if err_lines.contains(&ln) {
                            gutter.push_str(&format!("!{ln:>3}\n"));
                        } else {
                            gutter.push_str(&format!("{ln:>4}\n"));
                        }
                    }
                    ui.add(
                        egui::Label::new(
                            RichText::new(gutter)
                                .font(FontId::monospace(13.5))
                                .color(pal.muted),
                        )
                        .selectable(false),
                    );
                    ui.separator();

                    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                        let mut job = highlight_latex(buf.as_str(), pal, 13.5);
                        job.wrap.max_width = wrap_width;
                        ui.fonts_mut(|f| f.layout_job(job))
                    };

                    let prev_text = self.code_text.clone();
                    let avail_w = ui.available_width().max(200.0);
                    let output = TextEdit::multiline(&mut self.code_text)
                        .id_source("latex_code_editor")
                        .font(FontId::monospace(13.5))
                        .desired_width(avail_w)
                        .desired_rows(40)
                        .lock_focus(true)
                        .frame(Frame::NONE)
                        .layouter(&mut layouter)
                        .show(ui);

                    if let Some(range) = output.cursor_range {
                        let a: usize = range.primary.index.into();
                        let b: usize = range.secondary.index.into();
                        self.cursor_range = Some((a, b));
                    }

                    let target = self.jump_to_line.take().map(|ln| char_of_line(&self.code_text, ln)).or(self.pending_cursor.take());
                    if let Some(char_idx) = target {
                        let mut state = output.state.clone();
                        let ccursor = egui::text::CCursor::new(char_idx);
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(ccursor)));
                        state.store(ui.ctx(), output.response.id);
                        output.response.request_focus();
                    }

                    if output.response.changed() {
                        // Group a typing burst into one undo step, like Word: snapshot when a burst starts
                        // (after a 1s pause, or every 5s of continuous typing).
                        let now = Instant::now();
                        let new_burst = self.last_typing.map_or(true, |(start, last)| last.elapsed().as_secs_f32() > 1.0 || start.elapsed().as_secs_f32() > 5.0);
                        if new_burst {
                            if self.undo_stack.last().map(|s| s.as_str()) != Some(prev_text.as_str()) {
                                self.undo_stack.push(prev_text);
                                if self.undo_stack.len() > 100 {
                                    self.undo_stack.remove(0);
                                }
                                self.redo_stack.clear();
                            }
                            self.last_typing = Some((now, now));
                        } else if let Some((_, last)) = &mut self.last_typing {
                            *last = now;
                        }
                        self.last_edit_time = Some(now);
                        self.autofix_attempts = 0;
                    }
                    output.response.context_menu(|ui| self.editor_context_menu(ui));
                });
            });
        });
    }

    fn ui_diff_overlay(&mut self, ui: &mut Ui, pal: ThemePalette, orig: &str, prop: &str, recompile_after: bool) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Proposed AI Changes").strong().color(pal.accent));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Discard").clicked() {
                    self.ai_diff_proposal = None;
                }
                let acc_btn = egui::Button::new(RichText::new("Accept Changes").color(Color32::WHITE)).fill(pal.green);
                if ui.add(acc_btn).clicked() {
                    self.code_text = prop.to_string();
                    self.save_current_file();
                    self.ai_diff_proposal = None;
                    if recompile_after {
                        self.trigger_compile();
                    }
                }
            });
        });
        ui.separator();

        let orig_lines: Vec<&str> = orig.lines().collect();
        let prop_lines: Vec<&str> = prop.lines().collect();
        let orig_set: BTreeSet<&str> = orig_lines.iter().copied().collect();
        let prop_set: BTreeSet<&str> = prop_lines.iter().copied().collect();

        ScrollArea::vertical().id_salt("diff_scroll").show(ui, |ui| {
            for l in &orig_lines {
                if !prop_set.contains(l) {
                    Frame::new().fill(pal.diff_del_bg).show(ui, |ui| {
                        ui.label(RichText::new(format!("- {l}")).font(FontId::monospace(12.5)).color(pal.red));
                    });
                }
            }
            ui.add_space(6.0);
            for l in &prop_lines {
                let bg = if orig_set.contains(l) { Color32::TRANSPARENT } else { pal.diff_add_bg };
                let col = if orig_set.contains(l) { pal.text } else { pal.green };
                let prefix = if orig_set.contains(l) { "  " } else { "+ " };
                Frame::new().fill(bg).show(ui, |ui| {
                    ui.label(RichText::new(format!("{prefix}{l}")).font(FontId::monospace(12.5)).color(col));
                });
            }
        });
    }

    fn ui_pdf_pane(&mut self, ui: &mut Ui, ctx: &egui::Context, pal: ThemePalette) {
        pal.glass_panel_frame(Margin::same(8)).show(ui, |ui| {
            // Preview header
            ui.horizontal(|ui| {
                if let Some((img_name, _)) = &self.preview_image {
                    ui.label(RichText::new(format!("Image: {img_name}")).strong().size(13.0).color(pal.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Close preview").clicked() {
                            self.preview_image = None;
                        }
                    });
                } else {
                    let pdf_title = self.current_pdf_rel.as_deref().unwrap_or("No PDF compiled yet");
                    ui.label(RichText::new(pdf_title).strong().size(13.0).color(pal.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Hide PDF pane").clicked() {
                            self.view_mode = ViewMode::Code;
                        }
                        let log_lbl = if self.show_logs_drawer { "Hide Logs" } else { "Compile Logs" };
                        if ui.small_button(log_lbl).clicked() {
                            self.show_logs_drawer = !self.show_logs_drawer;
                        }
                        if ui.small_button("+").on_hover_text("Zoom in").clicked() {
                            self.pdf_zoom = (self.pdf_zoom + 0.2).min(3.0);
                        }
                        if ui.small_button(format!("{}%", (self.pdf_zoom * 100.0).round() as i32)).on_hover_text("Reset zoom to Fit").clicked() {
                            self.pdf_zoom = 1.0;
                        }
                        if ui.small_button("-").on_hover_text("Zoom out").clicked() {
                            self.pdf_zoom = (self.pdf_zoom - 0.2).max(0.4);
                        }
                    });
                }
            });
            ui.separator();

            let logs_t = ctx.animate_bool_responsive(egui::Id::new("anim_logs_drawer"), self.show_logs_drawer);
            let logs_h = 170.0 * logs_t;
            let preview_h = (ui.available_height() - logs_h - 8.0).max(120.0);

            ui.allocate_ui(Vec2::new(ui.available_width(), preview_h), |ui| {
                if let Some((_, tex)) = &self.preview_image {
                    ScrollArea::both().show(ui, |ui| {
                        let avail_w = ui.available_width().max(100.0);
                        let sz = tex.size_vec2();
                        let scale = (avail_w / sz.x).min(1.0);
                        ui.image((tex.id(), sz * scale));
                    });
                } else if self.pdf_rendering {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("Rendering PDF...").color(pal.muted));
                    });
                } else if !self.pdf_err.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new(&self.pdf_err).color(pal.red));
                    });
                } else if self.pdf_pages.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("Click Recompile (Ctrl+Enter) to build your PDF.").color(pal.muted));
                    });
                } else {
                    ScrollArea::both().id_salt("pdf_scroll").show(ui, |ui| {
                        if ui.rect_contains_pointer(ui.max_rect()) {
                            let scroll_delta = ui.input(|i| if i.modifiers.ctrl || i.modifiers.command { i.smooth_scroll_delta.y } else { 0.0 });
                            if scroll_delta != 0.0 {
                                self.pdf_zoom = (self.pdf_zoom + scroll_delta * 0.002).clamp(0.4, 3.0);
                            }
                        }
                        let avail_w = (ui.available_width() - 16.0).max(100.0);
                        let pages_meta: Vec<(egui::TextureId, Vec2, f32, f32)> = self
                            .pdf_pages
                            .iter()
                            .map(|(t, pw, ph)| (t.id(), t.size_vec2(), *pw, *ph))
                            .collect();
                        for (idx, (tid, px_sz, pt_w, pt_h)) in pages_meta.into_iter().enumerate() {
                            let disp_w = (avail_w * self.pdf_zoom).max(80.0);
                            let disp_h = disp_w * (px_sz.y / px_sz.x.max(1.0));
                            let resp = ui.add(egui::Image::new((tid, Vec2::new(disp_w, disp_h))).sense(Sense::click()));
                            if resp.clicked() {
                                if let Some(pos) = resp.interact_pointer_pos() {
                                    let rel_x = ((pos.x - resp.rect.left()) / disp_w).clamp(0.0, 1.0);
                                    let rel_y = ((pos.y - resp.rect.top()) / disp_h).clamp(0.0, 1.0);
                                    let pt_x = (rel_x * pt_w) as f64;
                                    let pt_y = (rel_y * pt_h) as f64;
                                    if let (Some(root), Some(pdf_rel)) = (&self.open_project_root, &self.current_pdf_rel) {
                                        if let Ok(v) = synctex(root, pdf_rel, (idx + 1) as u32, pt_x, pt_y) {
                                            if let (Some(file), Some(line)) = (v["file"].as_str(), v["line"].as_u64()) {
                                                self.open_file(ctx, file);
                                                self.jump_to_line = Some(line as usize);
                                            }
                                        }
                                    }
                                }
                            }
                            ui.add_space(10.0);
                        }
                    });
                }
            });

            if logs_t > 0.02 {
                ui.separator();
                self.ui_compile_logs(ui, ctx, pal);
            }
        });
    }

    fn ui_compile_logs(&mut self, ui: &mut Ui, ctx: &egui::Context, pal: ThemePalette) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Errors ({})", self.compile_errors.len())).strong().size(12.0).color(pal.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.autofix_toggle(ui);
                if !self.compile_errors.is_empty() && ui.small_button("Fix all with AI").clicked() {
                    self.start_ai_edit(ctx, "Fix all compile errors in this file.".into(), true);
                }
            });
        });
        ScrollArea::vertical().id_salt("compile_logs_scroll").max_height(135.0).show(ui, |ui| {
            for err in self.compile_errors.clone() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{}:{}: {}", err.file, err.line, err.msg)).size(12.0).color(pal.red));
                    if err.line > 0 && ui.small_button("Go").clicked() {
                        if !err.file.is_empty() && !self.current_file.ends_with(&err.file) {
                            if let Some(m) = self.files.iter().find(|f| f.ends_with(&err.file)).cloned() {
                                self.open_file(ctx, &m);
                            }
                        }
                        self.jump_to_line = Some(err.line);
                    }
                });
            }
            if self.compile_errors.is_empty() && !self.compile_log.is_empty() {
                ui.label(RichText::new(&self.compile_log).font(FontId::monospace(11.5)).color(pal.muted));
            }
        });
    }

    fn ui_ai_panel(&mut self, ui: &mut Ui, ctx: &egui::Context, pal: ThemePalette) {
        ui.horizontal(|ui| {
            if ui.selectable_label(self.ai_tab == AiTab::Chat, "AI Chat").clicked() {
                self.ai_tab = AiTab::Chat;
            }
            if ui.selectable_label(self.ai_tab == AiTab::Edit, "Edit File").clicked() {
                self.ai_tab = AiTab::Edit;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("×").on_hover_text("Close AI panel").clicked() {
                    self.show_ai_drawer = false;
                }
                if self.ai_tab == AiTab::Chat && ui.small_button("New chat").clicked() {
                    self.chat_messages.clear();
                }
            });
        });
        ui.separator();

        // Model picker
        ui.horizontal(|ui| {
            if self.models.is_empty() {
                ui.label(RichText::new("No model configured").color(pal.amber));
                if ui.small_button("Add Key").clicked() {
                    self.show_profile_dlg = true;
                }
            } else {
                let cur_label = self.models[self.selected_model_idx].0.clone();
                egui::ComboBox::from_id_salt("ai_model_picker")
                    .selected_text(cur_label)
                    .width(ui.available_width() - 10.0)
                    .show_ui(ui, |ui| {
                        for (idx, (lbl, _, _)) in self.models.iter().enumerate() {
                            if ui.selectable_label(self.selected_model_idx == idx, lbl).clicked() {
                                self.selected_model_idx = idx;
                            }
                        }
                    });
            }
        });

        ui.checkbox(&mut self.full_project_access, "Full Project Access (all chapters & .bib)");
        ui.separator();

        match self.ai_tab {
            AiTab::Chat => {
                let input_h = 84.0;
                let chat_h = (ui.available_height() - input_h - 12.0).max(120.0);

                ui.allocate_ui(Vec2::new(ui.available_width(), chat_h), |ui| {
                    ScrollArea::vertical().id_salt("ai_chat_scroll").stick_to_bottom(true).show(ui, |ui| {
                        if self.chat_messages.is_empty() {
                            ui.label(RichText::new("Ask about your paper, abstract, chapters, equations, or compile errors.").size(12.5).color(pal.muted));
                        }
                        let mut insert_req: Option<String> = None;
                        let mut copy_req: Option<String> = None;
                        let mut apply_edits_req: Option<(Vec<(String, String)>, bool)> = None;
                        let msg_count = self.chat_messages.len();
                        for (msg_idx, msg) in self.chat_messages.iter().enumerate() {
                            let is_user = msg.role == "user";
                            let is_last_streaming = self.chat_streaming && msg_idx + 1 == msg_count && !is_user;
                            let bg = if is_user { pal.surface } else { pal.bg };
                            Frame::new()
                                .fill(bg)
                                .stroke(Stroke::new(1.0, pal.line))
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(Margin::same(10))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(if is_user { "You" } else { "inkTeX AI" }).strong().size(11.5).color(if is_user { pal.accent } else { pal.green }));
                                        if is_last_streaming {
                                            ui.add_space(6.0);
                                            ui.add(egui::Spinner::new().size(12.0));
                                            ui.label(RichText::new(if msg.text.is_empty() { "Thinking..." } else { "Generating..." }).size(11.0).color(pal.muted));
                                        }
                                    });
                                    if !msg.thinking.is_empty() {
                                        ui.collapsing("Reasoning", |ui| {
                                            ui.label(RichText::new(&msg.thinking).size(11.5).color(pal.muted));
                                        });
                                    }
                                    if !msg.text.is_empty() {
                                        render_chat_markdown(ui, &msg.text, pal);
                                    }
                                    if !is_user && !msg.text.is_empty() {
                                        let edits = parse_file_edits(&msg.text);
                                        if !edits.is_empty() {
                                            ui.add_space(6.0);
                                            ui.horizontal_wrapped(|ui| {
                                                for (path, content) in &edits {
                                                    if ui.small_button(format!("Apply to {path}")).clicked() {
                                                        apply_edits_req = Some((vec![(path.clone(), content.clone())], false));
                                                    }
                                                }
                                                let all_btn = egui::Button::new(RichText::new(format!("Apply All ({}) & Recompile", edits.len())).color(Color32::WHITE)).fill(pal.green);
                                                if ui.add(all_btn).clicked() {
                                                    apply_edits_req = Some((edits.clone(), true));
                                                }
                                            });
                                        } else if let Some(code) = extract_latex_block(&msg.text) {
                                            ui.add_space(6.0);
                                            ui.horizontal(|ui| {
                                                if ui.small_button("Insert LaTeX").clicked() {
                                                    insert_req = Some(code.clone());
                                                }
                                                if ui.small_button("Copy").clicked() {
                                                    copy_req = Some(code.clone());
                                                }
                                            });
                                        }
                                    }
                                });
                            ui.add_space(6.0);
                        }
                        if let Some((edits, recompile)) = apply_edits_req {
                            let count = edits.len();
                            for (path, content) in edits {
                                if let Err(e) = self.apply_file_edit(&path, &content) {
                                    self.set_status(format!("Failed editing {path}: {e}"), true);
                                }
                            }
                            self.set_status(format!("Applied AI edits to {count} file(s)"), false);
                            if recompile {
                                self.trigger_compile();
                            }
                        }
                        if let Some(code) = insert_req {
                            let c = insert_at_cursor(&mut self.code_text, self.cursor_range, &code);
                            self.cursor_range = Some((c, c));
                        }
                        if let Some(code) = copy_req {
                            ctx.copy_text(code);
                            self.set_status("Copied LaTeX block", false);
                        }
                    });
                });

                ui.separator();
                let resp = ui.add(
                    TextEdit::multiline(&mut self.chat_input)
                        .hint_text("Ask anything about your paper...")
                        .desired_rows(3)
                        .desired_width(ui.available_width()),
                );
                if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift) {
                    self.start_chat_send(ctx);
                }
                ui.horizontal(|ui| {
                    if self.chat_streaming {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.label(RichText::new("Generating...").size(12.0).color(pal.muted));
                        if ui.button(RichText::new("Stop").color(pal.red)).clicked() {
                            self.chat_cancel.store(true, Ordering::SeqCst);
                        }
                    } else {
                        let send_btn = egui::Button::new(RichText::new("Send").color(Color32::WHITE)).fill(pal.accent);
                        if ui.add(send_btn).clicked() {
                            self.start_chat_send(ctx);
                        }
                    }
                });
            }
            AiTab::Edit => {
                ui.label(RichText::new("Describe how to rewrite or fix the current file:").size(12.5).color(pal.muted));
                ui.add_space(6.0);
                ui.add(
                    TextEdit::multiline(&mut self.ai_edit_instruction)
                        .hint_text("e.g., Fix grammar in the abstract, or tighten the introduction...")
                        .desired_rows(5)
                        .desired_width(ui.available_width()),
                );
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    let apply_btn = egui::Button::new(RichText::new(if self.ai_edit_busy { "Working..." } else { "Apply Edit (Current File)" }).color(Color32::WHITE)).fill(pal.accent);
                    if ui.add_enabled(!self.ai_edit_busy && !self.ai_edit_instruction.trim().is_empty(), apply_btn).clicked() {
                        let instr = self.ai_edit_instruction.trim().to_string();
                        self.start_ai_edit(ctx, instr, false);
                    }
                    if !self.compile_errors.is_empty() && ui.add_enabled(!self.ai_edit_busy, egui::Button::new("Fix Compile Errors")).clicked() {
                        self.start_ai_edit(ctx, "Fix all compile errors.".into(), true);
                    }
                    let debug_proj_btn = egui::Button::new(RichText::new("Debug & Fix Any Project File").color(Color32::WHITE)).fill(pal.green);
                    if ui.add_enabled(!self.chat_streaming, debug_proj_btn).clicked() {
                        let custom = self.ai_edit_instruction.trim();
                        self.chat_input = if custom.is_empty() {
                            "Inspect all project files and compile errors. Debug and output <edit file=\"...\"> blocks for every file that needs fixing.".into()
                        } else {
                            format!("{custom}\n\nInspect all project files and output <edit file=\"...\"> blocks for every file that needs updating.")
                        };
                        self.ai_tab = AiTab::Chat;
                        self.start_chat_send(ctx);
                    }
                });
            }
        }
    }

    fn ui_dialogs(&mut self, ctx: &egui::Context, pal: ThemePalette) {
        // Profile & Gemini API Key Dialog
        if self.show_profile_dlg {
            let mut open = true;
            egui::Window::new("Profile & API Keys")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_min_width(400.0);
                    egui::Grid::new("prof_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        ui.label("Author Name");
                        ui.add(TextEdit::singleline(&mut self.prof_name).desired_width(260.0));
                        ui.end_row();

                        ui.label("Affiliation");
                        ui.add(TextEdit::singleline(&mut self.prof_affil).desired_width(260.0));
                        ui.end_row();

                        ui.label("Email");
                        ui.add(TextEdit::singleline(&mut self.prof_email).desired_width(260.0));
                        ui.end_row();
                    });

                    ui.add_space(10.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Google Gemini Account Login").strong().size(14.0).color(pal.text));
                        if self.gemini_key_set {
                            ui.label(RichText::new("· Connected & Ready").size(12.5).strong().color(pal.green));
                        }
                    });
                    ui.add_space(4.0);
                    if let Some(email) = self.google_signed_in_email.clone() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("Signed in as {email}")).color(pal.green).strong());
                            if ui.button("Sign Out").clicked() {
                                sign_out_google();
                                self.google_signed_in_email = None;
                                self.gemini_key_set = get_gemini_auth().is_ok();
                                self.refresh_models();
                            }
                        });
                    } else {
                        // Option 1: Instant browser login via Google AI Studio (auto-detects copied key)
                        Frame::new()
                            .fill(pal.surface)
                            .stroke(Stroke::new(1.0, pal.line))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::same(10))
                            .show(ui, |ui| {
                                ui.label(RichText::new("Option 1: Sign in with Google AI Studio (No Cloud Setup)").strong().size(12.5).color(pal.text));
                                ui.label(RichText::new("1. Click below to sign in with Google and copy your Gemini key.\n2. inkTeX will automatically detect and activate it when copied.").size(12.0).color(pal.muted));
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    let q_btn = egui::Button::new(RichText::new(if self.clipboard_watch_until.is_some() { "Waiting for copied key..." } else { "Login via Google AI Studio" }).color(Color32::WHITE)).fill(pal.green);
                                    if ui.add(q_btn).clicked() {
                                        open_url("https://aistudio.google.com/apikey");
                                        self.start_clipboard_watch(ctx, "gemini");
                                    }
                                    if ui.button("Paste & Activate").clicked() {
                                        if let Ok(out) = std::process::Command::new("powershell").args(["-NoProfile", "-Command", "Get-Clipboard"]).output() {
                                            let clip = String::from_utf8_lossy(&out.stdout).trim().to_string();
                                            if !clip.is_empty() {
                                                self.gemini_key_input = clip.clone();
                                                if let Err(e) = save_gemini_key(&clip) {
                                                    self.profile_err = e;
                                                } else {
                                                    self.gemini_key_set = true;
                                                    self.clipboard_watch_until = None;
                                                    self.refresh_models();
                                                    self.profile_err.clear();
                                                    self.set_status("Gemini key activated from clipboard!", false);
                                                }
                                            } else {
                                                self.profile_err = "Clipboard is empty. Copy your key from AI Studio first.".into();
                                            }
                                        }
                                    }
                                });
                            });

                        ui.add_space(8.0);

                        // Option 2: Full Google OAuth 2.0 (for Google Drive Sync + Gemini)
                        Frame::new()
                            .fill(pal.surface)
                            .stroke(Stroke::new(1.0, pal.line))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::same(10))
                            .show(ui, |ui| {
                                ui.label(RichText::new("Option 2: Google OAuth 2.0 (For Google Drive Sync)").strong().size(12.5).color(pal.text));
                                egui::Grid::new("oauth_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                                    ui.label("OAuth Client ID:");
                                    ui.horizontal(|ui| {
                                        ui.add(TextEdit::singleline(&mut self.oauth_client_id).hint_text("...apps.googleusercontent.com").desired_width(210.0));
                                        if ui.small_button("Create ID").on_hover_text("Create a Desktop OAuth Client ID in Google Cloud Console").clicked() {
                                            let _ = std::process::Command::new("cmd").args(["/C", "start", "", "https://console.cloud.google.com/apis/credentials/oauthclient"]).spawn();
                                        }
                                    });
                                    ui.end_row();

                                    ui.label("Client Secret:");
                                    ui.add(TextEdit::singleline(&mut self.oauth_client_secret).password(true).hint_text("Paste Client Secret (if issued by Google)").desired_width(210.0));
                                    ui.end_row();
                                });
                                ui.add_space(4.0);
                                let login_btn = egui::Button::new(RichText::new(if self.google_login_busy { "Waiting for Google Login..." } else { "Sign in with Google OAuth" }).color(Color32::WHITE)).fill(pal.accent);
                                if ui.add_enabled(!self.google_login_busy, login_btn).clicked() {
                                    if self.oauth_client_id.trim().is_empty() {
                                        self.profile_err = "Google OAuth requires a Desktop Client ID from Google Cloud Console (click 'Create ID' -> Desktop app -> paste ID & Secret above).".into();
                                    } else {
                                        let (cid, csec) = (self.oauth_client_id.clone(), self.oauth_client_secret.clone());
                                        self.start_google_login(ctx, cid, csec);
                                    }
                                }
                            });
                    }

                    ui.add_space(6.0);
                    ui.collapsing("Manual API Key", |ui| {
                        egui::Grid::new("key_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                            ui.label("Gemini API Key");
                            ui.horizontal(|ui| {
                                ui.add(
                                    TextEdit::singleline(&mut self.gemini_key_input)
                                        .password(!self.gemini_key_show)
                                        .hint_text("AIzaSy... or AQ...")
                                        .desired_width(185.0),
                                );
                                if ui.small_button(if self.gemini_key_show { "Hide" } else { "Show" }).clicked() {
                                    self.gemini_key_show = !self.gemini_key_show;
                                }
                            });
                            ui.end_row();
                        });
                    });

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Claude: {}", if self.claude_key_set { "Connected" } else { "Not connected" })).size(12.0).color(pal.muted));
                        if self.claude_key_set {
                            if ui.small_button("Sign out").clicked() {
                                sign_out_claude();
                                self.claude_key_set = get_claude_key().is_ok(); // stays on if ANTHROPIC_API_KEY is set
                                self.refresh_models();
                            }
                        } else if ui.small_button("Sign in...").clicked() {
                            self.show_profile_dlg = false;
                            self.show_login = true;
                        }
                    });
                    ui.label(RichText::new(format!("Compiler: {}", self.compiler_desc)).size(12.0).color(pal.muted));

                    if !self.profile_err.is_empty() {
                        ui.label(RichText::new(&self.profile_err).color(pal.red).size(12.5));
                    }

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let save_btn = egui::Button::new(RichText::new("Save & Activate").color(Color32::WHITE)).fill(pal.green);
                        if ui.add(save_btn).clicked() {
                            let k = self.gemini_key_input.trim().to_string();
                            if !k.is_empty() {
                                if let Err(e) = save_gemini_key(&k) {
                                    self.profile_err = e;
                                    return;
                                }
                                self.gemini_key_set = true;
                            }
                            match save_profile(&json!({
                                "name": self.prof_name.trim(),
                                "affiliation": self.prof_affil.trim(),
                                "email": self.prof_email.trim(),
                                "theme": self.theme.id(),
                                "google_client_id": self.oauth_client_id.trim(),
                                "google_client_secret": self.oauth_client_secret.trim(),
                            })) {
                                Ok(_) => {
                                    self.refresh_models();
                                    self.show_profile_dlg = false;
                                    self.set_status("Settings & API key saved", false);
                                }
                                Err(e) => self.profile_err = e,
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_profile_dlg = false;
                        }
                    });
                });
            if !open {
                self.show_profile_dlg = false;
            }
        }

        // New Project Dialog
        if self.show_new_proj_dlg {
            let mut open = true;
            egui::Window::new("New Project")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Project Name");
                    ui.add(TextEdit::singleline(&mut self.new_proj_name).hint_text("e.g., Thesis Paper").desired_width(280.0));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.new_proj_template, "blank".to_string(), "Blank Paper");
                        ui.radio_value(&mut self.new_proj_template, "resume".to_string(), "Resume / CV");
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let create_btn = egui::Button::new(RichText::new("Create").color(Color32::WHITE)).fill(pal.accent);
                        if ui.add(create_btn).clicked() {
                            match new_project(&self.new_proj_name, &self.new_proj_template) {
                                Ok(n) => {
                                    log_event(&n, "New project", true, &self.new_proj_template);
                                    self.projects = list_projects();
                                    self.show_new_proj_dlg = false;
                                    self.open_project_by_name(ctx, &n);
                                }
                                Err(e) => self.set_status(e, true),
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_new_proj_dlg = false;
                        }
                    });
                });
            if !open {
                self.show_new_proj_dlg = false;
            }
        }

        // Confirm Delete Project
        if let Some(proj_name) = self.confirm_delete_proj.clone() {
            egui::Window::new("Delete Project")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!("Permanently delete '{proj_name}'?"));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Delete").color(pal.red)).clicked() {
                            if let Ok(dir) = project_dir(&proj_name) {
                                let _ = fs::remove_dir_all(dir);
                                self.projects = list_projects();
                                self.set_status(format!("Deleted {proj_name}"), false);
                            }
                            self.confirm_delete_proj = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_delete_proj = None;
                        }
                    });
                });
        }

        // New File Dialog
        if self.show_new_file_dlg {
            let mut open = true;
            egui::Window::new("New File")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Relative path (e.g., chapters/intro.tex)");
                    ui.add(TextEdit::singleline(&mut self.new_file_name).desired_width(260.0));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Create").clicked() {
                            if let Some(root) = &self.open_project_root {
                                match safe_path(root, self.new_file_name.trim()) {
                                    Ok(p) => {
                                        if let Some(parent) = p.parent() {
                                            let _ = fs::create_dir_all(parent);
                                        }
                                        if fs::write(&p, "").is_ok() {
                                            let rel = self.new_file_name.trim().to_string();
                                            self.refresh_project_files();
                                            self.open_file(ctx, &rel);
                                            self.show_new_file_dlg = false;
                                        }
                                    }
                                    Err(e) => self.set_status(e, true),
                                }
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_new_file_dlg = false;
                        }
                    });
                });
            if !open {
                self.show_new_file_dlg = false;
            }
        }

        // Rename File Dialog
        if let Some(target) = self.rename_file_target.clone() {
            egui::Window::new("Rename File")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.add(TextEdit::singleline(&mut self.rename_file_input).desired_width(260.0));
                    ui.horizontal(|ui| {
                        if ui.button("Rename").clicked() {
                            if let Some(root) = &self.open_project_root {
                                if let (Ok(src), Ok(dst)) = (safe_path(root, &target), safe_path(root, self.rename_file_input.trim())) {
                                    if let Some(p) = dst.parent() {
                                        let _ = fs::create_dir_all(p);
                                    }
                                    if fs::rename(src, dst).is_ok() {
                                        if self.current_file == target {
                                            self.current_file = self.rename_file_input.trim().to_string();
                                        }
                                        self.refresh_project_files();
                                    }
                                }
                            }
                            self.rename_file_target = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.rename_file_target = None;
                        }
                    });
                });
        }

        // Confirm Delete File
        if let Some(target) = self.confirm_delete_file.clone() {
            egui::Window::new("Delete File")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(format!("Delete '{target}'?"));
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Delete").color(pal.red)).clicked() {
                            if let Some(root) = &self.open_project_root {
                                if let Ok(p) = safe_path(root, &target) {
                                    let _ = fs::remove_file(p);
                                    if self.current_file == target {
                                        self.current_file.clear();
                                        self.code_text.clear();
                                        self.saved_text.clear();
                                    }
                                    self.refresh_project_files();
                                }
                            }
                            self.confirm_delete_file = None;
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_delete_file = None;
                        }
                    });
                });
        }

        // Activity Logs Dialog
        if self.show_logs_dlg {
            let mut open = true;
            egui::Window::new("Activity Logs")
                .open(&mut open)
                .default_size([560.0, 380.0])
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ScrollArea::vertical().show(ui, |ui| {
                        for entry in &self.logs_cache {
                            let proj = entry["project"].as_str().unwrap_or("");
                            let action = entry["action"].as_str().unwrap_or("");
                            let ok = entry["ok"].as_bool().unwrap_or(true);
                            let detail = entry["detail"].as_str().unwrap_or("");
                            ui.horizontal(|ui| {
                                let col = if ok { pal.green } else { pal.red };
                                ui.label(RichText::new(if ok { "OK" } else { "ERR" }).size(11.0).color(col).strong());
                                ui.label(RichText::new(format!("[{proj}] {action}: {detail}")).size(12.5));
                            });
                        }
                    });
                });
            if !open {
                self.show_logs_dlg = false;
            }
        }

        // Insert Image Dialog
        if self.show_image_dlg {
            let mut open = true;
            egui::Window::new("Insert Figure")
                .open(&mut open)
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Image file:");
                        ui.add(TextEdit::singleline(&mut self.img_dlg_file).desired_width(180.0));
                        if ui.button("Browse & Upload").clicked() {
                            if let Some(p) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "pdf", "eps"]).pick_file() {
                                if let (Some(root), Ok(bytes)) = (&self.open_project_root, fs::read(&p)) {
                                    let fname = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                                    if let Ok(rel) = save_asset(root, &fname, &bytes) {
                                        self.img_dlg_file = rel;
                                        self.refresh_project_files();
                                    }
                                }
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Width:");
                        ui.add(TextEdit::singleline(&mut self.img_dlg_width).desired_width(140.0));
                        ui.label("Position:");
                        ui.add(TextEdit::singleline(&mut self.img_dlg_pos).desired_width(60.0));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Caption:");
                        ui.add(TextEdit::singleline(&mut self.img_dlg_caption).desired_width(240.0));
                    });
                    ui.add_space(8.0);
                    if ui.button("Insert").clicked() && !self.img_dlg_file.trim().is_empty() {
                        ensure_package(&mut self.code_text, "graphicx");
                        let cap = if self.img_dlg_caption.trim().is_empty() {
                            String::new()
                        } else {
                            format!("\n  \\caption{{{}}}", self.img_dlg_caption.trim())
                        };
                        let snippet = format!(
                            "\\begin{{figure}}[{}]\n  \\centering\n  \\includegraphics[width={}]{{{}}}{}\n\\end{{figure}}\n",
                            self.img_dlg_pos.trim(),
                            self.img_dlg_width.trim(),
                            self.img_dlg_file.trim(),
                            cap
                        );
                        let c = insert_at_cursor(&mut self.code_text, self.cursor_range, &snippet);
                        self.cursor_range = Some((c, c));
                        self.show_image_dlg = false;
                    }
                });
            if !open {
                self.show_image_dlg = false;
            }
        }

        // Insert Table Dialog
        if self.show_table_dlg {
            let mut open = true;
            egui::Window::new("Insert Table")
                .open(&mut open)
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Rows:");
                        ui.add(egui::DragValue::new(&mut self.tbl_rows).range(1..=30));
                        ui.label("Cols:");
                        ui.add(egui::DragValue::new(&mut self.tbl_cols).range(1..=15));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Caption:");
                        ui.add(TextEdit::singleline(&mut self.tbl_caption).desired_width(220.0));
                    });
                    if ui.button("Insert").clicked() {
                        let col_spec = self.tbl_align.repeat(self.tbl_cols);
                        let header = (1..=self.tbl_cols).map(|c| format!("Col {c}")).collect::<Vec<_>>().join(" & ");
                        let row = vec!["cell"; self.tbl_cols].join(" & ");
                        let mut body = String::new();
                        for _ in 0..self.tbl_rows {
                            body.push_str(&format!("    {row} \\\\\n"));
                        }
                        let cap = if self.tbl_caption.trim().is_empty() {
                            String::new()
                        } else {
                            format!("  \\caption{{{}}}\n", self.tbl_caption.trim())
                        };
                        let snippet = format!(
                            "\\begin{{table}}[htbp]\n  \\centering\n{cap}  \\begin{{tabular}}{{{col_spec}}}\n    \\hline\n    {header} \\\\\n    \\hline\n{body}    \\hline\n  \\end{{tabular}}\n\\end{{table}}\n"
                        );
                        let c = insert_at_cursor(&mut self.code_text, self.cursor_range, &snippet);
                        self.cursor_range = Some((c, c));
                        self.show_table_dlg = false;
                    }
                });
            if !open {
                self.show_table_dlg = false;
            }
        }

        // NN Diagram Dialog
        if self.show_nn_dlg {
            let mut open = true;
            egui::Window::new("Neural Network Diagram (PlotNeuralNet)")
                .open(&mut open)
                .default_size([460.0, 320.0])
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.add(TextEdit::singleline(&mut self.nn_name).desired_width(150.0));
                        ui.label("Caption:");
                        ui.add(TextEdit::singleline(&mut self.nn_caption).desired_width(180.0));
                    });
                    ui.add(
                        TextEdit::multiline(&mut self.nn_spec)
                            .font(FontId::monospace(12.5))
                            .desired_rows(10)
                            .desired_width(440.0),
                    );
                    ui.add_space(6.0);
                    let btn = egui::Button::new(if self.nn_busy { "Building..." } else { "Build & Insert" });
                    if ui.add_enabled(!self.nn_busy, btn).clicked() {
                        if let Some(root) = self.open_project_root.clone() {
                            self.nn_busy = true;
                            let name = self.nn_name.clone();
                            let spec = self.nn_spec.clone();
                            let caption = self.nn_caption.clone();
                            let tx = self.bg_tx.clone();
                            let ctx_clone = ctx.clone();
                            thread::spawn(move || {
                                let res = make_diagram(&root, &name, &spec)
                                    .map(|v| (v["pdf"].as_str().unwrap_or("diagrams/network.pdf").to_string(), caption));
                                let _ = tx.send(BgResult::DiagramBuilt(res));
                                ctx_clone.request_repaint();
                            });
                        }
                    }
                });
            if !open {
                self.show_nn_dlg = false;
            }
        }

        // Symbols Picker Dialog
        if self.show_symbols_dlg {
            let mut open = true;
            egui::Window::new("LaTeX Symbols & Structures")
                .open(&mut open)
                .default_size([420.0, 260.0])
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    const SYMS: &[(&str, &str)] = &[
                        ("Frac", "\\frac{a}{b}"),
                        ("Sqrt", "\\sqrt{x}"),
                        ("Sum", "\\sum_{i=1}^{n}"),
                        ("Int", "\\int_{a}^{b}"),
                        ("α", "\\alpha"),
                        ("β", "\\beta"),
                        ("γ", "\\gamma"),
                        ("δ", "\\delta"),
                        ("θ", "\\theta"),
                        ("λ", "\\lambda"),
                        ("μ", "\\mu"),
                        ("σ", "\\sigma"),
                        ("Ω", "\\Omega"),
                        ("≤", "\\leq"),
                        ("≥", "\\geq"),
                        ("≠", "\\neq"),
                        ("≈", "\\approx"),
                        ("∈", "\\in"),
                        ("∀", "\\forall"),
                        ("∃", "\\exists"),
                        ("→", "\\rightarrow"),
                        ("⇒", "\\Rightarrow"),
                        ("∞", "\\infty"),
                        ("∂", "\\partial"),
                        ("∇", "\\nabla"),
                    ];
                    ui.horizontal_wrapped(|ui| {
                        for (lbl, tex) in SYMS {
                            if ui.button(*lbl).on_hover_text(*tex).clicked() {
                                let c = insert_at_cursor(&mut self.code_text, self.cursor_range, tex);
                                self.cursor_range = Some((c, c));
                            }
                        }
                    });
                });
            if !open {
                self.show_symbols_dlg = false;
            }
        }

        // Help Me Write Dialog
        if self.show_hw_dlg {
            let mut open = true;
            egui::Window::new("Help me write (Ctrl+K)")
                .open(&mut open)
                .default_size([440.0, 220.0])
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.add(
                        TextEdit::singleline(&mut self.hw_prompt)
                            .hint_text("e.g., Write an abstract summarizing the methodology...")
                            .desired_width(420.0),
                    );
                    ui.add_space(6.0);
                    let gen_btn = egui::Button::new(if self.hw_busy { "Writing..." } else { "Generate" });
                    if ui.add_enabled(!self.hw_busy && !self.hw_prompt.trim().is_empty(), gen_btn).clicked() {
                        self.start_help_me_write(ctx);
                    }
                    if let Some(res) = self.hw_result.clone() {
                        ui.separator();
                        ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                            ui.label(RichText::new(&res).font(FontId::monospace(12.5)));
                        });
                        ui.horizontal(|ui| {
                            if ui.button("Insert / Replace").clicked() {
                                let c = insert_at_cursor(&mut self.code_text, self.cursor_range, &res);
                                self.cursor_range = Some((c, c));
                                self.show_hw_dlg = false;
                            }
                            if ui.button("Discard").clicked() {
                                self.hw_result = None;
                            }
                        });
                    }
                });
            if !open {
                self.show_hw_dlg = false;
            }
        }

        // Google Drive Live Sync & Share Dialog
        if self.show_share_dlg {
            let mut open = true;
            egui::Window::new("Google Drive Live Sync & Share")
                .open(&mut open)
                .default_size([460.0, 320.0])
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    if self.google_signed_in_email.is_none() {
                        ui.label(RichText::new("Sign in with your Google Account in Settings & Key to enable Google Drive Sync & Sharing.").color(pal.amber));
                        if ui.button("Open Settings & Login").clicked() {
                            self.show_share_dlg = false;
                            self.show_profile_dlg = true;
                        }
                    } else {
                        ui.horizontal(|ui| {
                            let sync_btn = egui::Button::new(RichText::new(if self.drive_syncing { "Syncing..." } else { "Sync Now" }).color(Color32::WHITE)).fill(pal.accent);
                            if ui.add_enabled(!self.drive_syncing, sync_btn).clicked() {
                                self.trigger_drive_sync(ctx);
                            }
                            ui.checkbox(&mut self.live_sync, "Live Auto-Sync (every 12s)");
                        });
                        if !self.drive_share_url.is_empty() {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label("Drive Folder:");
                                ui.add(TextEdit::singleline(&mut self.drive_share_url).desired_width(270.0));
                                if ui.small_button("Copy").clicked() {
                                    ctx.copy_text(self.drive_share_url.clone());
                                    self.set_status("Copied Drive link", false);
                                }
                            });
                        }
                        ui.separator();
                        ui.label(RichText::new("Share with Collaborator").strong().color(pal.text));
                        ui.horizontal(|ui| {
                            ui.add(TextEdit::singleline(&mut self.drive_collab_email).hint_text("collaborator@gmail.com (or blank for link)").desired_width(220.0));
                            egui::ComboBox::from_id_salt("drive_role")
                                .selected_text(if self.drive_collab_role == "writer" { "Can Edit" } else { "View Only" })
                                .width(90.0)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.drive_collab_role, "writer".into(), "Can Edit");
                                    ui.selectable_value(&mut self.drive_collab_role, "reader".into(), "View Only");
                                });
                            if ui.button("Share").clicked() {
                                if self.drive_folder_id.is_empty() {
                                    self.drive_msg = "Click Sync Now first to create the Drive folder.".into();
                                } else {
                                    let fid = self.drive_folder_id.clone();
                                    let em = self.drive_collab_email.clone();
                                    let role = self.drive_collab_role.clone();
                                    let tx = self.bg_tx.clone();
                                    let ctx_c = ctx.clone();
                                    thread::spawn(move || {
                                        let res = share_drive_folder(&fid, &em, &role);
                                        let _ = tx.send(BgResult::DriveShared(res));
                                        ctx_c.request_repaint();
                                    });
                                }
                            }
                        });
                        ui.separator();
                        ui.label(RichText::new("Connect to a Collaborator's Shared Folder").strong().color(pal.text));
                        ui.horizontal(|ui| {
                            ui.add(TextEdit::singleline(&mut self.drive_link_input).hint_text("Paste Google Drive folder URL or ID...").desired_width(290.0));
                            if ui.button("Connect & Pull").clicked() {
                                if let Some(root) = &self.open_project_root {
                                    match link_drive_folder(root, &self.drive_link_input) {
                                        Ok(fid) => {
                                            self.drive_folder_id = fid;
                                            self.trigger_drive_sync(ctx);
                                        }
                                        Err(e) => self.drive_msg = e,
                                    }
                                }
                            }
                        });
                    }
                    if !self.drive_msg.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new(&self.drive_msg).size(12.0).color(pal.muted));
                    }
                });
            if !open {
                self.show_share_dlg = false;
            }
        }
    }
}

fn find_compiler_desc() -> String {
    match crate::backend::find_compiler("main.tex") {
        Some((p, _)) => p.to_string_lossy().to_string(),
        None => "Built-in Tectonic (downloaded on first compile)".into(),
    }
}

fn file_badge_label(path: &str) -> &'static str {
    let l = path.to_lowercase();
    if l.ends_with(".tex") || l.ends_with(".sty") || l.ends_with(".cls") {
        "TEX"
    } else if l.ends_with(".bib") {
        "BIB"
    } else if l.ends_with(".pdf") {
        "PDF"
    } else if l.ends_with(".png") || l.ends_with(".jpg") || l.ends_with(".jpeg") || l.ends_with(".eps") {
        "IMG"
    } else {
        "TXT"
    }
}

/// Custom-painted geometric line chevron (no 3D emojis, no play/pause triangle look).
const LOGIN_SKIPPED: &str = "login_skipped";
const TEXT_SCALE_ID: &str = "ui_text_scale";
const ICON_SCALE_ID: &str = "ui_icon_scale";
const TEXT_SIZES: [(&str, f32); 4] = [("Small", 0.9), ("Default", 1.0), ("Large", 1.1), ("Extra large", 1.2)];
const ICON_SIZES: [(&str, f32); 3] = [("Small", 0.8), ("Medium", 1.0), ("Large", 1.3)];

/// Which provider a copied API key belongs to, by its well-known prefix.
fn key_kind(s: &str) -> Option<&'static str> {
    let s = s.trim();
    if s.len() < 20 || s.contains(char::is_whitespace) {
        None
    } else if s.starts_with("sk-ant-") {
        Some("claude")
    } else if s.starts_with("AIza") || s.starts_with("AQ.") {
        Some("gemini")
    } else {
        None
    }
}

/// Run a console tool without flashing a console window (the app is a GUI-subsystem exe).
fn quiet_cmd(program: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new(program);
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c
}

fn read_clipboard() -> String {
    quiet_cmd("powershell")
        .args(["-NoProfile", "-Command", "Get-Clipboard"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn open_url(url: &str) {
    let _ = quiet_cmd("cmd").args(["/C", "start", "", url]).spawn();
}

/// Full-width sign-in row: brand letter badge, label, and a status on the right.
#[allow(clippy::too_many_arguments)]
fn provider_button(ui: &mut Ui, pal: ThemePalette, w: f32, letter: &str, brand: Color32, label: &str, status: &str, connected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 44.0), if connected { Sense::hover() } else { Sense::click() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, !connected, label));
    let hovered = resp.hovered() && !connected;
    let fill = if hovered { pal.surface_hover } else { pal.glass_card_color() };
    let border = if connected { pal.green.gamma_multiply(0.7) } else if hovered { pal.accent.gamma_multiply(0.6) } else { pal.line };
    let p = ui.painter();
    p.rect(rect, 10.0, fill, Stroke::new(1.0, border), egui::StrokeKind::Inside);
    let badge = egui::pos2(rect.left() + 26.0, rect.center().y);
    p.circle_filled(badge, 12.0, brand);
    p.text(badge, egui::Align2::CENTER_CENTER, letter, FontId::proportional(13.0), Color32::WHITE);
    p.text(egui::pos2(rect.left() + 48.0, rect.center().y), egui::Align2::LEFT_CENTER, label, FontId::proportional(14.0), pal.text);
    if !status.is_empty() {
        let color = if connected { pal.green } else { pal.muted };
        p.text(egui::pos2(rect.right() - 14.0, rect.center().y), egui::Align2::RIGHT_CENTER, status, FontId::proportional(11.5), color);
    }
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

#[cfg(test)]
mod login_tests {
    use super::key_kind;

    #[test]
    fn detects_key_provider_by_prefix() {
        assert_eq!(key_kind("sk-ant-api03-abcdefghijklmnop"), Some("claude"));
        assert_eq!(key_kind("AIzaSyAbcdefghijklmnopqrstu"), Some("gemini"));
        assert_eq!(key_kind("AQ.Abcdefghijklmnopqrstuvw"), Some("gemini"));
        assert_eq!(key_kind("sk-ant-short"), None);
        assert_eq!(key_kind("just some copied sentence here"), None);
        assert_eq!(key_kind("sk-ant-api03 with spaces in it"), None);
    }
}

const HOME_SEARCH_HINT: &str = "Search projects";
const AUTOFIX_ID: &str = "ai_autofix";
const AUTOFIX_MAX: u8 = 3;
const PINNED_ID: &str = "pinned_projects";

/// Search field: solid fill, visible border with an accent focus ring, magnifier, clear button and a Ctrl+F hint.
fn search_box(ui: &mut Ui, pal: ThemePalette, text: &mut String, hint: &str, width: f32, height: f32) -> egui::Response {
    let id = egui::Id::new(("search_box", hint));
    let focused = ui.memory(|m| m.has_focus(id));
    let border = if focused { Stroke::new(1.5, pal.accent) } else { Stroke::new(1.0, pal.line) };
    Frame::new()
        .fill(pal.panel)
        .stroke(border)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin { left: 34, right: 8, top: 0, bottom: 0 })
        .show(ui, |ui| {
            ui.set_height(height - 2.0);
            ui.set_width(width - 42.0);
            ui.horizontal_centered(|ui| {
                let trailing = if text.is_empty() { 56.0 } else { 26.0 };
                let resp = ui.add(
                    TextEdit::singleline(text)
                        .id(id)
                        .hint_text(hint)
                        .frame(Frame::NONE)
                        .font(FontId::proportional(14.0))
                        .desired_width(width - 42.0 - trailing),
                );
                // magnifier in the left padding
                let c = egui::pos2(resp.rect.left() - 19.0, resp.rect.center().y - 1.0);
                let st = Stroke::new(1.5, if focused { pal.accent } else { pal.muted });
                ui.painter().circle_stroke(c, 5.5, st);
                ui.painter().line_segment([c + Vec2::splat(4.0), c + Vec2::splat(8.0)], st);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if !text.is_empty() {
                        if ui.add(egui::Button::new(RichText::new("×").size(16.0).color(pal.muted)).frame(false)).on_hover_text("Clear search").clicked() {
                            text.clear();
                            ui.memory_mut(|m| m.request_focus(id));
                        }
                    } else if !focused {
                        ui.label(RichText::new(" Ctrl+F ").size(11.0).color(pal.muted).background_color(pal.surface));
                    }
                });
                resp
            })
            .inner
        })
        .inner
}

/// A captioned group of ribbon commands (like Word's "Font" or "Paragraph"), followed by a divider.
fn ribbon_group(ui: &mut Ui, pal: ThemePalette, caption: &str, add: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.horizontal(add);
        ui.label(RichText::new(caption).size(10.5).color(pal.muted));
    });
    ui.separator();
}

const SORT_NAME: u8 = 0;
const SORT_DATE: u8 = 1;
const SORT_BUILDS: u8 = 2;

fn sort_label(k: u8) -> &'static str {
    match k {
        SORT_NAME => "Name",
        SORT_DATE => "Date modified",
        _ => "Builds",
    }
}

/// (name, modified unix secs, build count); ties fall back to name so the order is stable.
fn sort_projects(items: &mut [(String, u64, usize)], key: u8, asc: bool) {
    items.sort_by(|a, b| {
        let by_name = a.0.to_lowercase().cmp(&b.0.to_lowercase());
        let o = match key {
            SORT_NAME => by_name,
            SORT_DATE => a.1.cmp(&b.1).then(by_name),
            _ => a.2.cmp(&b.2).then(by_name),
        };
        if asc { o } else { o.reverse() }
    });
}

/// List-view column rects: Name 55%, Date modified 25%, Builds the rest (minus room for the hover Delete).
fn list_col(row: egui::Rect, i: usize) -> egui::Rect {
    let w = row.width();
    let (a, b) = match i {
        0 => (0.0, 0.55),
        1 => (0.55, 0.80),
        _ => (0.80, 1.0),
    };
    let left = row.left() + w * a;
    let right = if i == 2 { row.left() + w * b - 80.0 } else { row.left() + w * b };
    egui::Rect::from_x_y_ranges(left..=right.max(left), row.y_range())
}

#[cfg(test)]
mod home_sort_tests {
    use super::*;

    #[test]
    fn sorts_by_each_key_and_direction() {
        let mut v = vec![("beta".to_string(), 30, 1), ("Alpha".to_string(), 10, 5), ("gamma".to_string(), 20, 1)];
        sort_projects(&mut v, SORT_NAME, true);
        assert_eq!(v.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["Alpha", "beta", "gamma"]);
        sort_projects(&mut v, SORT_DATE, false);
        assert_eq!(v.iter().map(|x| x.1).collect::<Vec<_>>(), [30, 20, 10]);
        sort_projects(&mut v, SORT_BUILDS, true);
        assert_eq!(v.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["beta", "gamma", "Alpha"]);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum WinBtn {
    Min,
    Max,
    Restore,
    Close,
}

/// Caption button for the frameless window, painted (no glyph-font dependency), Windows-sized.
fn window_button(ui: &mut Ui, kind: WinBtn, pal: ThemePalette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(46.0, ui.available_height()), Sense::click());
    let label = match kind {
        WinBtn::Min => "Minimize",
        WinBtn::Max => "Maximize",
        WinBtn::Restore => "Restore",
        WinBtn::Close => "Close",
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let (bg, fg) = match (kind, resp.hovered()) {
        (WinBtn::Close, true) => (Color32::from_rgb(196, 43, 28), Color32::WHITE),
        (_, true) => (pal.surface_hover, pal.text),
        _ => (Color32::TRANSPARENT, pal.muted),
    };
    let p = ui.painter();
    p.rect_filled(rect, 0.0, bg);
    let c = rect.center();
    let st = Stroke::new(1.0, fg);
    let s = 5.0;
    match kind {
        WinBtn::Min => {
            p.line_segment([egui::pos2(c.x - s, c.y), egui::pos2(c.x + s, c.y)], st);
        }
        WinBtn::Max => {
            p.rect_stroke(egui::Rect::from_center_size(c, Vec2::splat(2.0 * s)), 1.0, st, egui::StrokeKind::Inside);
        }
        WinBtn::Restore => {
            let back = egui::Rect::from_center_size(c + Vec2::new(2.0, -2.0), Vec2::splat(2.0 * s - 2.0));
            let front = egui::Rect::from_center_size(c + Vec2::new(-1.0, 1.0), Vec2::splat(2.0 * s - 2.0));
            p.rect_stroke(back, 1.0, st, egui::StrokeKind::Inside);
            p.rect_filled(front, 1.0, if bg == Color32::TRANSPARENT { pal.panel } else { bg });
            p.rect_stroke(front, 1.0, st, egui::StrokeKind::Inside);
        }
        WinBtn::Close => {
            p.line_segment([c + Vec2::new(-s, -s), c + Vec2::new(s, s)], st);
            p.line_segment([c + Vec2::new(-s, s), c + Vec2::new(s, -s)], st);
        }
    }
    resp
}

/// Frameless window: thin border plus edge/corner resize (winit gives an undecorated window no resize borders).
fn window_edges(ctx: &egui::Context, pal: ThemePalette) {
    use egui::{CursorIcon as C, ResizeDirection as D};
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
        return;
    }
    let screen = ctx.content_rect();
    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("win_border")))
        .rect_stroke(screen, 0.0, Stroke::new(1.0, pal.line), egui::StrokeKind::Inside);

    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
    const M: f32 = 5.0;
    let (l, r) = (pos.x - screen.left() < M, screen.right() - pos.x < M);
    let (t, b) = (pos.y - screen.top() < M, screen.bottom() - pos.y < M);
    let (dir, icon) = match (l, r, t, b) {
        (true, _, true, _) => (D::NorthWest, C::ResizeNorthWest),
        (_, true, true, _) => (D::NorthEast, C::ResizeNorthEast),
        (true, _, _, true) => (D::SouthWest, C::ResizeSouthWest),
        (_, true, _, true) => (D::SouthEast, C::ResizeSouthEast),
        (true, _, _, _) => (D::West, C::ResizeWest),
        (_, true, _, _) => (D::East, C::ResizeEast),
        (_, _, true, _) => (D::North, C::ResizeNorth),
        (_, _, _, true) => (D::South, C::ResizeSouth),
        _ => return,
    };
    ctx.set_cursor_icon(icon);
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
    }
}

/// Like `ui.put`, but left-aligned and vertically centred (ui.put centres horizontally).
fn put_left(ui: &mut Ui, rect: egui::Rect, w: impl egui::Widget) -> egui::Response {
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)), |ui| ui.add(w)).inner
}

/// Document glyph with a folded corner, used by the home list and tile views.
fn paint_file_icon(painter: &egui::Painter, rect: egui::Rect, color: Color32, label: Option<&str>) {
    let fold = rect.width() * 0.3;
    let body = [
        rect.left_top(),
        egui::pos2(rect.right() - fold, rect.top()),
        egui::pos2(rect.right(), rect.top() + fold),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    painter.add(egui::Shape::convex_polygon(body.to_vec(), color.gamma_multiply(0.18), Stroke::new(1.0, color.gamma_multiply(0.8))));
    painter.line_segment([egui::pos2(rect.right() - fold, rect.top()), egui::pos2(rect.right() - fold, rect.top() + fold)], Stroke::new(1.0, color.gamma_multiply(0.8)));
    painter.line_segment([egui::pos2(rect.right() - fold, rect.top() + fold), egui::pos2(rect.right(), rect.top() + fold)], Stroke::new(1.0, color.gamma_multiply(0.8)));
    if let Some(t) = label {
        painter.text(
            egui::pos2(rect.center().x, rect.bottom() - rect.height() * 0.3),
            egui::Align2::CENTER_CENTER,
            t,
            FontId::monospace((rect.width() * 0.26).max(7.0)),
            color,
        );
    }
}

fn paint_chevron(ui: &Ui, rect: egui::Rect, expanded: bool, color: Color32) {
    let c = rect.center();
    let stroke = Stroke::new(1.5, color);
    let pts = if expanded {
        // Downward chevron (⌄)
        [
            egui::pos2(c.x - 3.5, c.y - 2.0),
            egui::pos2(c.x, c.y + 2.0),
            egui::pos2(c.x + 3.5, c.y - 2.0),
        ]
    } else {
        // Rightward chevron (›)
        [
            egui::pos2(c.x - 2.0, c.y - 3.5),
            egui::pos2(c.x + 2.0, c.y),
            egui::pos2(c.x - 2.0, c.y + 3.5),
        ]
    };
    ui.painter().line_segment([pts[0], pts[1]], stroke);
    ui.painter().line_segment([pts[1], pts[2]], stroke);
}

fn paint_doc_badge(ui: &mut Ui, color: Color32, text: &str) {
    Frame::new()
        .fill(color.linear_multiply(0.16))
        .stroke(Stroke::new(1.0, color.linear_multiply(0.5)))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(4, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).font(FontId::monospace(9.5)).strong().color(color));
        });
}

fn format_rel_time(unix_secs: u64) -> String {
    if unix_secs == 0 {
        return "Just now".into();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let diff = now.saturating_sub(unix_secs);
    if diff < 60 {
        "Just now".into()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

fn extract_latex_block(text: &str) -> Option<String> {
    let start = text.find("```latex")?;
    let after = &text[start + "```latex".len()..];
    let after = after.strip_prefix('\r').unwrap_or(after);
    let after = after.strip_prefix('\n').unwrap_or(after);
    let end = after.find("```")?;
    Some(after[..end].trim_end().to_string())
}

/// Render chat text with real bold (**text**), italic (*text*), inline code (`code`), headings, and bullets.
fn render_chat_markdown(ui: &mut Ui, text: &str, pal: ThemePalette) {
    use eframe::egui::text::{LayoutJob, TextFormat};
    use eframe::egui::FontFamily;

    let mut job = LayoutJob::default();
    let base_sz = 13.0;
    let normal_fmt = TextFormat {
        font_id: FontId::new(base_sz, FontFamily::Proportional),
        color: pal.text,
        ..Default::default()
    };
    let bold_fmt = TextFormat {
        font_id: FontId::new(base_sz, FontFamily::Name("Bold".into())),
        color: pal.text,
        ..Default::default()
    };
    let italic_fmt = TextFormat {
        font_id: FontId::new(base_sz, FontFamily::Proportional),
        color: pal.text,
        italics: true,
        ..Default::default()
    };
    let code_fmt = TextFormat {
        font_id: FontId::new(12.2, FontFamily::Monospace),
        color: pal.syn_cmd,
        background: pal.surface,
        ..Default::default()
    };
    let heading_fmt = TextFormat {
        font_id: FontId::new(14.5, FontFamily::Proportional),
        color: pal.accent,
        ..Default::default()
    };

    let mut in_fence = false;
    for (idx, line) in text.split('\n').enumerate() {
        if idx > 0 {
            job.append("\n", 0.0, normal_fmt.clone());
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            job.append(line, 0.0, code_fmt.clone());
            continue;
        }
        if let Some(h) = trimmed.strip_prefix("### ").or_else(|| trimmed.strip_prefix("## ")).or_else(|| trimmed.strip_prefix("# ")) {
            job.append(h, 0.0, heading_fmt.clone());
            continue;
        }

        let line_to_parse = if let Some(b) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            job.append("•  ", 0.0, bold_fmt.clone());
            b
        } else {
            line
        };

        let mut rest = line_to_parse;
        while !rest.is_empty() {
            if let Some(after_open) = rest.strip_prefix("**") {
                if let Some(end) = after_open.find("**") {
                    job.append(&after_open[..end], 0.0, bold_fmt.clone());
                    rest = &after_open[end + 2..];
                    continue;
                }
            }
            if let Some(after_open) = rest.strip_prefix('`') {
                if let Some(end) = after_open.find('`') {
                    job.append(&after_open[..end], 0.0, code_fmt.clone());
                    rest = &after_open[end + 1..];
                    continue;
                }
            }
            if let Some(after_open) = rest.strip_prefix('*') {
                if !after_open.starts_with('*') {
                    if let Some(end) = after_open.find('*') {
                        job.append(&after_open[..end], 0.0, italic_fmt.clone());
                        rest = &after_open[end + 1..];
                        continue;
                    }
                }
            }
            // Advance to next markdown token
            let next_pos = rest[1..]
                .find(['*', '`'])
                .map(|p| p + 1)
                .unwrap_or(rest.len());
            job.append(&rest[..next_pos], 0.0, normal_fmt.clone());
            rest = &rest[next_pos..];
        }
    }

    ui.add(egui::Label::new(job).selectable(true).wrap());
}
