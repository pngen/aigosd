use serde::Serialize;
use std::env;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::SystemTime;

static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);
static WRITE_FAILURE_DETECTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    #[allow(dead_code)]
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn as_str(&self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

struct Logger {
    file: Option<File>,
    structured: bool,
    min_level: Level,
}

#[derive(Serialize)]
struct StructuredRecord<'a> {
    ts: &'a serde_json::value::RawValue,
    level: &'static str,
    msg: &'a str,
}

impl Logger {
    fn format_msg(&self, level: Level, msg: &str) -> io::Result<String> {
        let elapsed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        if self.structured {
            let millis = elapsed.as_millis();
            let ts = serde_json::value::RawValue::from_string(format!(
                "{}.{:03}",
                millis / 1000,
                millis % 1000
            ))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            serde_json::to_string(&StructuredRecord {
                ts: &ts,
                level: level.as_str(),
                msg,
            })
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        } else {
            Ok(format!("[{}] [{}] {}", elapsed.as_secs(), level.as_str(), msg))
        }
    }

    fn write(&mut self, level: Level, msg: &str) -> io::Result<()> {
        if (level as u8) < (self.min_level as u8) {
            return Ok(());
        }
        let line = self.format_msg(level, msg)?;
        let mut file_error = None;
        if let Some(ref mut f) = self.file {
            if let Err(error) = writeln!(f, "{}", line) {
                file_error = Some(error);
            }
        }
        let stderr_result = writeln!(io::stderr().lock(), "{}", line);
        if let Some(error) = file_error {
            return Err(error);
        }
        stderr_result
    }
}

fn logger_guard() -> MutexGuard<'static, Option<Logger>> {
    LOGGER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn report_write_failure(failure_flag: &AtomicBool, error: &io::Error) {
    failure_flag.store(true, Ordering::SeqCst);
    let _ = writeln!(
        io::stderr().lock(),
        "[AIGOSD LOGGER ERROR] log write failed: {}",
        error
    );
}

fn reset_write_failure(failure_flag: &AtomicBool) {
    failure_flag.store(false, Ordering::SeqCst);
}

fn ensure_failure_flag_healthy(failure_flag: &AtomicBool) -> io::Result<()> {
    if failure_flag.load(Ordering::SeqCst) {
        Err(io::Error::other(
            "one or more daemon log writes have failed",
        ))
    } else {
        Ok(())
    }
}

fn write_with_failure_tracking_for(
    logger: &mut Logger,
    level: Level,
    msg: &str,
    failure_flag: &AtomicBool,
) {
    if let Err(error) = logger.write(level, msg) {
        report_write_failure(failure_flag, &error);
    }
}

fn write_with_failure_tracking(logger: &mut Logger, level: Level, msg: &str) {
    write_with_failure_tracking_for(logger, level, msg, &WRITE_FAILURE_DETECTED);
}

fn validate_local_log_path(path: &str) -> io::Result<()> {
    crate::config::validate_log_file_path(path).map_err(|error| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("invalid local log path: {error}"),
        )
    })
}

fn normalized_expected_log_path(path: &str) -> io::Result<PathBuf> {
    validate_local_log_path(path)?;

    let runtime_root = fs::canonicalize(env::current_dir()?)?;
    let mut expected_path = runtime_root.clone();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => expected_path.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "log path contains a component outside the runtime directory",
                ));
            }
        }
    }

    if expected_path == runtime_root || !expected_path.starts_with(&runtime_root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "log path does not resolve to a file inside the runtime directory",
        ));
    }

    Ok(expected_path)
}

#[cfg(any(windows, target_os = "linux", target_os = "android"))]
fn ensure_handle_path_validation_supported() -> io::Result<()> {
    Ok(())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
fn ensure_handle_path_validation_supported() -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "configured file logging is unavailable because retained-handle path validation is unsupported on this platform",
    ))
}

