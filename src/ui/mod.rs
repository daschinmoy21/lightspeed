use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::discovery::DiscoveryPeer;
use walkdir::WalkDir;
use ratatui::widgets::{Clear};

pub enum AppState {
    Splash(Instant), // Track time for fade effect
    Main,
}

#[derive(PartialEq, Clone)]
pub enum Tab {
    Send,
    Receive,
    About,
}

// Sub-states for Send Tab
pub enum SendState {
    Input,
    FuzzyFinding,
    Discovery,
    Confirming(DiscoveryPeer),
    Transferring,
    Done,
}

// Sub-states for Receive Tab
pub enum ReceiveState {
    Idle,
    Listening(u16), // port
    Receiving,
    Done,
}

pub struct App {
    pub peers: Arc<Mutex<HashMap<String, DiscoveryPeer>>>,
    pub state: AppState,
    pub current_tab: Tab,
    pub known_peers: Vec<DiscoveryPeer>, 
    
    // Components
    pub send_state: SendState,
    pub receive_state: ReceiveState,
    
    // Inputs/State
    pub input_buffer: String, // For file path
    pub peer_list_state: ListState,
    
    // Fuzzy Search
    pub all_files: Vec<String>,
    pub filtered_files: Vec<String>,
    pub fuzzy_list_state: ListState,
    
    // Transfer Viz
    pub transfer: TransferApp,
}

impl App {
    pub fn new(peers: Arc<Mutex<HashMap<String, DiscoveryPeer>>>) -> Self {
        Self {
            peers,
            state: AppState::Splash(Instant::now()),
            current_tab: Tab::Send,
            known_peers: vec![],
            send_state: SendState::Input,
            receive_state: ReceiveState::Idle,
            input_buffer: String::new(),
            peer_list_state: ListState::default(),
            all_files: vec![],
            filtered_files: vec![],
            fuzzy_list_state: ListState::default(),
            transfer: TransferApp::new(),
        }
    }

