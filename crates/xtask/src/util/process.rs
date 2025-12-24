use anyhow::Result;
use sysinfo::{Pid, ProcessesToUpdate, Signal, System};

#[derive(Clone, Copy, Debug)]
pub enum KillStrategy {
    TermThenKill,
    #[allow(dead_code)]
    Kill,
}

pub fn cmd_any_contains(proc: &sysinfo::Process, needle: &str) -> bool {
    proc.cmd()
        .iter()
        .any(|s| s.to_string_lossy().contains(needle))
}

pub fn cmd_any_eq(proc: &sysinfo::Process, token: &str) -> bool {
    proc.cmd()
        .iter()
        .any(|s| s.to_string_lossy().as_ref() == token)
}

pub fn find_pids_matching<F>(mut predicate: F) -> Vec<Pid>
where
    F: FnMut(&sysinfo::Process) -> bool,
{
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);

    system
        .processes()
        .iter()
        .filter_map(|(pid, proc)| {
            if predicate(proc) {
                Some(*pid)
            } else {
                None
            }
        })
        .collect()
}

pub fn kill_pids(pids: &[Pid], strategy: KillStrategy) -> Result<()> {
    if pids.is_empty() {
        return Ok(());
    }

    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);

    for pid in pids {
        if let Some(proc) = system.process(*pid) {
            match strategy {
                KillStrategy::TermThenKill => {
                    let _ = proc.kill_with(Signal::Term);
                }
                KillStrategy::Kill => {
                    let _ = proc.kill_with(Signal::Kill);
                }
            }
        }
    }

    // Give TERM some time, then KILL remaining.
    if matches!(strategy, KillStrategy::TermThenKill) {
        std::thread::sleep(std::time::Duration::from_millis(500));
        system.refresh_processes(ProcessesToUpdate::All, true);
        for pid in pids {
            if let Some(proc) = system.process(*pid) {
                let _ = proc.kill_with(Signal::Kill);
            }
        }
    }

    Ok(())
}
