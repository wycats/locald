use crate::client;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use locald_core::{
    IpcRequest, IpcResponse,
    ipc::{PublicationState, ServiceType},
    state::ServiceState,
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use std::{fmt::Write as FmtWrite, io, time::Duration};

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
                    let status_style = s.publication.as_ref().map_or_else(
                        || match s.status {
                            ServiceState::Running => {
                                if s.warnings.is_empty() {
                                    Style::default().fg(Color::Green)
                                } else {
                                    Style::default().fg(Color::Yellow)
                                }
                            }
                            ServiceState::Stopped => Style::default().fg(Color::Red),
                            ServiceState::Building => Style::default().fg(Color::Blue),
                            ServiceState::ExternallyManaged => Style::default().fg(Color::Cyan),
                        },
                        |publication| match publication.state {
                            PublicationState::Ready => Style::default().fg(Color::Green),
                            PublicationState::CheckingEndpoint => Style::default().fg(Color::Blue),
                            PublicationState::EndpointUnhealthy => {
                                Style::default().fg(Color::Yellow)
                            }
                            PublicationState::WaitingForPublisher => {
                                Style::default().fg(Color::Cyan)
                            }
                            PublicationState::RoutePaused => Style::default().fg(Color::DarkGray),
                            PublicationState::InstanceMissing => Style::default().fg(Color::Red),
                        },
                    );

                    let mut content = if s.service_type == ServiceType::Published {
                        s.publication.as_ref().map_or_else(
                            || {
                                format!(
                                    "{:<20} [published] Origin: {}",
                                    s.name,
                                    s.url.as_deref().unwrap_or("-")
                                )
                            },
                            |publication| {
                                format!(
                                    "{:<20} [published: {}] Origin: {} — {}",
                                    s.name,
                                    publication.state,
                                    publication.origin,
                                    publication.explanation
                                )
                            },
                        )
                    } else {
                        format!(
                            "{:<20} [{}] PID: {:<6} Port: {:<5} URL: {}",
                            s.name,
                            s.status,
                            s.pid.map_or_else(|| "-".into(), |p| p.to_string()),
                            s.port.map_or_else(|| "-".into(), |p| p.to_string()),
                            s.url.as_deref().unwrap_or("-")
                        )
                    };

                    if let Some(next_step) = s
                        .publication
                        .as_ref()
                        .and_then(|publication| publication.next_step.as_deref())
                    {
                        let _ = write!(content, " NEXT: {next_step}");
                    }

                    if !s.warnings.is_empty() {
                        let _ = write!(content, " WARNING: {}", s.warnings.join(", "));
                    }

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