    pub fn next(&mut self) {
        if self.known_peers.is_empty() {
            return;
        }
        let i = match self.peer_list_state.selected() {
            Some(i) => {
                if i >= self.known_peers.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.peer_list_state.select(Some(i));
    }

    pub fn previous(&mut self) {
        if self.known_peers.is_empty() {
            return;
        }
        let i = match self.peer_list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.known_peers.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.peer_list_state.select(Some(i));
    }
}

pub async fn run_tui(peers: Arc<Mutex<HashMap<String, DiscoveryPeer>>>) -> Result<()> {
    // Setup Terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create App
    let mut app = App::new(peers);
    app.peer_list_state.select(Some(0));

    let res = run_app(&mut terminal, &mut app).await;

    // Restore Terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    res
}

async fn run_app<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    let tick_rate = Duration::from_millis(50); 
    let mut last_tick = Instant::now();
    
    // Transfer Channel
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ProgressEvent>(100);

    loop {
        terminal.draw(|f| ui(f, app))?;

        // Splash Screen Check
        if let AppState::Splash(start) = app.state {
            if start.elapsed() > Duration::from_secs(2) {
                app.state = AppState::Main;
            }
        }
        
        // Poll for Transfer Events
        let mut events_processed = 0;
        while events_processed < 20 {
            match rx.try_recv() {
                Ok(ev) => {
                     match ev {
                         ProgressEvent::Started { total_chunks, total_size, filename } => {
                            app.transfer.total_chunks = total_chunks;
                            app.transfer.total_size = total_size;
                            app.transfer.filename = filename;
                            app.transfer.start_time = Instant::now();
                         }
                         ProgressEvent::ChunkSent { chunk_id: _, size } => {
                            app.transfer.chunks_sent += 1;
                            app.transfer.bytes_sent += size as u64;
                         }
                         ProgressEvent::Error(e) => app.transfer.logs.push(format!("Error: {}", e)),
                         ProgressEvent::Done => app.transfer.logs.push("Done!".into()),
                         ProgressEvent::FilesFound(files) => {
                             app.all_files = files.clone();
                             app.filtered_files = files;
                             if !app.filtered_files.is_empty() {
                                 app.fuzzy_list_state.select(Some(0));
                             }
                         }
                     }
                },
                Err(_) => break,
            }
            events_processed += 1;
        }

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if crossterm::event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                // Global Keys
                match key.code {
                    KeyCode::Char('q') if matches!(app.state, AppState::Main) => return Ok(()),
                    KeyCode::Tab => {
                        // Cycle Tabs
                         app.current_tab = match app.current_tab {
                             Tab::Send => Tab::Receive,
                             Tab::Receive => Tab::About,
                             Tab::About => Tab::Send,
                         };
                    }
                    _ => {}
                }
                
                // Context Keys
                match app.current_tab {
                    Tab::Send => match app.send_state {
                        SendState::Input => match key.code {
                            KeyCode::Char(c) => {
                                if c == '@' && app.input_buffer.is_empty() {
                                    // Trigger Fuzzy Search
                                    app.send_state = SendState::FuzzyFinding;
                                    let tx_files = tx.clone();
                                    tokio::task::spawn_blocking(move || {
                                        let mut files = Vec::new();
                                        for entry in WalkDir::new(".").into_iter().filter_map(|e| e.ok()) {
                                            if entry.file_type().is_file() {
                                                if let Some(path_str) = entry.path().to_str() {
                                                    files.push(path_str.to_string());
                                                }
                                            }
                                        }
                                        let _ = tx_files.blocking_send(ProgressEvent::FilesFound(files));
                                    });
                                } else {
                                    app.input_buffer.push(c);
                                }
                            },
                            KeyCode::Backspace => { app.input_buffer.pop(); },
                            KeyCode::Enter => {
                                if !app.input_buffer.is_empty() {
                                    app.send_state = SendState::Discovery;
                                }
                            }
                            KeyCode::Esc => app.input_buffer.clear(),
                            _ => {}
                        },
                        SendState::FuzzyFinding => match key.code {
                            KeyCode::Down | KeyCode::Char('j') => {
                                if let Some(i) = app.fuzzy_list_state.selected() {
                                    if !app.filtered_files.is_empty() {
                                        let next = if i >= app.filtered_files.len() - 1 { 0 } else { i + 1 };
                                        app.fuzzy_list_state.select(Some(next));
                                    }
                                }
                            },
                            KeyCode::Up | KeyCode::Char('k') => {
                                if let Some(i) = app.fuzzy_list_state.selected() {
                                    if !app.filtered_files.is_empty() {
                                        let next = if i == 0 { app.filtered_files.len() - 1 } else { i - 1 };
                                        app.fuzzy_list_state.select(Some(next));
                                    }
                                }
                            },
                            KeyCode::Enter => {
                                if let Some(i) = app.fuzzy_list_state.selected() {
                                    if let Some(file) = app.filtered_files.get(i) {
                                        app.input_buffer = file.clone();
                                        app.send_state = SendState::Input; // Go back to input with selected file
                                    }
                                }
                            },
                            KeyCode::Esc => {
                                app.send_state = SendState::Input;
                                app.input_buffer.clear();
                            },
                            KeyCode::Backspace => {
                                app.input_buffer.pop();
                                let query = app.input_buffer.to_lowercase();
                                app.filtered_files = app.all_files.iter()
                                    .filter(|f| f.to_lowercase().contains(&query))
                                    .cloned()
                                    .collect();
                                app.fuzzy_list_state.select(if app.filtered_files.is_empty() { None } else { Some(0) });
                            },
                            KeyCode::Char(c) => {
                                app.input_buffer.push(c);
                                // Filter Logic
                                let query = app.input_buffer.to_lowercase();
                                app.filtered_files = app.all_files.iter()
                                    .filter(|f| f.to_lowercase().contains(&query))
                                    .cloned()
                                    .collect();
                                app.fuzzy_list_state.select(if app.filtered_files.is_empty() { None } else { Some(0) });
                            },
                            _ => {}
                        },
                        SendState::Discovery => match key.code {
                            KeyCode::Down | KeyCode::Char('j') => app.next(),
                            KeyCode::Up | KeyCode::Char('k') => app.previous(),
                            KeyCode::Enter => {
                                 if let Some(i) = app.peer_list_state.selected() {
                                     if let Some(peer) = app.known_peers.get(i) {
                                         // START TRANSFER
                                         app.send_state = SendState::Transferring;
                                         
                                         let file_path = app.input_buffer.clone();
                                         let addr = format!("{}:{}", peer.addr.ip(), peer.packet.tcp_port);
                                         let tx_clone = tx.clone();
                                         
                                         tokio::spawn(async move {
                                             let sender = crate::protocol::tcp_send::TcpSender::new(addr, file_path);
                                             let _ = sender.parallel_send(Some(tx_clone)).await;
                                         });
                                     }
                                 }
                            }
                            _ => {}
                        },
                        _ => {}
                    },
                    Tab::Receive => {},
                    Tab::About => {},
                }
                
                // Tab Navigation (Vim Style)
                if matches!(app.state, AppState::Main) && matches!(app.send_state, SendState::Input) == false {
                    match key.code {
                        KeyCode::Char('l') | KeyCode::Right => {
                             app.current_tab = match app.current_tab {
                                 Tab::Send => Tab::Receive,
                                 Tab::Receive => Tab::About,
                                 Tab::About => Tab::Send,
                             };
                        }
                        KeyCode::Char('h') | KeyCode::Left => {
                             app.current_tab = match app.current_tab {
                                 Tab::Send => Tab::About,
                                 Tab::Receive => Tab::Send,
                                 Tab::About => Tab::Receive,
                             };
                        }
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
             // Logic to update peer lists (only if in discovery mode)
            if matches!(app.current_tab, Tab::Send) && matches!(app.send_state, SendState::Discovery) {
                let map = app.peers.lock().unwrap();
                let mut peers: Vec<DiscoveryPeer> = map.values().cloned().collect();
                peers.sort_by_key(|p| p.packet.hostname.clone());
                app.known_peers = peers;
                
                if app.peer_list_state.selected().is_none() && !app.known_peers.is_empty() {
                    app.peer_list_state.select(Some(0));
                }
            }
            last_tick = Instant::now();
        }
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    match &app.state {
        AppState::Splash(_) => render_splash(f),
        AppState::Main => render_main(f, app),
    }
}

fn render_splash(f: &mut Frame) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(45),
            Constraint::Length(3),
            Constraint::Percentage(50),
        ].as_ref())
        .split(f.size());

