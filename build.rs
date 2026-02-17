use std::path::PathBuf;
use std::process::Command;

fn run_command(command: &mut Command, label: &str) {
    let status = command.status().unwrap_or_else(|err| {
        panic!("failed to spawn {label}: {err}");
    });
    if !status.success() {
        panic!("{label} failed with status {status}");
    }
}

fn main() {
    let profile = std::env::var("PROFILE").unwrap_or_default();

    if profile != "release" {
        return;
    }

    println!("cargo:rustc-cfg=embed_frontend");

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let frontend_dir = PathBuf::from(manifest_dir).join("frontend");

    println!("cargo:rerun-if-changed=frontend/index.html");
    println!("cargo:rerun-if-changed=frontend/package.json");
    println!("cargo:rerun-if-changed=frontend/bun.lock");
    println!("cargo:rerun-if-changed=frontend/tsconfig.json");
    println!("cargo:rerun-if-changed=frontend/tsconfig.app.json");
    println!("cargo:rerun-if-changed=frontend/tsconfig.node.json");
    println!("cargo:rerun-if-changed=frontend/vite.config.ts");
    println!("cargo:rerun-if-changed=frontend/tailwind.config.cjs");
    println!("cargo:rerun-if-changed=frontend/postcss.config.js");
    println!("cargo:rerun-if-changed=frontend/components.json");
    println!("cargo:rerun-if-changed=frontend/public");
    println!("cargo:rerun-if-changed=frontend/src");

    let mut install = Command::new("bun");
    install.arg("install").current_dir(&frontend_dir);
    run_command(&mut install, "bun install");

    let mut build = Command::new("bun");
    build.arg("run").arg("build").current_dir(&frontend_dir);
    run_command(&mut build, "bun run build");
}
