//! Unix handles for a PTY and direct child inherited across daemon exec.
use portable_pty::{Child, ChildKiller, ExitStatus, MasterPty, PtySize};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::ExitStatusExt;

pub(crate) struct InheritedMaster(pub OwnedFd, pub u32);

impl MasterPty for InheritedMaster {
    fn resize(&self, size: PtySize) -> anyhow::Result<()> {
        let size = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: size.pixel_width,
            ws_ypixel: size.pixel_height,
        };
        // SAFETY: self owns a live terminal descriptor; size has the ioctl ABI.
        if unsafe { libc::ioctl(self.0.as_raw_fd(), libc::TIOCSWINSZ, &size) } == -1 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
    fn get_size(&self) -> anyhow::Result<PtySize> {
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(self.0.as_raw_fd(), libc::TIOCGWINSZ, &mut size) } == -1 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(PtySize {
            rows: size.ws_row,
            cols: size.ws_col,
            pixel_width: size.ws_xpixel,
            pixel_height: size.ws_ypixel,
        })
    }
    fn try_clone_reader(&self) -> anyhow::Result<Box<dyn io::Read + Send>> {
        Ok(Box::new(InheritedReader(File::from(self.0.try_clone()?))))
    }
    fn take_writer(&self) -> anyhow::Result<Box<dyn io::Write + Send>> {
        Ok(Box::new(File::from(self.0.try_clone()?)))
    }
    fn process_group_leader(&self) -> Option<libc::pid_t> {
        Some(self.1 as libc::pid_t)
    }
    fn as_raw_fd(&self) -> Option<libc::c_int> {
        Some(self.0.as_raw_fd())
    }
    fn tty_name(&self) -> Option<std::path::PathBuf> {
        None
    }
}

#[derive(Debug, Clone)]
pub(crate) struct InheritedChild {
    pub pid: u32,
    pub status: Option<ExitStatus>,
}
impl InheritedChild {
    fn reap(&mut self, flags: i32) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = &self.status {
            return Ok(Some(status.clone()));
        }
        loop {
            let mut status = 0;
            // SAFETY: only the inherited direct child is waited; same PID after exec.
            match unsafe { libc::waitpid(self.pid as libc::pid_t, &mut status, flags) } {
                0 => return Ok(None),
                -1 => {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(error);
                }
                _ => {
                    let status = ExitStatus::from(std::process::ExitStatus::from_raw(status));
                    self.status = Some(status.clone());
                    return Ok(Some(status));
                }
            }
        }
    }
}
impl ChildKiller for InheritedChild {
    fn kill(&mut self) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        if unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGKILL) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}
impl Child for InheritedChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.reap(libc::WNOHANG)
    }
    fn wait(&mut self) -> io::Result<ExitStatus> {
        self.reap(0)?
            .ok_or_else(|| io::Error::other("waitpid returned no status"))
    }
    fn process_id(&self) -> Option<u32> {
        Some(self.pid)
    }
}

struct InheritedReader(File);
impl io::Read for InheritedReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match io::Read::read(&mut self.0, bytes) {
            Err(e) if e.raw_os_error() == Some(libc::EIO) => Ok(0),
            result => result,
        }
    }
}
