/// 工具函数：格式化和打印数据包信息

pub fn format_hex_dump(data: &[u8], max_lines: usize) -> String {
    let mut output = String::new();
    let mut line_count = 0;
    
    for chunk in data.chunks(16) {
        if line_count >= max_lines {
            output.push_str("    ...\n");
            break;
        }
        
        // 偏移量
        output.push_str(&format!("{:04x}  ", line_count * 16));
        
        // 十六进制
        for (i, &byte) in chunk.iter().enumerate() {
            if i == 8 {
                output.push(' ');
            }
            output.push_str(&format!("{:02x} ", byte));
        }
        
        // 填充
        for _ in chunk.len()..16 {
            output.push_str("   ");
            if chunk.len() == 8 {
                output.push(' ');
            }
        }
        
        output.push_str(" │ ");
        
        // ASCII
        for &byte in chunk {
            if byte >= 32 && byte <= 126 {
                output.push(byte as char);
            } else {
                output.push('.');
            }
        }
        output.push_str(" │\n");
        line_count += 1;
    }
    
    output
}

pub fn print_packet_info(data: &[u8], src_ip: &str, dst_ip: &str, proto: &str) {
    log::info!("═══════════════════════════════════════════════════════════════");
    log::info!("📦 数据包信息:");
    log::info!("   源地址: {}", src_ip);
    log::info!("   目标: {}", dst_ip);
    log::info!("   协议: {}", proto);
    log::info!("   大小: {} 字节", data.len());
    log::info!("───────────────────────────────────────────────────────────────");
    
    let hex_dump = format_hex_dump(data, 10);
    for line in hex_dump.lines() {
        log::info!("   {}", line);
    }
    
    log::info!("═══════════════════════════════════════════════════════════════");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_dump() {
        let data = b"Hello, World!";
        let dump = format_hex_dump(data, 5);
        assert!(dump.contains("Hello"));
        assert!(!dump.is_empty());
    }

    #[test]
    fn test_hex_dump_binary() {
        let data = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05];
        let dump = format_hex_dump(&data, 5);
        assert!(dump.contains("00 01 02 03"));
    }
}
