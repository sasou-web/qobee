fn main() {
    // Re-run when the icon file changes so the Windows resource gets
    // updated (otherwise Cargo caches the build script output).
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    tauri_build::build();
}
