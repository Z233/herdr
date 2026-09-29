use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use super::ClientLoopEvent;

/// Exactly one terminal command may be waiting on the host. The event loop retains
/// the next semantic frame, not a queue of already encoded image data.
pub(super) struct TerminalOutput {
    commands: Option<mpsc::SyncSender<Vec<u8>>>,
    acknowledgements: mpsc::Receiver<io::Result<()>>,
    shutdown: Arc<TerminalOutputShutdown>,
    busy: bool,
}

#[derive(Default)]
pub(super) struct TerminalOutputShutdown {
    stopped: Arc<AtomicBool>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl TerminalOutputShutdown {
    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(worker) = worker {
            if worker.thread().id() == std::thread::current().id() {
                return;
            }
            #[cfg(windows)]
            crate::platform::interrupt_client_terminal_writer(&worker);
            let _ = worker.join();
        }
    }

    pub(super) fn is_worker_thread(&self) -> bool {
        self.worker
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .is_some_and(|worker| worker.thread().id() == std::thread::current().id())
    }
}

impl TerminalOutput {
    pub(super) fn start(
        events: tokio::sync::mpsc::Sender<ClientLoopEvent>,
        shutdown: Arc<TerminalOutputShutdown>,
    ) -> io::Result<Self> {
        let mut writer = crate::platform::open_client_terminal_writer(shutdown.stopped.clone())?;
        let (commands, receiver) = mpsc::sync_channel::<Vec<u8>>(1);
        let (acknowledge, acknowledgements) = mpsc::sync_channel(1);
        let shutdown_worker = shutdown.stopped.clone();
        let worker = std::thread::Builder::new()
            .name("herdr-terminal-output".into())
            .spawn(move || {
                while !shutdown_worker.load(Ordering::Acquire) {
                    let bytes = match receiver.recv_timeout(Duration::from_millis(50)) {
                        Ok(bytes) => bytes,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    let result = writer.write_all(&bytes).and_then(|()| writer.flush());
                    let failed = result.is_err();
                    let _ = acknowledge.try_send(result);
                    let _ = events.try_send(ClientLoopEvent::OutputReady);
                    if failed {
                        break;
                    }
                }
            })?;
        *shutdown
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(worker);
        Ok(Self {
            commands: Some(commands),
            acknowledgements,
            shutdown,
            busy: false,
        })
    }

    pub(super) fn is_busy(&self) -> bool {
        self.busy
    }

    pub(super) fn send(&mut self, bytes: Vec<u8>) -> io::Result<()> {
        if self.busy {
            return Err(io::Error::other("terminal output is still busy"));
        }
        self.commands
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "terminal output stopped"))?
            .try_send(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "terminal output stopped"))?;
        self.busy = true;
        Ok(())
    }

    pub(super) fn acknowledge(&mut self) -> Option<io::Result<()>> {
        let result = match self.acknowledgements.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal output worker stopped",
            )),
        };
        self.busy = false;
        Some(result)
    }

    pub(super) fn finish_pending(&mut self, timeout: Duration) -> Option<io::Result<()>> {
        if !self.busy {
            return Some(Ok(()));
        }
        let result = self.acknowledgements.recv_timeout(timeout).ok()?;
        self.busy = false;
        Some(result)
    }

    pub(super) fn stop(&mut self) {
        self.commands.take();
        self.shutdown.stop();
    }
}

impl Drop for TerminalOutput {
    fn drop(&mut self) {
        self.stop();
    }
}
