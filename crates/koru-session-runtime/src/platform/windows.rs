// SPDX-License-Identifier: MPL-2.0

use std::ffi::{OsStr, c_void};
use std::fs::File;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::ptr::{null, null_mut};

use koru_session_protocol::GridSize;
use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, STILL_ACTIVE,
    SetHandleInformation, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
    SetConsoleCtrlHandler,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};

use super::{PlatformControl, PlatformWaiter, ProcessExit, SpawnedPty};
use crate::{SpawnRequest, TransportError, TransportResult, TransportStage};

const PSEUDOCONSOLE_RESIZE_QUIRK: u32 = 0x2;
const PSEUDOCONSOLE_WIN32_INPUT_MODE: u32 = 0x4;
const MINIMUM_CONPTY_BUILD: u32 = 17_763;

pub(crate) fn spawn(request: &SpawnRequest) -> TransportResult<SpawnedPty> {
    request.validate()?;
    verify_conpty()?;

    let (pty_input, parent_writer) = pipe()?;
    let (parent_reader, pty_output) = pipe()?;
    parent_writer.clear_inherit()?;
    parent_reader.clear_inherit()?;

    let mut hpcon: HPCON = 0;
    let result = unsafe {
        CreatePseudoConsole(
            coord(request.size),
            pty_input.raw(),
            pty_output.raw(),
            PSEUDOCONSOLE_RESIZE_QUIRK | PSEUDOCONSOLE_WIN32_INPUT_MODE,
            &mut hpcon,
        )
    };
    if result < 0 {
        return Err(hresult_error(TransportStage::OpenPty, result));
    }
    let console = PseudoConsole(hpcon);
    drop(pty_input);
    drop(pty_output);

    let job = create_job()?;
    let mut attributes = AttributeList::new(console.0)?;
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
    startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
    startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
    startup.lpAttributeList = attributes.pointer();

    let mut command_line = command_line(&request.argv);
    process_ctrl_c_in_children()?;
    let environment = environment_block(request);
    let cwd = wide_nul(request.cwd().as_os_str());
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    let created = unsafe {
        CreateProcessW(
            null(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            0,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
            environment.as_ptr().cast(),
            cwd.as_ptr(),
            &startup.StartupInfo,
            &mut process,
        )
    };
    if created == 0 {
        return Err(last_error(TransportStage::Spawn));
    }
    let process_handle = OwnedHandle(process.hProcess);
    let thread_handle = OwnedHandle(process.hThread);

    if unsafe { AssignProcessToJobObject(job.raw(), process_handle.raw()) } == 0 {
        unsafe { TerminateProcess(process_handle.raw(), 1) };
        return Err(last_error(TransportStage::AssignProcessTree));
    }
    if unsafe { ResumeThread(thread_handle.raw()) } == u32::MAX {
        unsafe { TerminateJobObject(job.raw(), 1) };
        return Err(last_error(TransportStage::Resume));
    }
    drop(thread_handle);
    drop(attributes);

    let reader = parent_reader.into_file();
    let writer = parent_writer.into_file();
    Ok(SpawnedPty {
        reader: Box::new(reader),
        writer: Box::new(writer),
        control: Box::new(WindowsControl {
            console: Some(console),
            job,
            process_id: process.dwProcessId,
        }),
        waiter: Box::new(WindowsWaiter {
            process: process_handle,
        }),
    })
}

/// Clear the "ignore Ctrl+C" attribute this process would hand to its child.
///
/// Windows copies that attribute from the creating process to every process it
/// creates, and a process that has it never runs its Ctrl+C handlers: the
/// `0x03` that [`crate::PtyWriter::interrupt`] writes becomes a `CTRL_C_EVENT`
/// in the pseudo-console, and the shell drops it. A process has the attribute
/// whenever anything above it was started with `CREATE_NEW_PROCESS_GROUP`,
/// which Koru itself does when it launches Koru Agent (and so the sessions
/// worker that owns these consoles), and which many launchers do for their
/// own reasons. Ctrl+C in a terminal tab is the only interrupt a person has,
/// so it cannot depend on how this process was started.
///
/// This is the Windows half of what `portable-pty` already does on unix, where
/// the child resets an inherited `SIG_IGN` for `SIGINT` to the default before
/// `exec`. Windows has no per-child switch; the documented way is to clear the
/// attribute here, in the creator, before `CreateProcessW` copies it. That
/// also makes this process handle Ctrl+C by default — harmless for the owner of
/// pseudo-consoles, whose own console is its private one (the sessions worker
/// runs with `CREATE_NO_WINDOW`), and idempotent, so it is repeated per spawn
/// rather than hidden behind a one-time flag another caller could undo.
fn process_ctrl_c_in_children() -> TransportResult<()> {
    if unsafe { SetConsoleCtrlHandler(None, 0) } == 0 {
        Err(last_error(TransportStage::Spawn))
    } else {
        Ok(())
    }
}

struct WindowsControl {
    console: Option<PseudoConsole>,
    job: OwnedHandle,
    process_id: u32,
}

unsafe impl Send for WindowsControl {}

impl PlatformControl for WindowsControl {
    fn resize(&mut self, size: GridSize) -> TransportResult<()> {
        let console = self
            .console
            .as_ref()
            .ok_or_else(|| TransportError::stage(TransportStage::Resize))?;
        let result = unsafe { ResizePseudoConsole(console.0, coord(size)) };
        if result < 0 {
            Err(hresult_error(TransportStage::Resize, result))
        } else {
            Ok(())
        }
    }

    fn terminate_tree(&mut self) -> TransportResult<()> {
        if unsafe { TerminateJobObject(self.job.raw(), 1) } == 0 {
            Err(last_error(TransportStage::Terminate))
        } else {
            Ok(())
        }
    }

    fn close_pty(&mut self) {
        self.console.take();
    }

    fn process_id(&self) -> u32 {
        self.process_id
    }
}

struct WindowsWaiter {
    process: OwnedHandle,
}

unsafe impl Send for WindowsWaiter {}

impl PlatformWaiter for WindowsWaiter {
    fn wait(self: Box<Self>) -> TransportResult<ProcessExit> {
        if unsafe { WaitForSingleObject(self.process.raw(), INFINITE) } != WAIT_OBJECT_0 {
            return Err(last_error(TransportStage::Wait));
        }
        let mut code = STILL_ACTIVE as u32;
        if unsafe { GetExitCodeProcess(self.process.raw(), &mut code) } == 0 {
            return Err(last_error(TransportStage::Wait));
        }
        if code == STILL_ACTIVE as u32 {
            return Err(TransportError::stage(TransportStage::Wait));
        }
        Ok(ProcessExit {
            code,
            success: code == 0,
        })
    }
}

struct OwnedHandle(HANDLE);

unsafe impl Send for OwnedHandle {}

impl OwnedHandle {
    fn raw(&self) -> HANDLE {
        self.0
    }

    fn clear_inherit(&self) -> TransportResult<()> {
        if unsafe { SetHandleInformation(self.0, HANDLE_FLAG_INHERIT, 0) } == 0 {
            Err(last_error(TransportStage::OpenPty))
        } else {
            Ok(())
        }
    }

    fn into_file(self) -> File {
        let handle = self.0;
        std::mem::forget(self);
        unsafe { File::from_raw_handle(handle as RawHandle) }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

struct PseudoConsole(HPCON);

unsafe impl Send for PseudoConsole {}

impl Drop for PseudoConsole {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { ClosePseudoConsole(self.0) };
        }
    }
}

struct AttributeList {
    storage: Vec<usize>,
}

impl AttributeList {
    fn new(console: HPCON) -> TransportResult<Self> {
        let mut bytes = 0;
        unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes) };
        if bytes == 0 {
            return Err(last_error(TransportStage::Spawn));
        }
        let words = bytes.div_ceil(size_of::<usize>());
        let mut list = Self {
            storage: vec![0; words],
        };
        if unsafe { InitializeProcThreadAttributeList(list.pointer(), 1, 0, &mut bytes) } == 0 {
            return Err(last_error(TransportStage::Spawn));
        }
        if unsafe {
            UpdateProcThreadAttribute(
                list.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                console as usize as *const c_void,
                size_of::<HPCON>(),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(last_error(TransportStage::Spawn));
        }
        Ok(list)
    }

    fn pointer(&mut self) -> *mut c_void {
        self.storage.as_mut_ptr().cast()
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.pointer()) };
    }
}