    let text = Paragraph::new("Accelerate, Anon")
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(ratatui::layout::Alignment::Center)
        .block(Block::default());
        
    f.render_widget(text, chunks[1]);
}

fn render_main(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Tabs
            Constraint::Min(1),    // Content
            Constraint::Length(1), // Footer
        ].as_ref())
        .split(f.size());

    // --- Tabs ---
    let titles: Vec<Line> = vec!["Send", "Receive", "About"]
        .iter()
        .map(|t| {
             let (first, rest) = t.split_at(1);
             Line::from(vec![
                 Span::styled(first, Style::default().fg(Color::Yellow)),
                 Span::styled(rest, Style::default().fg(Color::White)),
             ])
        })
        .collect();

    let tab_index = match app.current_tab {
        Tab::Send => 0,
        Tab::Receive => 1,
        Tab::About => 2,
    };

    let tabs = ratatui::widgets::Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title("Tasks"))
        .select(tab_index)
        .style(Style::default().fg(Color::White))
        .highlight_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    
    f.render_widget(tabs, chunks[0]);

    // --- Content ---
    match app.current_tab {
        Tab::Send => render_send_tab(f, app, chunks[1]),
        Tab::Receive => render_receive_tab(f, app, chunks[1]),
        Tab::About => render_about_tab(f, chunks[1]),
    }

    // --- Footer ---
    let footer_text = match app.current_tab {
        Tab::Send => match app.send_state {
            SendState::Input => "Type Path | Enter: Confirm | Esc: Clear",
            SendState::Discovery => "Up/Down: Select Peer | Enter: Send | R: Rescan",
            SendState::Transferring => "Transferring... | Q: Background",
            _ => "h/l: Switch Tabs | q: Quit",
        },
        _ => "h/l: Switch Tabs | q: Quit",
    };
    let footer = Paragraph::new(footer_text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, chunks[2]);
}

