use crate::client;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use locald_core::{IpcRequest, IpcResponse};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use std::{io, time::Duration};

pub fn run() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run_app(&mut terminal);

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{err:?}");
    }

    Ok(())
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    loop {
        // Fetch status
        let services = match client::send_request(&IpcRequest::Status) {
            Ok(IpcResponse::Status(s)) => s,
            _ => vec![], // Handle error gracefully in UI later
        };

        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([Constraint::Percentage(10), Constraint::Percentage(90)].as_ref())
                .split(f.area());

            let title = Paragraph::new("locald monitor (Press 'q' to quit)")
                .block(Block::default().borders(Borders::ALL));
            f.render_widget(title, chunks[0]);

            let items: Vec<ListItem> = services
                .iter()
                .map(|s| {
                    let status_style = match s.status.as_str() {
                        "running" => Style::default().fg(Color::Green),
                        "stopped" => Style::default().fg(Color::Red),
                        _ => Style::default(),
                    };

                    let content = format!(
                        "{:<20} [{}] PID: {:<6} Port: {:<5} URL: {}",
                        s.name,
                        s.status,
                        s.pid.map_or_else(|| "-".into(), |p| p.to_string()),
                        s.port.map_or_else(|| "-".into(), |p| p.to_string()),
                        s.url.as_deref().unwrap_or("-")
                    );
                    ListItem::new(content).style(status_style)
                })
                .collect();

            let list =
                List::new(items).block(Block::default().title("Services").borders(Borders::ALL));
            f.render_widget(list, chunks[1]);
        })?;

        if event::poll(Duration::from_millis(500))?
            && let Event::Key(key) = event::read()?
            && key.code == KeyCode::Char('q')
        {
            return Ok(());
        }
    }
}
