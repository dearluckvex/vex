use bytes::Bytes;

#[derive(Debug, Clone)]
pub struct DnsPacket {
    pub id: u16,
    pub is_query: bool,
    pub questions: Vec<DnsQuestion>,
    pub answers: Vec<DnsAnswer>,
    pub raw: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct DnsQuestion {
    pub name: String,
    pub qtype: u16,
    pub qclass: u16,
}

#[derive(Debug, Clone)]
pub struct DnsAnswer {
    pub name: String,
    pub atype: u16,
    pub aclass: u16,
    pub ttl: u32,
    pub data: Vec<u8>,
}

impl DnsPacket {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 12 {
            return None;
        }

        let id = u16::from_be_bytes([data[0], data[1]]);
        let flags = u16::from_be_bytes([data[2], data[3]]);

        let is_query = (flags & 0x8000) == 0;
        let question_count = u16::from_be_bytes([data[4], data[5]]) as usize;

        let mut offset = 12;
        let mut questions = Vec::new();

        // 解析问题部分
        for _ in 0..question_count {
            if let Some((name, new_offset)) = Self::parse_domain_name(data, offset) {
                if new_offset + 4 > data.len() {
                    return None;
                }

                let qtype = u16::from_be_bytes([data[new_offset], data[new_offset + 1]]);
                let qclass = u16::from_be_bytes([data[new_offset + 2], data[new_offset + 3]]);

                questions.push(DnsQuestion {
                    name,
                    qtype,
                    qclass,
                });

                offset = new_offset + 4;
            } else {
                return None;
            }
        }

        // 解析回答部分（简化版，只读取计数）
        let answer_count = u16::from_be_bytes([data[6], data[7]]) as usize;
        let mut answers = Vec::new();

        for _ in 0..answer_count {
            if let Some((name, new_offset)) = Self::parse_domain_name(data, offset) {
                if new_offset + 10 > data.len() {
                    break;
                }

                let atype = u16::from_be_bytes([data[new_offset], data[new_offset + 1]]);
                let aclass = u16::from_be_bytes([data[new_offset + 2], data[new_offset + 3]]);
                let ttl = u32::from_be_bytes([
                    data[new_offset + 4],
                    data[new_offset + 5],
                    data[new_offset + 6],
                    data[new_offset + 7],
                ]);
                let rdlen =
                    u16::from_be_bytes([data[new_offset + 8], data[new_offset + 9]]) as usize;

                let rdata_offset = new_offset + 10;
                if rdata_offset + rdlen > data.len() {
                    break;
                }

                answers.push(DnsAnswer {
                    name,
                    atype,
                    aclass,
                    ttl,
                    data: data[rdata_offset..rdata_offset + rdlen].to_vec(),
                });

                offset = rdata_offset + rdlen;
            } else {
                break;
            }
        }

        Some(DnsPacket {
            id,
            is_query,
            questions,
            answers,
            raw: data.to_vec(),
        })
    }

    fn parse_domain_name(data: &[u8], mut offset: usize) -> Option<(String, usize)> {
        let mut name = String::new();
        let _original_offset = offset;

        loop {
            if offset >= data.len() {
                return None;
            }

            let len = data[offset] as usize;
            offset += 1;

            if len == 0 {
                break;
            }

            // 处理指针压缩（DNS 消息压缩）
            if (len & 0xc0) == 0xc0 {
                if offset >= data.len() {
                    return None;
                }
                let _ptr_offset = ((len & 0x3f) as u16) << 8 | data[offset] as u16;
                offset += 1;
                // 为简化，这里只做基础支持
                break;
            }

            if offset + len > data.len() {
                return None;
            }

            if !name.is_empty() {
                name.push('.');
            }

            if let Ok(label) = std::str::from_utf8(&data[offset..offset + len]) {
                name.push_str(label);
            }

            offset += len;
        }

        Some((name, offset))
    }

    pub fn is_dns_query(&self) -> bool {
        self.is_query && !self.questions.is_empty()
    }

    pub fn get_first_domain(&self) -> Option<String> {
        self.questions.first().map(|q| q.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dns_query_parsing() {
        // 完整的 DNS 查询包（包含完整的头和问题部分）
        let dns_query = [
            0x12, 0x34, // ID
            0x01, 0x00, // Flags (query)
            0x00, 0x01, // Questions
            0x00, 0x00, // Answer RRs
            0x00, 0x00, // Authority RRs
            0x00, 0x00, // Additional RRs
            // 问题部分：example.com A IN
            0x07, b'e', b'x', b'a', b'm', b'p', b'l', b'e',
            0x03, b'c', b'o', b'm',
            0x00,       // 名称结束
            0x00, 0x01, // Type: A
            0x00, 0x01, // Class: IN
        ];

        let packet = DnsPacket::parse(&dns_query);
        assert!(packet.is_some());

        let packet = packet.unwrap();
        assert_eq!(packet.id, 0x1234);
        assert!(packet.is_query);
        assert!(!packet.questions.is_empty());
    }

    #[test]
    fn test_dns_response_parsing() {
        // 完整的 DNS 响应包
        let dns_response = [
            0x12, 0x34, // ID
            0x81, 0x80, // Flags (response)
            0x00, 0x01, // Questions
            0x00, 0x00, // Answer RRs
            0x00, 0x00, // Authority RRs
            0x00, 0x00, // Additional RRs
            // 问题部分
            0x07, b'e', b'x', b'a', b'm', b'p', b'l', b'e',
            0x03, b'c', b'o', b'm',
            0x00,       // 名称结束
            0x00, 0x01, // Type: A
            0x00, 0x01, // Class: IN
        ];

        let packet = DnsPacket::parse(&dns_response);
        assert!(packet.is_some());

        let packet = packet.unwrap();
        assert_eq!(packet.id, 0x1234);
        assert!(!packet.is_query);
    }
}
