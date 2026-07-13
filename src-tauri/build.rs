fn main() {
    // TEST binaries need a Common-Controls v6 manifest on Windows/MSVC:
    // building a real (mock-runtime) Tauri app in tests links tauri's menu
    // machinery, whose `TaskDialogIndirect` import only exists in comctl32 v6
    // — without the manifest the test exe dies at load time with
    // STATUS_ENTRYPOINT_NOT_FOUND. The APP binary already gets the full
    // manifest from `tauri_build::build()`'s compiled resource; this covers
    // tests only and changes nothing about the shipped exe.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-tests=/MANIFESTDEPENDENCY:type='win32' \
             name='Microsoft.Windows.Common-Controls' version='6.0.0.0' \
             publicKeyToken='6595b64144ccf1df' language='*' processorArchitecture='*'"
        );
    }

    tauri_build::build()
}