fn pipe() -> TransportResult<(OwnedHandle, OwnedHandle)> {
    let mut read = null_mut();
    let mut write = null_mut();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    if unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } == 0 {
        Err(last_error(TransportStage::OpenPty))
    } else {
        Ok((OwnedHandle(read), OwnedHandle(write)))
    }
}

fn create_job() -> TransportResult<OwnedHandle> {
    let handle = unsafe { CreateJobObjectW(null(), null()) };
    if handle.is_null() {
        return Err(last_error(TransportStage::AssignProcessTree));
    }
    let handle = OwnedHandle(handle);
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            handle.raw(),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(last_error(TransportStage::AssignProcessTree));
    }
    Ok(handle)
}

fn verify_conpty() -> TransportResult<()> {
    let mut info: OSVERSIONINFOW = unsafe { zeroed() };
    info.dwOSVersionInfoSize = size_of::<OSVERSIONINFOW>() as u32;
    if unsafe { RtlGetVersion(&mut info) } != 0
        || info.dwMajorVersion < 10
        || info.dwBuildNumber < MINIMUM_CONPTY_BUILD
    {
        Err(TransportError::stage(TransportStage::UnsupportedPlatform))
    } else {
        Ok(())
    }
}

fn coord(size: GridSize) -> COORD {
    COORD {
        X: size.columns as i16,
        Y: size.rows as i16,
    }
}

