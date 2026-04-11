use std::io::{self, Write};
use std::fs::OpenOptions;

#[cfg(target_os = "linux")]
use std::io::Read;

#[cfg(target_os = "linux")]
use tun2::{Configuration, Device};

#[cfg(target_os = "windows")]
use wintun::Adapter;

#[cfg(target_os = "macos")]
use tun::Device as TunDevice;

fn log_message(msg: &str) {
    // 同时输出到控制台和文件
    eprintln!("{}", msg);
    let _ = io::stderr().flush();
    
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("xtune.log")
    {
        let _ = writeln!(file, "{}", msg);
    }
}

#[cfg(target_os = "linux")]
fn create_tun_device() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Configuration::default();
    config
        .address((10, 0, 0, 1))
        .netmask((255, 255, 255, 0))
        .up();
    config.tun_name("xtun0");

    let mut dev = Device::new(&config)?;
    
    let mut buf = [0u8; 1500];
    loop {
        let n = dev.read(&mut buf)?;
        println!("接收到原始 IP 包，长度: {} 字节", n);
    }
}

#[cfg(target_os = "windows")]
fn create_tun_device() -> Result<(), Box<dyn std::error::Error>> {
    log_message("开始加载 WinTun...");
    
    // 检查 DLL 文件是否存在
    let dll_exists = std::path::Path::new("wintun.dll").exists()
        || std::path::Path::new("C:\\Windows\\System32\\wintun.dll").exists();
    
    if !dll_exists {
        log_message("⚠️  wintun.dll 未找到，进入演示模式");
        log_message("");
        log_message("【演示模式】");
        log_message("正在模拟 TUN 网络适配器运行...");
        log_message("（实际工作需要安装真实的 wintun.dll）");
        log_message("");
        
        loop {
            std::thread::sleep(std::time::Duration::from_secs(2));
            eprintln!("✓ 模拟 TUN 适配器正在运行 (xtun)");
            let _ = io::stderr().flush();
        }
    }
    
    match unsafe { wintun::load() } {
        Ok(wintun) => {
            log_message("✓ WinTun 库已加载");
            
            match Adapter::create(&wintun, "xtun", "Xnet", None) {
                Ok(adapter) => {
                    log_message("✓ TUN 适配器已创建: xtun");
                    
                    match adapter.start_session(wintun::MAX_RING_CAPACITY) {
                        Ok(_session) => {
                            log_message("✓ TUN 会话已启动，监听中...");
                            
                            loop {
                                std::thread::sleep(std::time::Duration::from_secs(1));
                                let msg = "✓ 正在监听 TUN 适配器 (xtun)";
                                eprintln!("{}", msg);
                                let _ = io::stderr().flush();
                            }
                        }
                        Err(e) => {
                            let err_msg = format!("✗ 启动会话失败: {}", e);
                            log_message(&err_msg);
                            return Err(Box::new(e));
                        }
                    }
                }
                Err(e) => {
                    let err_msg = format!("✗ 创建适配器失败: {}", e);
                    log_message(&err_msg);
                    log_message("提示: 需要安装 WinTun 驱动程序");
                    return Err(Box::new(e));
                }
            }
        }
        Err(e) => {
            let err_msg = format!("✗ 加载 WinTun 失败: {}", e);
            log_message(&err_msg);
            log_message("");
            log_message("【可能的原因】");
            log_message("1. wintun.dll 架构不匹配（32位 vs 64位）");
            log_message("2. wintun.dll 损坏或不完整");
            log_message("3. 缺少系统依赖");
            log_message("");
            log_message("【解决方案】");
            log_message("方案 A: 重新下载 WinTun x64 版本");
            log_message("  下载链接: https://www.wintun.net/");
            log_message("  确保是 x64 版本，不是 x86");
            log_message("");
            log_message("方案 B: 进入演示模式（不需要 DLL）");
            log_message("  删除或重命名 wintun.dll 文件");
            log_message("  程序会进入演示模式运行");
            log_message("");
            return Err(Box::new(e));
        }
    }
}

#[cfg(target_os = "macos")]
fn create_tun_device() -> Result<(), Box<dyn std::error::Error>> {
    let mut dev = TunDevice::new()?;
    dev.set_address(std::net::Ipv4Addr::new(10, 0, 0, 1))?;
    dev.set_netmask(std::net::Ipv4Addr::new(255, 255, 255, 0))?;
    dev.set_mtu(1500)?;
    
    println!("TUN 设备已创建: {}", dev.name()?);
    
    let mut buf = [0u8; 1500];
    loop {
        let n = dev.read(&mut buf)?;
        println!("接收到原始 IP 包，长度: {} 字节", n);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    log_message("\n╔════════════════════════════════════════════════════════════════╗");
    log_message("║                   🚀 xTune TUN 网络适配器                     ║");
    log_message("╚════════════════════════════════════════════════════════════════╝\n");
    
    #[cfg(target_os = "linux")]
    log_message("📍 平台: Linux");
    
    #[cfg(target_os = "windows")]
    log_message("📍 平台: Windows");
    
    #[cfg(target_os = "macos")]
    log_message("📍 平台: macOS");
    
    log_message("");
    
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        log_message("❌ 不支持的操作系统");
        return Err("Unsupported OS".into());
    }
    
    match create_tun_device() {
        Ok(_) => Ok(()),
        Err(e) => {
            log_message(&format!("程序退出，错误: {}", e));
            Err(e)
        }
    }
}