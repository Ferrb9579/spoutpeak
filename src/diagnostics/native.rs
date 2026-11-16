//! Best-effort Windows unhandled-exception capture on a prestarted thread.
//! The exception callback only signals/waits: it never allocates or logs.
//! See Microsoft's MiniDumpWriteDump guidance on avoiding the failing thread.
use std::{
    fs::File,
    io::{self, Write},
    os::windows::io::AsRawHandle,
    path::Path,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    },
};
use windows::{
    Win32::{
        Foundation::HANDLE,
        System::{
            Diagnostics::Debug::{
                EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpNormal,
                MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump,
                SetUnhandledExceptionFilter,
            },
            LibraryLoader::{
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
                GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
                GetModuleHandleExW,
            },
            Threading::{
                CreateEventW, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, SetEvent,
                WaitForSingleObject,
            },
        },
    },
    core::PCWSTR,
};

struct Signals {
    // Process-lifetime event handles; the OS closes them at process exit.
    request: usize,
    done: usize,
    active: AtomicBool,
    exception: AtomicUsize,
    thread: AtomicU32,
}
static SIGNALS: OnceLock<Signals> = OnceLock::new();

pub(super) fn install(base: &Path) -> io::Result<()> {
    let request =
        unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.map_err(io::Error::other)?;
    let done =
        unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(io::Error::other)?;
    SIGNALS
        .set(Signals {
            request: request.0 as usize,
            done: done.0 as usize,
            active: AtomicBool::new(false),
            exception: AtomicUsize::new(0),
            thread: AtomicU32::new(0),
        })
        .map_err(|_| io::Error::other("native crash handler already installed"))?;
    let base = base.to_owned();
    std::thread::Builder::new()
        .name("native-crash-writer".into())
        .spawn(move || {
            let Some(signals) = SIGNALS.get() else { return };
            unsafe {
                WaitForSingleObject(HANDLE(signals.request as _), u32::MAX);
            }
            let exception = signals.exception.load(Ordering::Acquire) as *mut EXCEPTION_POINTERS;
            let thread = signals.thread.load(Ordering::Acquire);
            let mut details = format!(
                "Spoutpeak {}\nRevision: {}\nUTC: {}\nNative exception on OS thread {thread}\n",
                env!("CARGO_PKG_VERSION"),
                option_env!("SPOUTPEAK_BUILD_REVISION").unwrap_or("unknown"),
                chrono::Utc::now().to_rfc3339()
            );
            // The faulting thread remains blocked in the exception callback,
            // keeping the exception record/context alive until this writer ends.
            if let Some(record) =
                unsafe { exception.as_ref().and_then(|p| p.ExceptionRecord.as_ref()) }
            {
                details.push_str(&format!(
                    "Exception code: 0x{:08X}\nAddress: {:?}\n",
                    record.ExceptionCode.0 as u32, record.ExceptionAddress
                ));
                let mut module = Default::default();
                if unsafe {
                    GetModuleHandleExW(
                        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                            | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                        PCWSTR(record.ExceptionAddress.cast()),
                        &mut module,
                    )
                }
                .is_ok()
                {
                    let mut name = [0u16; 32768];
                    let len = unsafe { GetModuleFileNameW(Some(module), &mut name) } as usize;
                    details.push_str(&format!(
                        "Module: {}\n",
                        String::from_utf16_lossy(&name[..len])
                    ));
                }
            }
            // Save a readable report before attempting DbgHelp, which can fail
            // in a corrupt process. No application logger or application lock.
            let report = base.with_extension("native.txt");
            let _ = std::fs::write(&report, super::redact(&details));
            let result = File::create(base.with_extension("dmp")).and_then(|file| {
                let info = MINIDUMP_EXCEPTION_INFORMATION {
                    ThreadId: thread,
                    ExceptionPointers: exception,
                    ClientPointers: false.into(),
                };
                unsafe {
                    MiniDumpWriteDump(
                        GetCurrentProcess(),
                        GetCurrentProcessId(),
                        HANDLE(file.as_raw_handle()),
                        MiniDumpNormal | MiniDumpWithThreadInfo | MiniDumpWithUnloadedModules,
                        Some(&info),
                        None,
                        None,
                    )
                }
                .map_err(io::Error::other)?;
                file.sync_all()
            });
            if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(report) {
                let _ = writeln!(file, "Crash dump result: {result:?}");
                let _ = file.sync_all();
            }
            unsafe {
                let _ = SetEvent(HANDLE(signals.done as _));
            }
        })?;
    unsafe {
        SetUnhandledExceptionFilter(Some(exception_filter));
    }
    tracing::info!("Windows native crash capture installed");
    Ok(())
}

unsafe extern "system" fn exception_filter(exception: *const EXCEPTION_POINTERS) -> i32 {
    if let Some(signals) = SIGNALS.get() {
        if !signals.active.swap(true, Ordering::AcqRel) {
            signals
                .thread
                .store(unsafe { GetCurrentThreadId() }, Ordering::Release);
            signals
                .exception
                .store(exception as usize, Ordering::Release);
            unsafe {
                let _ = SetEvent(HANDLE(signals.request as _));
            }
        }
        // Bound the delay if DbgHelp cannot finish after a severe failure.
        unsafe {
            WaitForSingleObject(HANDLE(signals.done as _), 10_000);
        }
        // EXCEPTION_EXECUTE_HANDLER: terminate; never resume at the fault.
        return 1;
    }
    0
}
