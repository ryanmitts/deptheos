// A per-user cache of the vendored native libraries, shared by the build
// scripts of deptheos_raw and deptheos_dng. Cargo keys build-script output
// on the package's location, so without it every worktree compiles the same
// C++ again. A library group is stored under a key hashed from everything
// that decides its bytes (sources, build scripts, compiler, target, profile,
// compiler environment).
//
// Location: `DEPTHEOS_NATIVE_CACHE` if set (`off` disables it), else
// `%LOCALAPPDATA%\deptheos\native-cache`, `~/Library/Caches/deptheos/
// native-cache` or `$XDG_CACHE_HOME/deptheos/native-cache`. Nothing is
// evicted; the folder is safe to delete at any time.

use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};

/// Bump to invalidate every stored entry.
const CACHE_VERSION: u32 = 1;

/// Builds the static libraries `libs` with `build`, or links stored copies.
///
/// `build` must compile exactly `libs` with `cc`, in link order. `inputs`
/// are the source trees and build-script files that decide the result.
/// Callers print their own `rerun-if-changed` lines, since a hit skips
/// `build`.
pub fn build_or_reuse(group: &str, libs: &[&str], inputs: &[PathBuf], build: impl FnOnce()) {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-env-changed=DEPTHEOS_NATIVE_CACHE");

    let Some(root) = cache_root() else {
        build();
        return;
    };
    let entry = root.join(format!("{group}-{}", cache_key(group, inputs)));
    let files: Vec<String> = libs.iter().map(|lib| static_lib_file(lib)).collect();

    if files.iter().all(|f| entry.join(f).is_file()) {
        for file in &files {
            let dst = out_dir.join(file);
            let _ = std::fs::remove_file(&dst);
            if std::fs::hard_link(entry.join(file), &dst).is_err() {
                std::fs::copy(entry.join(file), &dst)
                    .unwrap_or_else(|e| panic!("copying {file} from the native cache: {e}"));
            }
        }
        for lib in libs {
            println!("cargo:rustc-link-lib=static={lib}");
        }
        println!("cargo:rustc-link-search=native={}", out_dir.display());
        return;
    }

    // A hard link from an earlier hit would let the compile below write
    // through into the stored entry.
    for file in &files {
        let _ = std::fs::remove_file(out_dir.join(file));
    }
    build();
    store(&root, &entry, &out_dir, &files);
}

/// Copies the built libraries into the cache, renaming a temporary folder
/// into place so a reader sees all of it or none. A failure only costs the
/// next build its hit.
fn store(root: &Path, entry: &Path, out_dir: &Path, files: &[String]) {
    let temp = root.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let stored = std::fs::create_dir_all(&temp).is_ok()
        && files.iter().all(|f| std::fs::copy(out_dir.join(f), temp.join(f)).is_ok())
        && std::fs::rename(&temp, entry).is_ok();
    if !stored {
        // Most often another build stored the same entry first.
        let _ = std::fs::remove_dir_all(&temp);
    }
}

fn cache_root() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("DEPTHEOS_NATIVE_CACHE") {
        return match dir.as_str() {
            "" | "0" | "off" | "false" => None,
            _ => Some(PathBuf::from(dir)),
        };
    }
    let base = if let Ok(local) = std::env::var("LOCALAPPDATA") {
        PathBuf::from(local)
    } else if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        PathBuf::from(xdg)
    } else {
        let home = PathBuf::from(std::env::var("HOME").ok()?);
        if std::env::var("HOST").is_ok_and(|h| h.contains("apple")) {
            home.join("Library/Caches")
        } else {
            home.join(".cache")
        }
    };
    Some(base.join("deptheos").join("native-cache"))
}

/// The file `cc` writes for the static library `name`.
fn static_lib_file(name: &str) -> String {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        format!("{name}.lib")
    } else {
        format!("lib{name}.a")
    }
}

/// A hex digest of everything that decides the libraries' bytes.
fn cache_key(group: &str, inputs: &[PathBuf]) -> String {
    let mut h = DefaultHasher::new(); // fixed keys: stable across runs
    h.write_u32(CACHE_VERSION);
    h.write(group.as_bytes());

    // Target and profile, as cargo hands them to `cc`.
    for var in ["TARGET", "HOST", "OPT_LEVEL", "DEBUG", "CARGO_CFG_TARGET_FEATURE"] {
        hash_env(&mut h, var);
    }
    // The compiler environment `cc` reads, plain and per target.
    let target = std::env::var("TARGET").unwrap_or_default();
    for base in ["CC", "CXX", "CFLAGS", "CXXFLAGS", "AR", "ARFLAGS", "CRATE_CC_NO_DEFAULTS"] {
        hash_env(&mut h, base);
        hash_env(&mut h, &format!("TARGET_{base}"));
        hash_env(&mut h, &format!("{base}_{target}"));
        hash_env(&mut h, &format!("{base}_{}", target.replace('-', "_")));
    }
    for var in ["MACOSX_DEPLOYMENT_TARGET", "IPHONEOS_DEPLOYMENT_TARGET"] {
        hash_env(&mut h, var);
    }
    // The compilers themselves. MSVC's path names its exact version; the
    // others (/usr/bin/clang is a shim) report it.
    for cpp in [false, true] {
        let compiler = cc::Build::new().cpp(cpp).get_compiler();
        h.write(compiler.path().to_string_lossy().as_bytes());
        if !compiler.is_like_msvc()
            && let Ok(out) = std::process::Command::new(compiler.path()).arg("--version").output()
        {
            h.write(&out.stdout);
        }
    }
    for input in inputs {
        hash_tree(&mut h, input, input);
    }
    format!("{:016x}", h.finish())
}

fn hash_env(h: &mut DefaultHasher, var: &str) {
    h.write(var.as_bytes());
    match std::env::var_os(var) {
        Some(value) => {
            h.write_u8(1);
            h.write(value.to_string_lossy().as_bytes());
        }
        None => h.write_u8(0),
    }
}

/// Paths (relative to `root`) and contents of every file under `path`, in a
/// fixed order. Dot-entries are skipped: a submodule's `.git` file names
/// the checkout it lives in, which differs between worktrees.
fn hash_tree(h: &mut DefaultHasher, root: &Path, path: &Path) {
    let relative = path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/");
    if path.is_dir() {
        let mut children: Vec<PathBuf> = std::fs::read_dir(path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
            .flatten()
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path())
            .collect();
        children.sort();
        h.write(b"d");
        h.write(relative.as_bytes());
        for child in children {
            hash_tree(h, root, &child);
        }
    } else {
        let bytes =
            std::fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        h.write(b"f");
        h.write(relative.as_bytes());
        h.write_usize(bytes.len());
        h.write(&bytes);
    }
}
