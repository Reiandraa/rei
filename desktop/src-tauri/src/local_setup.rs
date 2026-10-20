//! Explicit local-AI setup. Fixed vendor links, fixed application paths and a small
//! model catalog; renderer input never becomes a URL, command, path or shell script.
use crate::{commands::IpcError, inbox, ollama};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};
use std::time::{Duration, Instant};

type Res<T> = Result<T, IpcError>;
const CATALOG: &[&str] = &["qwen3.5:2b", "qwen3.5:4b", "qwen3.5:9b"];
static DOWNLOAD: OnceLock<Mutex<Download>> = OnceLock::new();
static CANCEL: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Default, Serialize)]
pub struct Download {
    pub state: String,
    pub model: String,
    pub detail: String,
    pub completed: u64,
    pub total: u64,
}
#[derive(Serialize)]
pub struct View {
    pub platform: String,
    pub installed: bool,
    pub memory_gb: Option<u64>,
    pub recommended_model: String,
    pub download: Download,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub action: String,
    pub model: Option<String>,
}
fn download() -> &'static Mutex<Download> {
    DOWNLOAD.get_or_init(|| {
        Mutex::new(Download {
            state: "idle".into(),
            ..Download::default()
        })
    })
}
fn recommended(memory: Option<u64>) -> &'static str {
    match memory {
        Some(gb) if gb >= 24 => CATALOG[2],
        Some(gb) if gb >= 12 => CATALOG[1],
        _ => CATALOG[0],
    }
}
fn application() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let candidates = {
        let mut paths = vec![PathBuf::from("/Applications/Ollama.app")];
        if let Some(home) = std::env::var_os("HOME") {
            paths.push(PathBuf::from(home).join("Applications/Ollama.app"));
        }
        paths
    };
    #[cfg(target_os = "windows")]
    let candidates = std::env::var_os("LOCALAPPDATA")
        .map(|p| vec![PathBuf::from(p).join("Programs/Ollama/ollama app.exe")])
        .unwrap_or_default();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let candidates: Vec<PathBuf> = vec![];
    candidates.into_iter().find(|p| p.exists())
}
fn installed() -> bool {
    application().is_some()
        || std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|p| {
                p.join(if cfg!(windows) {
                    "ollama.exe"
                } else {
                    "ollama"
                })
                .is_file()
            })
        })
}
fn memory_gb() -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetPhysicallyInstalledSystemMemory(total: *mut u64) -> i32;
        }
        let mut kb = 0;
        // OS writes one u64 to our valid stack allocation. No user input or process.
        if unsafe { GetPhysicallyInstalledSystemMemory(&mut kb) } != 0 {
            return Some(kb.div_ceil(1024 * 1024));
        }
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        if out.status.success() {
            return String::from_utf8(out.stdout)
                .ok()?
                .trim()
                .parse::<u64>()
                .ok()
                .map(|b| b.div_ceil(1024 * 1024 * 1024));
        }
    }
    None
}
fn view() -> Res<View> {
    let memory = memory_gb();
    Ok(View {
        platform: std::env::consts::OS.into(),
        installed: installed(),
        memory_gb: memory,
        recommended_model: recommended(memory).into(),
        download: download().lock().map_err(|_| IpcError::internal())?.clone(),
    })
}
fn finish(state: &str, detail: &str) {
    if let Ok(mut d) = download().lock() {
        d.state = state.into();
        d.detail = detail.into();
    }
}
fn progress(v: &Value, d: &mut Download) -> Result<bool, ()> {
    if v.get("error").is_some() {
        return Err(());
    }
    let status = v.get("status").and_then(Value::as_str).ok_or(())?;
    if status == "success" {
        return Ok(true);
    }
    let total = v.get("total").and_then(Value::as_u64).unwrap_or(0);
    d.total = total;
    d.completed = v
        .get("completed")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(total);
    d.detail = if total > 0 {
        "Downloading a model file…"
    } else if status.contains("verifying") {
        "Verifying the download…"
    } else {
        "Preparing model files…"
    }
    .into();
    Ok(false)
}
fn pull(model: String) {
    let result = (|| -> Result<(), ()> {
        // No redirect, proxy, remote endpoint or generated content. Ollama handles
        // registry downloads; this request contains only the owner-selected model.
        let response = ureq::AgentBuilder::new()
            .redirects(0)
            .try_proxy_from_env(false)
            .timeout_connect(Duration::from_secs(3))
            .timeout_read(Duration::from_secs(15))
            .timeout_write(Duration::from_secs(5))
            .build()
            .post("http://127.0.0.1:11434/api/pull")
            .send_json(serde_json::json!({"model": model, "stream": true}))
            .map_err(|_| ())?;
        let mut reader = BufReader::new(response.into_reader());
        let started = Instant::now();
        loop {
            if CANCEL.load(Ordering::SeqCst) || started.elapsed() > Duration::from_secs(1800) {
                return Err(());
            }
            let mut line = String::new();
            let count = reader
                .by_ref()
                .take(16385)
                .read_line(&mut line)
                .map_err(|_| ())?;
            if count == 0 || count > 16384 {
                return Err(());
            }
            let value: Value = serde_json::from_str(&line).map_err(|_| ())?;
            let success = progress(&value, &mut *download().lock().map_err(|_| ())?)?;
            if success {
                // Ready means installed, never just a progress message from the server.
                let models = ollama::OllamaClient::new(inbox::OLLAMA_URL, 5)
                    .map_err(|_| ())?
                    .list_models()
                    .map_err(|_| ())?;
                return if models.contains(&model) {
                    Ok(())
                } else {
                    Err(())
                };
            }
        }
    })();
    if CANCEL.load(Ordering::SeqCst) {
        finish(
            "cancelled",
            "Download stopped. Retry later to resume available files.",
        );
    } else if result.is_ok() {
        finish(
            "complete",
            "Model installed. Choose it below to use it in REI.",
        );
    } else {
        finish("error", "Download did not finish. Check Ollama, internet access and free disk space, then retry. An older Ollama version may need updating.");
    }
}

