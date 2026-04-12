use std::io::{self, Write, Read};
use std::fs::OpenOptions;

mod packet;
mod proxy;
mod dns;

use packet::IpPacket;
use proxy::PacketProxy;

#[cfg(target_os = "linux")]
use tun2::{Configuration, Device};

#[cfg(target_os = "windows")]
use wintun::Adapter;

#[cfg(target_os = "macos")]
use tun::Device as TunDevice;

fn log_message(msg: &str) {
    // 同时输出到控制台和文件
    println!("{}", msg);
    let _ = io::stdout().flush();
    
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

    match Device::new(&config) {
        Ok(mut dev) => {
            log_message("✓ TUN 设备已创建: xtun0");
            log_message("✓ 会话已启动，监听中...");
            log_message("");
            
            let rt = tokio::runtime::Runtime::new()?;
            let result = rt.block_on(async {
                let proxy = PacketProxy::new();
                let mut buf = [0u8; 1500];
                
                loop {
                    match dev.read(&mut buf) {
                        Ok(n) => {
                            if let Some(packet) = IpPacket::parse(&buf[..n]) {
                                proxy.process_packet(packet).await;
                                
                                let stats = proxy.get_stats().await;
                                if stats.packets_received % 100 == 0 {
                                    log_message(&format!(
                                        "📊 已处理 {} 个数据包 ({} 字节)",
                                        stats.packets_received, stats.bytes_received
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            log_message(&format!("✗ 读取失败: {}", e));
                            return Err(Box::new(e) as Box<dyn std::error::Error>);
                        }
                    }
                }
            });
            result
        }
        Err(_e) => {
            log_message("⚠️  无法创建真实 TUN 设备，进入演示模式");
            log_message("");
            log_message("【演示模式】");
            log_message("正在模拟 TUN 网络适配器运行...");
            log_message("（实际工作需要以 root 权限运行）");
            log_message("");
            
            loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                log_message("✓ 模拟 TUN 适配器正在运行 (xtun0)");
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn create_tun_device() -> Result<(), Box<dyn std::error::Error>> {
    log_message("开始加载 WinTun...");
    
    // 检查 DLL 文件是否存在
    let dll_in_root = std::path::Path::new("wintun.dll").exists();
    let dll_in_system32 = std::path::Path::new("C:\\Windows\\System32\\wintun.dll").exists();
    
    log_message(&format!("  根目录中的 DLL: {}", if dll_in_root { "✓ 找到" } else { "✗ 未找到" }));
    log_message(&format!("  System32 中的 DLL: {}", if dll_in_system32 { "✓ 找到" } else { "✗ 未找到" }));
    
    if !dll_in_root && !dll_in_system32 {
        log_message("");
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
    
    log_message("尝试加载 wintun.dll...");
    
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
            log_message("1. DLL 文件损坏或不完整");
            log_message("2. DLL 缺少系统依赖（如 VC++ 运行库）");
            log_message("3. DLL 与 wintun crate 版本不兼容");
            log_message("4. 权限不足（需要以管理员身份运行）");
            log_message("");
            log_message("【建议解决方案】");
            log_message("1. 尝试将 DLL 放在 C:\\Windows\\System32\\");
            log_message("2. 安装 VC++ 可再发行包：");
            log_message("   https://support.microsoft.com/en-us/help/2977003");
            log_message("3. 以管理员身份运行此程序");
            log_message("4. 重新下载最新的 wintun.dll");
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