fn open_log_file(path: &Path, create: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.append(true).create(create);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

fn validate_opened_log_file(path: &Path, expected_path: &Path, file: &File) -> io::Result<()> {
    let opened_metadata = file.metadata()?;
    let current_metadata = fs::metadata(path)?;
    if !opened_metadata.is_file() || !current_metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "configured log path is not a regular file",
        ));
    }
    validate_file_identity(path, file, &opened_metadata, &current_metadata)?;
    validate_opened_handle_path(expected_path, file)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn validate_opened_handle_path(expected_path: &Path, file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let descriptor_path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
    let opened_path = fs::read_link(descriptor_path)?;
    if opened_path != expected_path {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "retained log handle resolves outside the expected path: expected {}, opened {}",
                expected_path.display(),
                opened_path.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_opened_handle_path(expected_path: &Path, file: &File) -> io::Result<()> {
    let information = windows_file_information(file)?;
    reject_windows_reparse_attributes(information.file_attributes)?;

    let expected_path = normalize_windows_dos_path(expected_path)?;
    let opened_path = normalize_windows_dos_path(Path::new(&windows_final_path(file)?))?;
    if !windows_paths_match_exactly(&expected_path, &opened_path) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "retained log handle resolves outside the expected path: expected {}, opened {}",
                expected_path, opened_path
            ),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_paths_match_exactly(expected: &str, opened: &str) -> bool {
    expected == opened
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn validate_opened_handle_path(_expected_path: &Path, _file: &File) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "configured file logging is unavailable because retained-handle path validation is unsupported on this Unix platform",
    ))
}

#[cfg(not(any(unix, windows)))]
fn validate_opened_handle_path(_expected_path: &Path, _file: &File) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "configured file logging is unavailable because retained-handle path validation is unsupported on this platform",
    ))
}

#[cfg(unix)]
fn validate_file_identity(
    _path: &Path,
    _file: &File,
    opened: &Metadata,
    current: &Metadata,
) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    if opened.dev() != current.dev()
        || opened.ino() != current.ino()
        || opened.nlink() != 1
        || current.nlink() != 1
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "log path changed during open or is hard-linked",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_file_identity(
    path: &Path,
    file: &File,
    _opened: &Metadata,
    _current: &Metadata,
) -> io::Result<()> {
    let current_file = open_log_file(path, false)?;
    if !current_file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "configured log path is not a regular file",
        ));
    }
    let opened_identity = windows_file_identity(file)?;
    let current_identity = windows_file_identity(&current_file)?;
    if opened_identity != current_identity || opened_identity.number_of_links != 1 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "log path identity is unavailable, changed during open, or is hard-linked",
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn validate_file_identity(
    _path: &Path,
    _file: &File,
    _opened: &Metadata,
    _current: &Metadata,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "safe log-file identity checks are unavailable on this platform",
    ))
}

#[cfg(windows)]
#[derive(Debug, PartialEq, Eq)]
struct WindowsFileIdentity {
    volume_serial_number: u32,
    file_index: u64,
    number_of_links: u32,
}

#[cfg(windows)]
struct WindowsFileInformation {
    identity: WindowsFileIdentity,
    file_attributes: u32,
}

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

#[cfg(windows)]
fn windows_file_information(file: &File) -> io::Result<WindowsFileInformation> {
    use std::ffi::c_void;
    use std::mem;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    struct FileTime {
        low_date_time: u32,
        high_date_time: u32,
    }

    #[repr(C)]
    struct ByHandleFileInformation {
        file_attributes: u32,
        creation_time: FileTime,
        last_access_time: FileTime,
        last_write_time: FileTime,
        volume_serial_number: u32,
        file_size_high: u32,
        file_size_low: u32,
        number_of_links: u32,
        file_index_high: u32,
        file_index_low: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandle(
            file: *mut c_void,
            information: *mut ByHandleFileInformation,
        ) -> i32;
    }

    let mut information: ByHandleFileInformation = unsafe { mem::zeroed() };
    let result = unsafe {
        GetFileInformationByHandle(
            file.as_raw_handle(),
            &mut information as *mut ByHandleFileInformation,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(WindowsFileInformation {
        identity: WindowsFileIdentity {
            volume_serial_number: information.volume_serial_number,
            file_index: (u64::from(information.file_index_high) << 32)
                | u64::from(information.file_index_low),
            number_of_links: information.number_of_links,
        },
        file_attributes: information.file_attributes,
    })
}

#[cfg(windows)]
fn windows_file_identity(file: &File) -> io::Result<WindowsFileIdentity> {
    Ok(windows_file_information(file)?.identity)
}

#[cfg(windows)]
fn reject_windows_reparse_attributes(file_attributes: u32) -> io::Result<()> {
    if file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "retained log handle has reparse-point attributes",
        ))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn windows_final_path(file: &File) -> io::Result<String> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::ptr;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetFinalPathNameByHandleW(
            file: *mut c_void,
            file_path: *mut u16,
            file_path_len: u32,
            flags: u32,
        ) -> u32;
    }

    let handle = file.as_raw_handle();
    let required = unsafe { GetFinalPathNameByHandleW(handle, ptr::null_mut(), 0, 0) };
    if required == 0 {
        return Err(io::Error::last_os_error());
    }

    let mut buffer = vec![0u16; required as usize + 1];
    loop {
        let length = unsafe {
            GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, 0)
        };
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        if length < buffer.len() as u32 {
            buffer.truncate(length as usize);
            return String::from_utf16(&buffer).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("retained log handle path is not valid UTF-16: {error}"),
                )
            });
        }
        buffer.resize(length as usize + 1, 0);
    }
}

