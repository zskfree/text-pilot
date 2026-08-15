use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/text-pilot.rc");
    println!("cargo:rerun-if-changed=assets/text-pilot.ico");
    println!("cargo:rerun-if-changed=assets/text-pilot.exe.manifest");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("text-pilot.res");
    let resource_dir = manifest_dir.join("assets");
    let compiler = find_resource_compiler().unwrap_or_else(|| PathBuf::from("rc.exe"));
    let status = Command::new(&compiler)
        .current_dir(&resource_dir)
        .arg("/nologo")
        .arg("/fo")
        .arg(&output)
        .arg("text-pilot.rc")
        .status()
        .unwrap_or_else(|error| panic!("failed to start {}: {error}", compiler.display()));
    assert!(status.success(), "Windows resource compilation failed");
    println!("cargo:rustc-link-arg-bin=TextPilot={}", output.display());
}

fn find_resource_compiler() -> Option<PathBuf> {
    for variable in ["WindowsSdkVerBinPath", "WindowsSdkBinPath"] {
        if let Some(path) = env::var_os(variable) {
            let candidate = PathBuf::from(path).join("x64").join("rc.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    let program_files = env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
    newest_sdk_compiler(&program_files.join(r"Windows Kits\10\bin"))
}

fn newest_sdk_compiler(bin_root: &Path) -> Option<PathBuf> {
    let mut candidates = fs::read_dir(bin_root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("x64").join("rc.exe"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.pop()
}
