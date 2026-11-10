// Tool Help reads executable basenames only. No command lines, process memory,
// injection, elevation or shell commands are used.
use std::ffi::c_void;
type Handle = *mut c_void;
#[repr(C)]
struct ProcessEntry {
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    executable: [u16; 260],
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
    fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetLastError() -> u32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn GetForegroundWindow() -> Handle;
    fn GetWindowThreadProcessId(window: Handle, pid: *mut u32) -> u32;
}
struct Snapshot(Handle);
impl Drop for Snapshot {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
pub fn processes() -> Result<(Vec<(u32, String)>, u32), ()> {
    unsafe {
        let handle = CreateToolhelp32Snapshot(2, 0);
        if handle as isize == -1 {
            return Err(());
        }
        let snapshot = Snapshot(handle);
        let mut entry: ProcessEntry = std::mem::zeroed();
        entry.size = std::mem::size_of::<ProcessEntry>() as u32;
        let mut result = Vec::new();
        let mut ok = Process32FirstW(snapshot.0, &mut entry);
        while ok != 0 {
            let len = entry.executable.iter().position(|c| *c == 0).unwrap_or(260);
            result.push((
                entry.pid,
                String::from_utf16_lossy(&entry.executable[..len]),
            ));
            if result.len() > 32768 {
                return Err(());
            }
            ok = Process32NextW(snapshot.0, &mut entry);
        }
        if GetLastError() != 18 {
            return Err(());
        } // ERROR_NO_MORE_FILES
        let mut foreground = 0;
        GetWindowThreadProcessId(GetForegroundWindow(), &mut foreground);
        Ok((result, foreground))
    }
}