#[cfg(windows)]
fn normalize_windows_dos_path(path: &Path) -> io::Result<String> {
    let path = path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "retained log handle path has no lossless Unicode representation",
        )
    })?;
    let path = path.replace('/', "\\");

    if let Some(unc_path) = path.strip_prefix(r"\\?\UNC\") {
        return Ok(format!(r"\\{}", unc_path));
    }
    if let Some(dos_path) = path.strip_prefix(r"\\?\") {
        return Ok(dos_path.to_string());
    }
    if path.starts_with(r"\\.\") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "retained log handle path is not a normalized DOS path",
        ));
    }
    Ok(path)
}

pub fn init(log_type: &str, file_path: Option<&str>) -> io::Result<()> {
    let file = match file_path {
        Some(path) => {
            let expected_path = normalized_expected_log_path(path)?;
            ensure_handle_path_validation_supported()?;
            let file = open_log_file(Path::new(path), true)?;
            validate_local_log_path(path)?;
            validate_opened_log_file(Path::new(path), &expected_path, &file)?;
            Some(file)
        }
        None => None,
    };
    let logger = Logger {
        file,
        structured: log_type == "structured",
        min_level: Level::Info,
    };
    let mut guard = logger_guard();
    *guard = Some(logger);
    reset_write_failure(&WRITE_FAILURE_DETECTED);
    Ok(())
}

fn log(level: Level, msg: &str) {
    let mut guard = logger_guard();
    if let Some(ref mut logger) = *guard {
        write_with_failure_tracking(logger, level, msg);
    } else if let Err(error) = writeln!(io::stderr().lock(), "[{}] {}", level.as_str(), msg) {
        report_write_failure(&WRITE_FAILURE_DETECTED, &error);
    }
}

pub fn write_failure_detected() -> bool {
    WRITE_FAILURE_DETECTED.load(Ordering::SeqCst)
}

pub fn ensure_healthy() -> io::Result<()> {
    ensure_failure_flag_healthy(&WRITE_FAILURE_DETECTED)
}

