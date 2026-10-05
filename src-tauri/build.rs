fn main() {
    // The default manifest arrives as an embedded winres resource — which
    // only reaches BINS. The `cargo test` lib harness is not a bin, so it
    // binds comctl32 v5 and dies at load with STATUS_ENTRYPOINT_NOT_FOUND
    // on the comctl6-only TaskDialogIndirect import that the dialog/tao code
    // path pulls in (tauri-apps/tauri#13419). Emit the Common-Controls v6
    // dependency through the LINKER instead — it reaches every linked
    // product, test harnesses included.
    #[cfg(windows)]
    {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='Win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest()),
    )
    .expect("tauri build script");
}
