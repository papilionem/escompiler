// @expect-error ESC-E700
// This test verifies that FFI usage without permission is rejected.
// The compiler should produce ESC-E700 when FFI calls appear without --allow-ffi.
// NOTE: This test will pass once the permission gate is implemented.
var ffi_handle = __esc_ffi_open("libc.so");