fn render_send_tab(f: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    match app.send_state {
        SendState::Input => {
             let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(1)].as_ref())
                .margin(2)
                .split(area);
                
             let input = Paragraph::new(app.input_buffer.as_str())
                .style(Style::default().fg(Color::Yellow))
                .block(Block::default().borders(Borders::ALL).title("File Path to Send"));
             f.render_widget(input, chunks[0]);
             
             // Instructions
             f.render_widget(Paragraph::new("Enter the full path to the file you want to transfer."), chunks[1]);
        }
        SendState::Discovery => {
            // Re-use previous discovery list logic
             let items: Vec<ListItem> = app.known_peers.iter().map(|p| {
                let lines = vec![
                    Line::from(vec![Span::styled(format!("{}", p.packet.hostname), Style::default().add_modifier(Modifier::BOLD))]),
                    Line::from(format!("IP: {}", p.addr.ip())),
                ];
                ListItem::new(lines).style(Style::default().fg(Color::White))
            }).collect();
            
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title("Select Peer"))
                .highlight_style(Style::default().bg(Color::Blue).add_modifier(Modifier::BOLD))
                .highlight_symbol(">> ");
            f.render_stateful_widget(list, area, &mut app.peer_list_state);
        }
        SendState::Transferring => {
            transfer_ui(f, &app.transfer); // Re-use transfer UI
        }
        SendState::FuzzyFinding => {
            // Render Input background
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(1)].as_ref())
                .margin(2)
                .split(area);

            let input = Paragraph::new(format!("Search: {}", app.input_buffer))
                .style(Style::default().fg(Color::Yellow))
                .block(Block::default().borders(Borders::ALL).title("Fuzzy Search"));
            f.render_widget(input, chunks[0]);
            
            // Pop-over list
            let area = centered_rect(60, 50, area);
            f.render_widget(Clear, area); // Clear background
            
            let items: Vec<ListItem> = app.filtered_files.iter().map(|s| ListItem::new(s.as_str())).collect();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title("Results"))
                .highlight_style(Style::default().bg(Color::Blue).add_modifier(Modifier::BOLD));
            
            f.render_stateful_widget(list, area, &mut app.fuzzy_list_state);
        }
        _ => {}
    }
}

// Helper to center a rect
fn centered_rect(percent_x: u16, percent_y: u16, r: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ].as_ref())
        .split(r);

    let hor_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ].as_ref())
        .split(popup_layout[1]);

    hor_layout[1]
}

fn render_receive_tab(f: &mut Frame, _app: &mut App, area: ratatui::layout::Rect) {
     let block = Block::default().borders(Borders::ALL).title("Receive Mode");
     let text = Paragraph::new("Listening on port 9001 (TCP) & 9003 (QUIC)\n\nReady to receive files...").block(block);
     f.render_widget(text, area);
}

fn render_about_tab(f: &mut Frame, area: ratatui::layout::Rect) {
     let block = Block::default().borders(Borders::ALL).title("About");
     let text = Paragraph::new("LightSpeed v0.1.0\n\nA high-performance file transfer tool.\n\nCreated with Rust + Ratatui.").block(block);
     f.render_widget(text, area);
}

// --- Transfer TUI ---

pub enum ProgressEvent {
    Started { total_chunks: u64, total_size: u64, filename: String },
    ChunkSent { chunk_id: u64, size: usize },
    Error(String),
    Done,
    FilesFound(Vec<String>),
}

pub struct TransferApp {
    pub filename: String,
    pub total_chunks: u64,
    pub chunks_sent: u64,
    pub total_size: u64,
    pub bytes_sent: u64,
    pub start_time: Instant,
    pub logs: Vec<String>,
}

impl TransferApp {
    fn new() -> Self {
        Self {
            filename: String::new(),
            total_chunks: 1, // Avoid div by zero
            chunks_sent: 0,
            total_size: 1,
            bytes_sent: 0,
            start_time: Instant::now(),
            logs: vec![],
        }
    }
}



fn transfer_ui(f: &mut Frame, app: &TransferApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Length(3), // Gauge
            Constraint::Min(1),    // Stats/Logs
        ].as_ref())
        .split(f.area());

    let title = Paragraph::new(format!("Sending: {}", app.filename))
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    let progress = (app.bytes_sent as f64 / app.total_size as f64).clamp(0.0, 1.0);
    // Use Gauge widget
    let gauge = ratatui::widgets::Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("Progress"))
        .gauge_style(Style::default().fg(Color::Green))
        .ratio(progress);
    f.render_widget(gauge, chunks[1]);
    
    // Stats
    let elapsed = app.start_time.elapsed().as_secs_f64();
    let speed = if elapsed > 0.0 {
        app.bytes_sent as f64 / elapsed
    } else {
        0.0
    };
    let speed_str = crate::format_bytes(speed as u64);
    
    let stats_text = format!(
        "Sent: {} / {}\nSpeed: {}/s\nChunks: {} / {}",
        crate::format_bytes(app.bytes_sent),
        crate::format_bytes(app.total_size),
        speed_str,
        app.chunks_sent,
        app.total_chunks
    );
    
    let stats = Paragraph::new(stats_text)
        .block(Block::default().borders(Borders::ALL).title("Stats"));
    f.render_widget(stats, chunks[2]);
}
