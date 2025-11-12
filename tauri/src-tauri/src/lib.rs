use tauri::Manager;
use tauri_plugin_shell::ShellExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const BACKEND_PORT: u16 = 5333;
const BACKEND_URL: &str = "http://localhost:5333";

struct BackendState {
    child_pid: Arc<Mutex<Option<u32>>>,
}

/// Wait for backend to be ready by checking health endpoint
async fn wait_for_backend_ready(max_attempts: u32) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;
    
    for attempt in 1..=max_attempts {
        println!("Checking backend health (attempt {}/{})", attempt, max_attempts);
        
        match client.get(format!("{}/api/health", BACKEND_URL)).send().await {
            Ok(response) if response.status().is_success() => {
                println!("Backend is ready!");
                return Ok(());
            }
            Ok(response) => {
                println!("Backend returned non-success status: {}", response.status());
            }
            Err(e) => {
                println!("Backend not ready yet: {}", e);
            }
        }
        
        if attempt < max_attempts {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    
    Err("Backend failed to start within timeout period".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let backend_state = BackendState {
                child_pid: Arc::new(Mutex::new(None)),
            };
            
            app.manage(backend_state);
            
            // Get the sidecar command
            let sidecar_command = app.shell().sidecar("smart-compressor-backend")
                .map_err(|e| format!("Failed to create sidecar command: {}", e))?;
            
            println!("Starting .NET backend sidecar...");
            
            // Spawn the backend process
            let (mut rx, mut child) = tauri::async_runtime::block_on(async {
                sidecar_command
                    .spawn()
                    .map_err(|e| format!("Failed to spawn backend: {}", e))
            })?;
            
            println!("Backend process spawned successfully");
            
            // Spawn task to monitor backend output
            tauri::async_runtime::spawn(async move {
                use tauri_plugin_shell::process::CommandEvent;
                
                while let Some(event) = rx.recv().await {
                    match event {
                        CommandEvent::Stdout(line) => {
                            let output = String::from_utf8_lossy(&line);
                            println!("[Backend] {}", output);
                        }
                        CommandEvent::Stderr(line) => {
                            let output = String::from_utf8_lossy(&line);
                            eprintln!("[Backend] {}", output);
                        }
                        CommandEvent::Error(err) => {
                            eprintln!("[Backend Error] {}", err);
                        }
                        CommandEvent::Terminated(payload) => {
                            println!("[Backend] Process exited with code: {:?}", payload.code);
                            break;
                        }
                        _ => {}
                    }
                }
            });
            
            // Wait for backend to be ready in background
            tauri::async_runtime::spawn(async move {
                match wait_for_backend_ready(40).await {
                    Ok(_) => {
                        println!("Backend is ready!");
                    }
                    Err(e) => {
                        eprintln!("Backend failed to start: {}", e);
                        eprintln!("The application may not function correctly.");
                    }
                }
            });
            
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                println!("Closing application, backend will be shut down");
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
