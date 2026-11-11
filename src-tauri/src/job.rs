//! Kernel-level ownership of the child processes the app spawns.
//!
//! The Telegram proxy and the short-lived `winws.exe` instances of a strategy
//! search must never outlive the app, but the cleanup itself must not depend on
//! external tools. During Windows shutdown a freshly created process can no
//! longer initialize its DLLs and dies with `0xc0000142` — that is the
//! "taskkill.exe — Ошибка приложения" dialog a user sees on logoff. A job
//! object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` solves it in the kernel:
//! every member dies when the last handle to the job closes, which happens when
//! this process ends, even if it is terminated without running any cleanup code.

use std::os::windows::io::AsRawHandle;
use std::process::Child;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

struct JobHandle(HANDLE);

// A job handle belongs to the process, not to a thread, and is used from the
// command handlers and background threads alike.
unsafe impl Send for JobHandle {}
unsafe impl Sync for JobHandle {}

static JOB: OnceLock<JobHandle> = OnceLock::new();

/// Creates a job that kills its members when the last handle closes. Returns a
/// null handle when the job cannot be set up.
fn create_job() -> HANDLE {
    unsafe {
        let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if handle.is_null() {
            return std::ptr::null_mut();
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if configured == 0 {
            let _ = CloseHandle(handle);
            return std::ptr::null_mut();
        }
        handle
    }
}

/// The process-wide job, created on first use.
fn job() -> Option<HANDLE> {
    let handle = JOB.get_or_init(|| JobHandle(create_job()));
    (!handle.0.is_null()).then_some(handle.0)
}

fn adopt_into(job: HANDLE, child: &Child) {
    unsafe {
        AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE);
    }
}

/// Puts a freshly spawned child under the job. Failures are not fatal: the
/// explicit stop paths still handle the common cases.
pub fn adopt(child: &Child) {
    if let Some(job) = job() {
        adopt_into(job, child);
    }
}

/// Terminates every process in the job. One kernel call: usable while the
/// session is shutting down, unlike spawning `taskkill.exe`.
pub fn terminate_all() {
    if let Some(job) = job() {
        unsafe {
            TerminateJobObject(job, 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn terminates_the_children_it_adopted() {
        // A dedicated job: the process-wide one is terminated for good by
        // `terminate_all`, and a terminated job can no longer adopt children.
        let job = create_job();
        assert!(!job.is_null(), "job object must be created");

        let mut child = Command::new(crate::sys::system32("ping.exe"))
            .args(["-n", "60", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn a long-running child");

        adopt_into(job, &child);
        unsafe {
            TerminateJobObject(job, 1);
        }

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut reaped = false;
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                reaped = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if !reaped {
            let _ = child.kill();
        }
        let _ = child.wait();
        unsafe {
            CloseHandle(job);
        }
        assert!(reaped, "the child survived the job termination");
    }
}
