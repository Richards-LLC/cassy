//! Factory example - spawn Claude instances in panes
//!
//! Run with: cargo run -p cas-mux --example factory
//!
//! This spawns actual Claude CLI instances. Make sure `claude` is in your PATH.

use cas_mux::{Mux, MuxConfig, MuxEvent, Renderer, SupervisorCli};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use std::io;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== CAS Factory Prototype ===\n");

    let cwd = std::env::current_dir()?;
    println!("Working directory: {}", cwd.display());

    // Create factory config with 2 workers + supervisor
    let config = MuxConfig {
        cwd,
        cas_root: None,
        factory_session: None,
        worker_cwds: std::collections::HashMap::new(),
        workers: 2,
        worker_names: vec!["swift-fox".to_string(), "calm-owl".to_string()],
        supervisor_name: "wise-eagle".to_string(),
        supervisor_cli: SupervisorCli::Claude,
        worker_cli: SupervisorCli::Claude,
        supervisor_model: None,
        worker_model: None,
        supervisor_effort: None,
        worker_effort: None,
        include_director: false, // No director for this demo
        rows: 24,
        cols: 120,
        teams_configs: std::collections::HashMap::new(),
        resolved_worker_specs: vec![],
    };

    println!("Starting factory with:");
    println!("  Workers: {:?}", config.worker_names);
    println!("  Supervisor: {}", config.supervisor_name);
    println!();

    // Create the multiplexer - this spawns Claude instances!
    println!("Spawning Claude instances...");
    let mut mux = match Mux::factory(config) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Failed to create factory: {e}");
            eprintln!("\nMake sure 'claude' CLI is installed and in your PATH.");
            eprintln!("Install with: npm install -g @anthropic-ai/claude-cli");
            return Err(e.into());
        }
    };

    println!("✓ Factory started with {} panes\n", mux.pane_ids().len());

    // Show pane info
    for id in mux.pane_ids() {
        let pane = mux.get(id).unwrap();
        println!(
            "  {} - {:?} {}",
            id,
            pane.kind(),
            if pane.is_focused() { "[FOCUSED]" } else { "" }
        );
    }

    println!("\n--- Starting TUI (press 'q' to quit, Tab to switch panes) ---\n");

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create renderer
    let renderer = Renderer::new();

    // Run the UI loop
    let result = run_app(&mut terminal, &mut mux, &renderer).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    if let Err(e) = result {
        eprintln!("Error: {e}");
    }

    println!("\n✓ Factory prototype demo complete!");

    Ok(())
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mux: &mut Mux,
    renderer: &Renderer,
) -> anyhow::Result<()> {
    loop {
        // Draw current state
        terminal.draw(|frame| {
            renderer.render(frame, mux);
        })?;

        // Poll for PTY events (non-blocking)
        while let Some(event) = mux.poll() {
            match event {
                MuxEvent::PaneOutput { pane_id: _, .. } => {
                    // Terminal state already updated, just redraw
                }
                MuxEvent::PaneExited {
                    pane_id, exit_code, ..
                } => {
                    eprintln!("Pane {pane_id} exited with code {exit_code:?}");
                }
                _ => {}
            }
        }

        // Poll for keyboard events with short timeout
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Tab => mux.focus_next(),
                KeyCode::BackTab => mux.focus_prev(),
                KeyCode::Char('i') => {
                    // Inject test prompt to focused pane
                    if let Err(e) = mux.inject_focused("What is 2+2? Answer briefly.").await {
                        eprintln!("Injection failed: {e}");
                    }
                }
                _ => {
                    // Send other keys to focused pane
                    if let Some(c) = key_to_bytes(key.code) {
                        let _ = mux.send_input(&c).await;
                    }
                }
            }
        }
    }
}

fn key_to_bytes(code: KeyCode) -> Option<Vec<u8>> {
    match code {
        KeyCode::Enter => Some(vec![b'\r']), // Carriage return for PTY
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Char(c) => {
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf);
            Some(s.as_bytes().to_vec())
        }
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        _ => None,
    }
}