#[allow(dead_code)]
pub fn debug(msg: &str) {
    log(Level::Debug, msg);
}
pub fn info(msg: &str) {
    log(Level::Info, msg);
}
pub fn warn(msg: &str) {
    log(Level::Warn, msg);
}
pub fn error(msg: &str) {
    log(Level::Error, msg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_file(label: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "aigosd-logging-{label}-{}-{unique}.log",
            std::process::id()
        ))
    }

    #[test]
    fn structured_messages_are_valid_json_for_arbitrary_text() {
        let logger = Logger {
            file: None,
            structured: true,
            min_level: Level::Info,
        };
        let message = "quoted \"value\" with \\ slash and control \u{0001}";
        let line = logger
            .format_msg(Level::Error, message)
            .expect("serialize structured record");
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON log");

        assert_eq!(value["level"], "ERROR");
        assert_eq!(value["msg"], message);
    }

    #[test]
    fn structured_ts_uses_millisecond_precision_with_epoch_semantics() {
        let logger = Logger {
            file: None,
            structured: true,
            min_level: Level::Info,
        };
        let line = logger
            .format_msg(Level::Info, "timestamp precision")
            .expect("serialize structured record");
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON log");
        assert_eq!(value["level"], "INFO");
        assert_eq!(value["msg"], "timestamp precision");

        // The emitted ts token itself must carry exactly three fractional digits.
        let ts_text = line
            .split_once("\"ts\":")
            .expect("structured log must contain a ts field")
            .1
            .split([',', '}'])
            .next()
            .expect("ts field must be terminated");
        let (whole, fraction) = ts_text
            .split_once('.')
            .expect("ts must include a fractional part");
        assert!(
            !whole.is_empty() && whole.chars().all(|c| c.is_ascii_digit()),
            "ts integer part must be whole epoch seconds: {ts_text}"
        );
        assert_eq!(
            fraction.len(),
            3,
            "ts must have exactly three fractional digits: {ts_text}"
        );
        assert!(fraction.chars().all(|c| c.is_ascii_digit()));

        // The parsed value must remain Unix epoch seconds close to the wall clock.
        let ts = value["ts"].as_f64().expect("ts must remain a JSON number");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_secs_f64();
        assert!(
            (now - ts).abs() < 2.0,
            "ts must reflect current epoch seconds: {ts} vs {now}"
        );
        assert!((0.0..1.0).contains(&ts.fract()));
    }

    #[test]
    fn requested_invalid_log_path_is_returned() {
        let missing_parent = temporary_file("missing-parent")
            .file_name()
            .expect("temporary file name")
            .to_owned();
        let path = std::path::PathBuf::from(missing_parent).join("aigosd.log");
        let error = init("plaintext", path.to_str()).expect_err("open failure must propagate");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn expected_log_path_is_canonical_runtime_root_plus_normal_components() {
        let file_name = temporary_file("expected-path")
            .file_name()
            .expect("temporary file name")
            .to_owned();
        let relative_path = PathBuf::from(".").join(&file_name);

        let expected = normalized_expected_log_path(
            relative_path
                .to_str()
                .expect("temporary relative path is Unicode"),
        )
        .expect("derive expected local log path");

        assert_eq!(
            expected,
            fs::canonicalize(env::current_dir().expect("current directory"))
                .expect("canonical runtime root")
                .join(file_name)
        );
    }

    #[cfg(any(windows, target_os = "linux", target_os = "android"))]
    #[test]
    fn retained_handle_path_accepts_its_exact_expected_path() {
        let path = temporary_file("handle-path-match");
        fs::write(&path, b"").expect("create handle-path fixture");
        let expected = fs::canonicalize(&path).expect("canonical handle-path fixture");
        let opened = open_log_file(&path, false).expect("open handle-path fixture");

        validate_opened_handle_path(&expected, &opened)
            .expect("retained handle should match its expected path");

        drop(opened);
        fs::remove_file(path).expect("remove handle-path fixture");
    }

    #[cfg(any(windows, target_os = "linux", target_os = "android"))]
    #[test]
    fn retained_handle_path_rejects_a_different_expected_path() {
        let opened_path = temporary_file("handle-path-opened");
        let expected_path = temporary_file("handle-path-expected");
        fs::write(&opened_path, b"opened").expect("create opened handle-path fixture");
        fs::write(&expected_path, b"expected").expect("create expected handle-path fixture");
        let expected = fs::canonicalize(&expected_path).expect("canonical expected fixture");
        let opened =
            open_log_file(&opened_path, false).expect("open mismatched handle-path fixture");

        let error = validate_opened_handle_path(&expected, &opened)
            .expect_err("retained handle must not match a different expected path");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);

        drop(opened);
        fs::remove_file(opened_path).expect("remove opened handle-path fixture");
        fs::remove_file(expected_path).expect("remove expected handle-path fixture");
    }

    #[cfg(windows)]
    #[test]
    fn windows_dos_path_normalization_handles_extended_prefixes() {
        assert_eq!(
            normalize_windows_dos_path(Path::new(r"\\?\C:\runtime\aigosd.log"))
                .expect("normalize extended DOS path"),
            r"C:\runtime\aigosd.log"
        );
        assert_eq!(
            normalize_windows_dos_path(Path::new(r"\\?\UNC\server\share\aigosd.log"))
                .expect("normalize extended UNC path"),
            r"\\server\share\aigosd.log"
        );
        assert!(windows_paths_match_exactly(
            r"C:\runtime\aigosd.log",
            r"C:\runtime\aigosd.log"
        ));
        assert!(
            !windows_paths_match_exactly(r"C:\runtime\aigosd.log", r"C:\Runtime\aigosd.log"),
            "case-distinct paths must fail closed on case-sensitive Windows directories"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_attributes_are_rejected() {
        let error = reject_windows_reparse_attributes(FILE_ATTRIBUTE_REPARSE_POINT)
            .expect_err("reparse-point handle must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        reject_windows_reparse_attributes(0).expect("ordinary file attributes should pass");
    }

    #[cfg(windows)]
    #[test]
    fn windows_log_open_does_not_follow_a_final_file_symlink() {
        use std::os::windows::fs::symlink_file;

        let target = temporary_file("reparse-target");
        let link = temporary_file("reparse-link");
        fs::write(&target, b"sensitive").expect("create reparse target");
        if let Err(error) = symlink_file(&target, &link) {
            fs::remove_file(target).expect("remove unsupported reparse target fixture");
            if error.kind() == io::ErrorKind::PermissionDenied {
                return;
            }
            panic!("create file symlink fixture: {error}");
        }

        if let Ok(opened) = open_log_file(&link, false) {
            let information =
                windows_file_information(&opened).expect("inspect retained reparse-point handle");
            assert_ne!(
                information.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT,
                0,
                "the hardened open must retain the reparse point rather than follow its target"
            );
            let error = validate_opened_handle_path(&link, &opened)
                .expect_err("retained reparse-point handle must fail closed");
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            drop(opened);
        }
        assert_eq!(
            fs::read(&target).expect("read reparse target"),
            b"sensitive"
        );

        fs::remove_file(link).expect("remove reparse link fixture");
        fs::remove_file(target).expect("remove reparse target fixture");
    }

    #[test]
    fn opened_log_identity_rejects_hard_links_before_writing() {
        let source = temporary_file("hard-link-source");
        let linked = temporary_file("hard-link-target");
        fs::write(&source, b"sensitive").expect("create hard-link source");
        fs::hard_link(&source, &linked).expect("create hard-link fixture");
        let opened = open_log_file(&linked, false).expect("open hard-link fixture");

        let expected = fs::canonicalize(&linked).expect("canonical hard-link fixture");
        let error = validate_opened_log_file(&linked, &expected, &opened)
            .expect_err("hard-linked log must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&source).expect("read source"), b"sensitive");

        drop(opened);
        fs::remove_file(linked).expect("remove hard-link fixture");
        fs::remove_file(source).expect("remove hard-link source");
    }

    #[test]
    fn opened_log_identity_rejects_path_swaps() {
        let opened_path = temporary_file("identity-opened");
        let current_path = temporary_file("identity-current");
        fs::write(&opened_path, b"opened").expect("create opened fixture");
        fs::write(&current_path, b"current").expect("create current fixture");
        let opened = open_log_file(&opened_path, false).expect("open identity fixture");

        let expected = fs::canonicalize(&current_path).expect("canonical current fixture");
        let error = validate_opened_log_file(&current_path, &expected, &opened)
            .expect_err("mismatched opened handle and current path must fail");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);

        drop(opened);
        fs::remove_file(opened_path).expect("remove opened fixture");
        fs::remove_file(current_path).expect("remove current fixture");
    }

    #[test]
    fn file_write_failure_sets_sticky_failure_flag() {
        let path = temporary_file("read-only-handle");
        fs::write(&path, b"").expect("create log fixture");
        let read_only = File::open(&path).expect("open read-only log fixture");
        let mut logger = Logger {
            file: Some(read_only),
            structured: false,
            min_level: Level::Info,
        };

        let failure_flag = AtomicBool::new(false);
        write_with_failure_tracking_for(
            &mut logger,
            Level::Error,
            "must fail to write",
            &failure_flag,
        );
        assert!(failure_flag.load(Ordering::SeqCst));
        assert!(ensure_failure_flag_healthy(&failure_flag).is_err());
        reset_write_failure(&failure_flag);
        ensure_failure_flag_healthy(&failure_flag).expect("reset flag should be healthy");

        drop(logger);
        fs::remove_file(path).expect("remove log fixture");
    }
}
