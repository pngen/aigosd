#[cfg(windows)]
use crate::logging;
#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::mem;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
#[cfg(windows)]
use std::process::Child;
#[cfg(windows)]
use std::ptr;

pub fn get_exe_name(mesh_name: &str, layer_name: &str) -> String {
    format!("aigosd-{}@{}.exe", mesh_name, layer_name)
}

pub fn get_layer_exe_path(layer_name: &str) -> String {
    format!(".\\{}.exe", layer_name)
}

#[cfg(windows)]
type Handle = *mut c_void;

#[cfg(windows)]
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: u32 = 9;
#[cfg(windows)]
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
struct JobObjectBasicLimitInformation {
    PerProcessUserTimeLimit: i64,
    PerJobUserTimeLimit: i64,
    LimitFlags: u32,
    MinimumWorkingSetSize: usize,
    MaximumWorkingSetSize: usize,
    ActiveProcessLimit: u32,
    Affinity: usize,
    PriorityClass: u32,
    SchedulingClass: u32,
}

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
struct IoCounters {
    ReadOperationCount: u64,
    WriteOperationCount: u64,
    OtherOperationCount: u64,
    ReadTransferCount: u64,
    WriteTransferCount: u64,
    OtherTransferCount: u64,
}

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
struct JobObjectExtendedLimitInformation {
    BasicLimitInformation: JobObjectBasicLimitInformation,
    IoInfo: IoCounters,
    ProcessMemoryLimit: usize,
    JobMemoryLimit: usize,
    PeakProcessMemoryUsed: usize,
    PeakJobMemoryUsed: usize,
}

#[cfg(windows)]
extern "system" {
    fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(
        job: Handle,
        info_class: u32,
        info: *mut c_void,
        info_len: u32,
    ) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
}

#[cfg(windows)]
pub struct JobObject {
    handle: Handle,
}

#[cfg(windows)]
impl JobObject {
    pub fn new() -> io::Result<Self> {
        let handle = unsafe { CreateJobObjectW(ptr::null_mut(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }

        let mut limits: JobObjectExtendedLimitInformation = unsafe { mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                &mut limits as *mut _ as *mut c_void,
                mem::size_of::<JobObjectExtendedLimitInformation>() as u32,
            )
        };

        if ok == 0 {
            let err = io::Error::last_os_error();
            unsafe {
                CloseHandle(handle);
            }
            return Err(err);
        }

        Ok(Self { handle })
    }

    pub fn assign_child(&self, child: &Child) -> io::Result<()> {
        let ok = unsafe { AssignProcessToJobObject(self.handle, child.as_raw_handle() as Handle) };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
impl Drop for JobObject {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(windows)]
pub fn register_service(
    mesh_name: &str,
    layer_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let svc_name = format!("aigosd-{}@{}", mesh_name, layer_name);
    let _exe_path = get_layer_exe_path(layer_name);
    logging::info(&format!("Registering Windows service: {}", svc_name));
    // Actual implementation requires windows-service crate
    // sc.exe create {svc_name} binPath= "{exe_path} --mesh {mesh_name}"
    Ok(())
}

#[cfg(not(windows))]
pub fn register_service(
    _mesh_name: &str,
    _layer_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    Err("Windows service registration not available on this platform".into())
}
