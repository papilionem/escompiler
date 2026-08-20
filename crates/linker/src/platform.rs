//! Platform-specific linker flags.
//!
//! Returns the default link-time libraries and flags required on each
//! target operating system (Linux ELF, macOS Mach-O, etc.).

/// Return platform-specific linker flags for the current target OS.
///
/// On Linux: `-lm -lpthread -ldl -lgcc_s` (math, POSIX threads, dynamic
/// loader, and GCC unwinding — required by Rust staticlibs).
/// On macOS: `-lSystem` (system library umbrella).
/// On Windows: Windows system libraries required by Rust's std
/// (WinSock, NT, userenv, bcrypt, etc.).
/// On other platforms: an empty list (best-effort; may need extension).
pub fn platform_flags() -> Vec<String> {
    if cfg!(target_os = "linux") {
        vec![
            "-lm".to_string(),
            "-lpthread".to_string(),
            "-ldl".to_string(),
            "-lgcc_s".to_string(),
        ]
    } else if cfg!(target_os = "macos") {
        vec!["-lSystem".to_string()]
    } else if cfg!(target_os = "windows") {
        // Rust's std on Windows depends on these system libraries.
        // Use -l flags so the linker searches its library paths.
        vec![
            "-lws2_32".to_string(),
            "-lntdll".to_string(),
            "-luserenv".to_string(),
            "-lbcrypt".to_string(),
            "-ladvapi32".to_string(),
            "-lkernel32".to_string(),
            "-lsynchronization".to_string(),
        ]
    } else {
        Vec::new()
    }
}