#[tauri::command]
pub async fn local_ai_setup(req: Request) -> Res<View> {
    match req.action.as_str() {
        "inspect" => {}
        "installer" => {
            let link = match std::env::consts::OS {
                "windows" => "https://ollama.com/download/windows",
                "macos" => "https://ollama.com/download/mac",
                _ => "https://ollama.com/download",
            };
            open::that(link).map_err(|_| {
                IpcError::new(
                    "Unavailable",
                    "Could not open the browser. Visit ollama.com/download to install Ollama.",
                )
            })?;
        }
        "open" => {
            let path = application().ok_or_else(|| IpcError::new("NotFound", "Ollama was not found in Applications or its usual install folder. Open it from your apps, then choose Check again."))?;
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                std::process::Command::new(path)
                    .creation_flags(0x08000000)
                    .spawn()
                    .map_err(|_| {
                        IpcError::new(
                            "Unavailable",
                            "Ollama could not open. Try opening it from Start.",
                        )
                    })?;
            }
            #[cfg(not(target_os = "windows"))]
            open::that(path).map_err(|_| {
                IpcError::new(
                    "Unavailable",
                    "Ollama could not open. Try opening it from Applications.",
                )
            })?;
        }
        "download" => {
            let model = req
                .model
                .filter(|m| CATALOG.contains(&m.as_str()))
                .ok_or_else(|| IpcError::new("Validation", "Choose one of the setup models."))?;
            let mut d = download().lock().map_err(|_| IpcError::internal())?;
            if matches!(d.state.as_str(), "downloading" | "cancelling") {
                return Err(IpcError::new(
                    "Conflict",
                    "A model download is already in progress.",
                ));
            }
            CANCEL.store(false, Ordering::SeqCst);
            *d = Download {
                state: "downloading".into(),
                model: model.clone(),
                detail: "Connecting to Ollama…".into(),
                completed: 0,
                total: 0,
            };
            std::thread::spawn(move || pull(model));
        }
        "cancel" => {
            let mut d = download().lock().map_err(|_| IpcError::internal())?;
            if d.state == "downloading" {
                CANCEL.store(true, Ordering::SeqCst);
                d.state = "cancelling".into();
                d.detail = "Stopping the download; waiting for Ollama to respond…".into();
            }
        }
        _ => {
            return Err(IpcError::new(
                "Validation",
                "Unknown local AI setup action.",
            ))
        }
    }
    tauri::async_runtime::spawn_blocking(view)
        .await
        .map_err(|_| IpcError::internal())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recommendations_are_conservative_ram_starting_points() {
        assert_eq!(recommended(None), "qwen3.5:2b");
        assert_eq!(recommended(Some(8)), "qwen3.5:2b");
        assert_eq!(recommended(Some(16)), "qwen3.5:4b");
        assert_eq!(recommended(Some(24)), "qwen3.5:9b");
    }
    #[test]
    fn progress_never_claims_completion_until_success_and_rejects_errors() {
        let mut d = Download::default();
        assert!(!progress(
            &serde_json::json!({"status":"pulling file","total":100,"completed":120}),
            &mut d
        )
        .unwrap());
        assert_eq!(d.completed, 100);
        assert!(!progress(
            &serde_json::json!({"status":"verifying sha256 digest"}),
            &mut d
        )
        .unwrap());
        assert_eq!(d.total, 0);
        assert!(progress(&serde_json::json!({"status":"success"}), &mut d).unwrap());
        assert!(progress(&serde_json::json!({"error":"synthetic failure"}), &mut d).is_err());
        assert!(progress(&serde_json::json!({}), &mut d).is_err());
    }
    #[test]
    fn setup_request_rejects_arbitrary_fields() {
        assert!(serde_json::from_value::<Request>(
            serde_json::json!({"action":"installer","url":"https://example.invalid"})
        )
        .is_err());
    }
}
