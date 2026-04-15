use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use crate::agent::{Provider, WorkerCmd, WorkerEvent};

pub struct WorkerHandles {
    pub cmd_tx: Sender<WorkerCmd>,
    pub event_tx: Sender<WorkerEvent>,
    pub event_rx: Receiver<WorkerEvent>,
    pub provider_label: &'static str,
}

pub fn spawn_worker(provider: Arc<dyn Provider>) -> WorkerHandles {
    let provider_label = provider.label();
    let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
    let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>();
    let mut provider = provider;
    let event_tx_for_handles = event_tx.clone();
    thread::spawn(move || {
        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                WorkerCmd::Send {
                    turn_id,
                    session_id,
                    request,
                } => {
                    let provider = Arc::clone(&provider);
                    let event_tx = event_tx.clone();
                    thread::spawn(move || {
                        provider.generate(&request, session_id, turn_id, &event_tx);
                    });
                }
                WorkerCmd::UpdateProvider {
                    provider: new_provider,
                } => {
                    provider = Arc::from(new_provider);
                }
                WorkerCmd::Shutdown => break,
            }
        }
    });
    WorkerHandles {
        cmd_tx,
        event_tx: event_tx_for_handles,
        event_rx,
        provider_label,
    }
}
