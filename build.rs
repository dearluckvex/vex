use std::env;
use std::path::PathBuf;
use std::fs;

fn main() {
    #[cfg(target_os = "windows")]
    {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR")
            .unwrap_or_else(|_| ".".to_string());
        
        let dll_src = PathBuf::from(&manifest_dir).join("wintun.dll");
        
        if dll_src.exists() {
            let profile = env::var("PROFILE")
                .unwrap_or_else(|_| "debug".to_string());
            
            // OUT_DIR path for profile build is like:
            // target/profile/deps/build-xxx/out
            // We want: target/profile
            let out_dir = env::var("OUT_DIR").unwrap();
            let out_path = PathBuf::from(&out_dir);
            
            // Navigate up: out -> build-xxx -> deps -> profile -> target
            let target_root = out_path
                .ancestors()
                .nth(3)  // Skip 3 levels: /out, /build-xxx, /deps
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(&manifest_dir).join("target").join(&profile));
            
            let dll_dest = target_root.join("wintun.dll");
            
            // 创建目标目录
            if let Some(parent) = dll_dest.parent() {
                let _ = fs::create_dir_all(parent);
            }
            
            // 复制 DLL
            match fs::copy(&dll_src, &dll_dest) {
                Ok(_) => {
                    println!("cargo:warning=✓ 已复制 wintun.dll 到 {}", dll_dest.display());
                }
                Err(e) => {
                    println!("cargo:warning=✗ 复制 wintun.dll 失败: {}", e);
                }
            }
            
            println!("cargo:rerun-if-changed={}", dll_src.display());
        }
    }
}

