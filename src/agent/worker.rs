use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::agent::{Provider, WorkerCmd, WorkerEvent};

pub struct WorkerHandles {
    pub cmd_tx: Sender<WorkerCmd>,
    pub event_rx: Receiver<WorkerEvent>,
    pub provider_label: &'static str,
}

pub fn spawn_worker(mut provider: Box<dyn Provider>) -> WorkerHandles {
    let provider_label = provider.label();
    let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
    let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>();
    thread::spawn(move || {
        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                WorkerCmd::Send {
                    session_id,
                    request,
                } => {
                    provider.generate(&request, session_id, &event_tx);
                }
                WorkerCmd::UpdateProvider { provider: new_provider } => {
                    provider = new_provider;
                }
                WorkerCmd::Shutdown => break,
            }
        }
    });
    WorkerHandles {
        cmd_tx,
        event_rx,
        provider_label,
    }
}
