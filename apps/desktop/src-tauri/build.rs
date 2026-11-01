fn main() {
    tauri_build::build();
    if std::env::var("CARGO_FEATURE_CUDA").is_ok() {
        copy_cuda_runtime();
    }
}

/// Puts the CUDA runtime beside the app so speech can load it without a reboot.
fn copy_cuda_runtime() {
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    let Ok(cuda) = std::env::var("CUDA_PATH") else {
        return;
    };
    let bin = std::path::PathBuf::from(cuda).join("bin");
    let Ok(entries) = std::fs::read_dir(&bin) else {
        return;
    };
    let out_dir = match std::env::var("OUT_DIR") {
        Ok(dir) => std::path::PathBuf::from(dir),
        Err(_) => return,
    };
    // OUT_DIR is target/<profile>/build/<crate>/out. The executable is three levels up.
    let dest = out_dir.join("../../..");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let text = name.to_string_lossy();
        let runtime = text.starts_with("cudart64_")
            || text.starts_with("cublas64_")
            || text.starts_with("cublasLt64_")
            || text.starts_with("nvJitLink")
            || text.starts_with("nvrtc64_");
        if runtime && text.ends_with(".dll") {
            let _ = std::fs::copy(entry.path(), dest.join(name));
        }
    }
}
