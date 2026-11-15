fn main() {
    let target = std::env::var("TARGET").expect("Cargo must provide the target triple");
    match target.as_str() {
        "aarch64-unknown-linux-gnu" => copy_pdfium_worker(&target),
        _ => panic!("HB PDF Studio desktop builds only support Linux AArch64 (target: {target})"),
    }

    for dir in ["resources/tessdata", "resources/fonts"] {
        if !std::path::Path::new(dir).exists() {
            let _ = std::fs::create_dir_all(dir);
        }
    }

    tauri_build::build();
}

fn copy_pdfium_worker(target: &str) {
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let worker_name = "pdfium-worker";
    let profile_dirs = || -> Vec<std::path::PathBuf> {
        match std::env::var("OUT_DIR") {
            Ok(out_dir) => std::path::Path::new(&out_dir)
                .ancestors()
                .skip(3)
                .take(2)
                .map(|dir| dir.join(worker_name))
                .collect(),
            Err(_) => Vec::new(),
        }
    };
    let mut candidates = profile_dirs();
    candidates.push(std::path::PathBuf::from("../../target").join(&profile).join(worker_name));

    let destination = std::path::PathBuf::from("binaries")
        .join(format!("pdfium-worker-{target}"));
    if let Some(source) = candidates.iter().find(|path| path.is_file()) {
        println!("cargo:rerun-if-changed={}", source.display());
        let _ = std::fs::create_dir_all("binaries");
        if let Err(error) = std::fs::copy(source, &destination) {
            println!(
                "cargo:warning=pdfium-worker copy failed ({} -> {}): {}",
                source.display(),
                destination.display(),
                error
            );
        }
    } else {
        println!(
            "cargo:warning=pdfium-worker not found in {:?}; build it with `cargo build -p pdfium-worker`",
            candidates.iter().map(|path| path.display().to_string()).collect::<Vec<_>>()
        );
    }
}
