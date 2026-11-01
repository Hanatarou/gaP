use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=res/resources.rc");
    println!("cargo:rerun-if-changed=res/icon.ico");
    println!("cargo:rerun-if-changed=res/gap.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Each resource compiler (rc.exe, llvm-rc, windres) resolves the relative paths of an .rc
        // differently (from the .rc, from the current folder...). To behave the same everywhere, the
        // .rc is copied to OUT_DIR with the file names made absolute (relative to `res/`).
        let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
        let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let res_fwd = root.join("res").to_string_lossy().replace('\\', "/");
        let rc = std::fs::read_to_string(root.join("res/resources.rc"))
            .unwrap()
            .replace("\"icon.ico\"", &format!("\"{res_fwd}/icon.ico\""))
            .replace("\"gap.manifest\"", &format!("\"{res_fwd}/gap.manifest\""));
        let rc_path = out.join("resources_abs.rc");
        std::fs::write(&rc_path, rc).unwrap();
        embed_resource::compile(&rc_path, embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