fn environment_block(request: &SpawnRequest) -> Vec<u16> {
    let mut entries: Vec<_> = request.environment().into_iter().collect();
    entries.sort_by_key(|(key, _)| windows_sort_key(key));
    let mut block = Vec::new();
    for (key, value) in entries {
        block.extend(key.encode_wide());
        block.push('=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

fn command_line(argv: &[std::ffi::OsString]) -> Vec<u16> {
    let mut line = Vec::new();
    for (index, arg) in argv.iter().enumerate() {
        if index > 0 {
            line.push(b' ' as u16);
        }
        quote_windows_argument(&mut line, &arg.encode_wide().collect::<Vec<_>>());
    }
    line.push(0);
    line
}

fn quote_windows_argument(output: &mut Vec<u16>, argument: &[u16]) {
    if !argument.is_empty()
        && !argument
            .iter()
            .any(|unit| matches!(*unit, 0x20 | 0x09 | 0x22))
    {
        output.extend_from_slice(argument);
        return;
    }
    output.push(b'"' as u16);
    let mut backslashes = 0;
    for &unit in argument {
        if unit == b'\\' as u16 {
            backslashes += 1;
        } else {
            if unit == b'"' as u16 {
                output.extend(std::iter::repeat_n(b'\\' as u16, backslashes + 1));
            }
            output.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
            backslashes = 0;
            output.push(unit);
        }
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    output.push(b'"' as u16);
}

fn windows_sort_key(value: &OsStr) -> Vec<u16> {
    value
        .encode_wide()
        .map(|unit| match unit {
            0x41..=0x5a => unit + 0x20,
            _ => unit,
        })
        .collect()
}

fn wide_nul(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn last_error(stage: TransportStage) -> TransportError {
    TransportError::at(stage, &std::io::Error::last_os_error())
}

fn hresult_error(stage: TransportStage, result: i32) -> TransportError {
    TransportError {
        stage,
        os_code: Some(result),
    }
}

#[cfg(test)]
mod tests {
    use super::command_line;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    #[test]
    fn quotes_windows_arguments_without_changing_boundaries() {
        let line = command_line(&[
            OsString::from("tool.exe"),
            OsString::from("plain"),
            OsString::from("two words"),
            OsString::from("quoted\"value"),
            OsString::from("trailing\\"),
            OsString::new(),
        ]);
        let text = String::from_utf16_lossy(&line[..line.len() - 1]);
        assert_eq!(
            text,
            "tool.exe plain \"two words\" \"quoted\\\"value\" trailing\\ \"\""
        );
    }

    #[test]
    fn command_line_preserves_unpaired_utf16_units() {
        let unusual = OsString::from_wide(&[b'x' as u16, 0xd800, b'y' as u16]);
        let line = command_line(&[OsString::from("tool.exe"), unusual]);
        assert!(
            line.windows(3)
                .any(|window| window == [b'x' as u16, 0xd800, b'y' as u16])
        );
    }
}
