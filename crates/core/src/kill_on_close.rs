//! Windows job objects that end every process in them when the last handle
//! closes: the browser and its helpers, and what a shell command starts.

use windows_sys::Win32::Foundation::HANDLE;

/// A job that ends every process in it when its last handle closes, which
/// Windows does however Themis exits.
#[derive(Debug)]
pub(crate) struct KillOnClose(HANDLE);

// SAFETY: a job handle may be used and closed from any thread.
unsafe impl Send for KillOnClose {}
unsafe impl Sync for KillOnClose {}

impl KillOnClose {
    /// Puts `process` (and the processes it starts from now on) in a new job.
    pub(crate) fn around(process: std::os::windows::io::RawHandle) -> std::io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        // SAFETY: plain Win32 calls on a job this function creates and owns;
        // the limits struct outlives the call that reads it.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let job = Self(job);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
                || AssignProcessToJobObject(job.0, process.cast()) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(job)
        }
    }

    /// Processes still running in the job (0 when it cannot be read).
    pub(crate) fn active_processes(&self) -> u32 {
        use windows_sys::Win32::System::JobObjects::{
            JobObjectBasicAccountingInformation, QueryInformationJobObject,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        };
        // SAFETY: the job handle is ours; the struct outlives the call that fills it.
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
            let read = QueryInformationJobObject(
                self.0,
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            );
            if read == 0 {
                0
            } else {
                info.ActiveProcesses
            }
        }
    }
}

impl Drop for KillOnClose {
    fn drop(&mut self) {
        // SAFETY: the handle is ours and closed once; closing it ends the job.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
