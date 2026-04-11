use std::env;
use std::path::PathBuf;

fn main() {
    #[cfg(target_os = "windows")]
    {
        // 获取构建输出目录
        let out_dir = env::var("OUT_DIR").unwrap();
        let out_path = PathBuf::from(&out_dir);
        let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
        
        // 构建目录和 DLL 位置
        let target_dir = out_path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        
        let dll_src = PathBuf::from("wintun.dll");
        let dll_dest = target_dir.join(&profile).join("wintun.dll");
        
        // 如果源文件存在，复制到输出目录
        if dll_src.exists() {
            match std::fs::copy(&dll_src, &dll_dest) {
                Ok(_) => println!("cargo:warning=已复制 wintun.dll 到输出目录"),
                Err(e) => println!("cargo:warning=复制 wintun.dll 失败: {}", e),
            }
        }
        
        // 告诉 cargo 我们依赖 wintun.dll
        println!("cargo:rerun-if-changed=wintun.dll");
    }
    
    // 非 Windows 平台什么都不做
    #[cfg(not(target_os = "windows"))]
    {}
}

