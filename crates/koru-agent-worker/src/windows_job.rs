// SPDX-License-Identifier: MPL-2.0

//! A kill-on-close Job Object per worker child, so the whole process tree —
//! the worker and anything it spawned — dies when the worker is killed, and
//! dies with the parent when the parent's handle table is torn down.
//!
//! The same shape `koru-session-runtime` uses for PTY trees. Windows allows
//! nested jobs since 8.1, so a sessions worker that puts its PTYs in jobs of
//! its own still sits inside this one; the integration test proves a
//! grandchild dies with the worker rather than assuming it.

use std::os::windows::io::AsRawHandle as _;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};

pub struct Job(HANDLE);

// A job handle is a kernel object reference; moving it between threads is
// exactly how the control thread outlives the spawner.
unsafe impl Send for Job {}

impl Job {
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(format!(
                "could not create a worker job object: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            let error = std::io::Error::last_os_error();
            unsafe { CloseHandle(handle) };
            return Err(format!(
                "could not configure the worker job object: {error}"
            ));
        }
        Ok(Self(handle))
    }

    pub fn assign(&self, child: &std::process::Child) -> Result<(), String> {
        let ok = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) };
        if ok == 0 {
            return Err(format!(
                "could not place the worker in its job object: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    /// End everything in the job now. Idempotent; the exit still arrives
    /// through the waiter's `wait()`.
    pub fn kill(&self) {
        unsafe { TerminateJobObject(self.0, 1) };
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // KILL_ON_JOB_CLOSE makes this close a kill for anything still
        // alive — which is the point: a parent that vanishes takes its
        // worker trees with it.
        unsafe { CloseHandle(self.0) };
    }
}
