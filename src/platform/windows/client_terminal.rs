use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub(crate) fn open_client_terminal_writer(
    stopped: Arc<AtomicBool>,
) -> io::Result<Box<dyn io::Write + Send>> {
    Ok(Box::new(InterruptibleClientStdout { stopped }))
}

struct InterruptibleClientStdout {
    stopped: Arc<AtomicBool>,
}

impl io::Write for InterruptibleClientStdout {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "client terminal output stopped",
            ));
        }
        io::stdout().write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "client terminal output stopped",
            ));
        }
        io::stdout().flush()
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CancelSynchronousIo(thread: windows_sys::Win32::Foundation::HANDLE) -> i32;
}

pub(crate) fn interrupt_client_terminal_writer(thread: &std::thread::JoinHandle<()>) {
    use std::os::windows::io::AsRawHandle;
    crate::platform::wait_for_client_terminal_writer_with_cancel(thread, || unsafe {
        CancelSynchronousIo(thread.as_raw_handle().cast());
    });
}
