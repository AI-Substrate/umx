use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};

pub(crate) fn set_nonblocking(fd: BorrowedFd<'_>) -> io::Result<()> {
    // SAFETY: fd is borrowed for both calls; F_GETFL/F_SETFL take no pointer arguments.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags == -1
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn exited_without_reap(pid: u32) -> io::Result<bool> {
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: info is writable siginfo storage; WNOWAIT keeps our child's pid reserved.
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: waitid succeeded and siginfo was zeroed for the no-event case.
    let info = unsafe { info.assume_init() };
    #[cfg(target_os = "macos")]
    let found = info.si_pid;
    #[cfg(not(target_os = "macos"))]
    // SAFETY: waitid supplied siginfo for a child event.
    let found = unsafe { info.si_pid() };
    Ok(found != 0)
}

pub(crate) fn kill_group(pgid: u32) -> io::Result<()> {
    let group = libc::pid_t::try_from(pgid).map_err(|_| io::ErrorKind::InvalidInput)?;
    // SAFETY: getpgrp has no arguments or memory preconditions.
    let ours = unsafe { libc::getpgrp() };
    if group <= 1 || group == ours {
        return Err(io::ErrorKind::InvalidInput.into());
    }

    // Refuse to signal after an early reap; a waitable leader reserves the group id.
    let exited = exited_without_reap(pgid)?;
    // SAFETY: this is our unreaped child's group, never this process's group.
    let result = unsafe { libc::killpg(group, libc::SIGKILL) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH)
        || (cfg!(target_os = "macos") && exited && error.raw_os_error() == Some(libc::EPERM))
    {
        // Darwin returns EPERM for zombie-only groups. A setuid descendant may also be unsignalable;
        // bounded draining must never wait for it.
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn an_exited_owned_group_is_cleaned_before_the_leader_is_reaped() {
        let mut child = Command::new("/usr/bin/true")
            .process_group(0)
            .spawn()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while !exited_without_reap(child.id()).unwrap() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(exited_without_reap(child.id()).unwrap());
        let stopped = kill_group(child.id());
        let status = child.wait().unwrap();
        stopped.expect("an owned zombie-only group is harmless, including Darwin EPERM");
        assert!(status.success());
        assert!(
            kill_group(child.id()).is_err(),
            "a reaped leader is not owned"
        );
    }
}
