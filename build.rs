use std::env;
use std::path::PathBuf;

fn main() {
    #[cfg(target_os = "windows")]
    {
        // 使用 CARGO_MANIFEST_DIR 获取项目根目录（更可靠）
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
        let dll_src = PathBuf::from(&manifest_dir).join("wintun.dll");
        
        if dll_src.exists() {
            // 获取构建输出目录
            let out_dir = env::var("OUT_DIR").unwrap();
            let out_path = PathBuf::from(&out_dir);
            let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
            
            // 构建目录
            let target_dir = out_path
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(&manifest_dir));
            
            let dll_dest = target_dir.join(&profile).join("wintun.dll");
            
            // 确保目标目录存在
            if let Some(parent) = dll_dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            
            // 复制到输出目录
            match std::fs::copy(&dll_src, &dll_dest) {
                Ok(_) => println!("cargo:warning=✓ 已复制 wintun.dll -> target/{}", profile),
                Err(e) => println!("cargo:warning=✗ 复制 wintun.dll 失败: {}", e),
            }
            
            // 告诉 cargo 依赖 DLL
            println!("cargo:rerun-if-changed={}", dll_src.display());
        } else {
            println!("cargo:warning=✓ wintun.dll 将从 System32 加载");
        }
    }
    
    // 非 Windows 平台什么都不做
    #[cfg(not(target_os = "windows"))]
    {}
}

