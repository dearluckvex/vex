#[cfg(target_os = "linux")]
use std::io::Read;

#[cfg(target_os = "linux")]
use tun2::{Configuration, Device};

#[cfg(target_os = "windows")]
use wintun::Adapter;

#[cfg(target_os = "macos")]
use tun::Device as TunDevice;

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
    println!("Windows TUN 适配器初始化...");
    println!();
    
    match unsafe { wintun::load() } {
        Ok(wintun) => {
            match Adapter::create(&wintun, "xtun", "Xnet", None) {
                Ok(adapter) => {
                    println!("✓ TUN 适配器已创建: xtun");
                    match adapter.start_session(wintun::MAX_RING_CAPACITY) {
                        Ok(_session) => {
                            println!("✓ TUN 会话已启动，监听中...");
                            loop {
                                std::thread::sleep(std::time::Duration::from_secs(1));
                                println!("✓ 正在监听 TUN 适配器 (xtun)");
                            }
                        }
                        Err(e) => {
                            eprintln!("✗ 启动会话失败: {}", e);
                            return Err(Box::new(e));
                        }
                    }
                }
                Err(e) => {
                    eprintln!("✗ 创建适配器失败: {}", e);
                    eprintln!("提示: 需要安装 WinTun 驱动程序");
                    return Err(Box::new(e));
                }
            }
        }
        Err(e) => {
            eprintln!("╔════════════════════════════════════════════════════════════════╗");
            eprintln!("║        ✗ 加载 WinTun 驱动库失败                                ║");
            eprintln!("╚════════════════════════════════════════════════════════════════╝");
            eprintln!();
            eprintln!("错误: {}", e);
            eprintln!();
            eprintln!("【解决方案】");
            eprintln!("方案 A: 自动加载（推荐）");
            eprintln!("  将 wintun.dll 放在项目根目录，重新编译：");
            eprintln!("  1. 下载: https://www.wintun.net/");
            eprintln!("  2. 提取 wintun.dll 到项目目录");
            eprintln!("  3. 运行: cargo build --release");
            eprintln!("  4. DLL 会自动打包到 target/release/ 目录");
            eprintln!();
            eprintln!("方案 B: 手动安装");
            eprintln!("  1. 下载 WinTun: https://www.wintun.net/");
            eprintln!("  2. 将 wintun.dll 放到: C:\\Windows\\System32\\");
            eprintln!();
            eprintln!("【注意】需要管理员权限运行此程序");
            eprintln!();
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
    println!("启动 xTune TUN 网络适配器...");
    
    #[cfg(target_os = "linux")]
    println!("平台: Linux");
    
    #[cfg(target_os = "windows")]
    println!("平台: Windows");
    
    #[cfg(target_os = "macos")]
    println!("平台: macOS");
    
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        eprintln!("不支持的操作系统");
        return Err("Unsupported OS".into());
    }
    
    create_tun_device()
}